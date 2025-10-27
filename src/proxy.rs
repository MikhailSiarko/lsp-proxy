use crate::hooks::{Hook, HookError, Message};
use crate::transport::{read_message, write_message};
use serde_json::Value;
use smol::io::{AsyncReadExt, AsyncWriteExt};
use smol::process::{Command, Stdio};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub struct Proxy {
    hooks: HashMap<String, Arc<dyn Hook>>,
    pending_requests: Arc<Mutex<HashMap<Value, String>>>,
}

impl Proxy {
    fn new(hooks: HashMap<String, Arc<dyn Hook>>) -> Self {
        Self {
            hooks,
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn process_client_message(
        &self,
        message: Message,
    ) -> Result<ProcessedMessage, HookError> {
        match &message {
            Message::Request { id, method, .. } => {
                if let Some(hook) = self.hooks.get(method) {
                    {
                        let mut pending = self.pending_requests.lock().unwrap();
                        pending.insert(id.clone(), method.clone());
                    }

                    let output = hook.on_request(message).await?;
                    Ok(ProcessedMessage::WithNotifications {
                        message: output.message,
                        notifications: output.notifications,
                    })
                } else {
                    {
                        let mut pending = self.pending_requests.lock().unwrap();
                        pending.insert(id.clone(), method.clone());
                    }
                    Ok(ProcessedMessage::Forward(message))
                }
            }
            Message::Notification { .. } => Ok(ProcessedMessage::Forward(message)),
            Message::Response { .. } => Ok(ProcessedMessage::Forward(message)),
        }
    }

    async fn process_server_message(
        &self,
        message: Message,
    ) -> Result<ProcessedMessage, HookError> {
        match &message {
            Message::Response { id, .. } => {
                let method = {
                    let mut pending = self.pending_requests.lock().unwrap();
                    pending.remove(id)
                };

                if let Some(method) = method
                    && let Some(hook) = self.hooks.get(&method)
                {
                    let output = hook.on_response(message).await?;
                    return Ok(ProcessedMessage::WithNotifications {
                        message: output.message,
                        notifications: output.notifications,
                    });
                }

                Ok(ProcessedMessage::Forward(message))
            }
            Message::Notification { .. } => Ok(ProcessedMessage::Forward(message)),
            Message::Request { .. } => Ok(ProcessedMessage::Forward(message)),
        }
    }

    async fn process_message(
        &self,
        message: Value,
        from_client: bool,
    ) -> Result<ProcessedMessage, String> {
        let parsed_message = Message::from_value(message)?;

        let result = if from_client {
            self.process_client_message(parsed_message).await
        } else {
            self.process_server_message(parsed_message).await
        };

        result.map_err(|e| e.to_string())
    }

    pub async fn spawn<R, W>(
        self,
        command: &str,
        args: &[&str],
        client_reader: R,
        client_writer: W,
    ) -> std::io::Result<()>
    where
        R: AsyncReadExt + Unpin + Send + 'static,
        W: AsyncWriteExt + Unpin + Send + 'static,
    {
        let proxy = Arc::new(self);
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        let server_writer = child.stdin.take().unwrap();
        let server_reader = child.stdout.take().unwrap();

        let client_to_server_proxy = proxy.clone();
        let server_to_client_proxy = proxy.clone();

        let (client_sender, client_receiver) = smol::channel::unbounded::<Message>();
        let (server_sender, server_receiver) = smol::channel::unbounded::<Message>();

        let server_message_sender = server_sender.clone();
        let server_notification_sender = server_sender.clone();
        let client_message_sender = client_sender.clone();
        let client_notification_sender = client_sender.clone();

        let client_to_server = smol::spawn(async move {
            forward_messages(
                &client_to_server_proxy,
                client_reader,
                server_message_sender,
                server_notification_sender,
                true,
            )
            .await
        });

        let server_to_client = smol::spawn(async move {
            forward_messages(
                &server_to_client_proxy,
                server_reader,
                client_message_sender,
                client_notification_sender,
                false,
            )
            .await
        });

        let write_to_server = smol::spawn(async move {
            let mut writer = server_writer;
            while let Ok(msg) = server_receiver.recv().await {
                if write_message(&mut writer, &msg.to_value()).await.is_err() {
                    break;
                }
            }
            Ok::<(), std::io::Error>(())
        });

        let write_to_client = smol::spawn(async move {
            let mut writer = client_writer;
            while let Ok(msg) = client_receiver.recv().await {
                if write_message(&mut writer, &msg.to_value()).await.is_err() {
                    break;
                }
            }
            Ok::<(), std::io::Error>(())
        });

        _ = smol::future::zip(client_to_server, server_to_client).await;

        let _ = child.kill();

        drop(write_to_server);
        drop(write_to_client);

        // forward_result.map_err(std::io::Error::other)
        Ok(())
    }
}

impl Default for Proxy {
    fn default() -> Self {
        Self::new(HashMap::new())
    }
}

#[derive(Debug)]
pub enum ProcessedMessage {
    Forward(Message),
    WithNotifications {
        message: Message,
        notifications: Vec<Message>,
    },
}

impl ProcessedMessage {
    pub fn get_message(&self) -> &Message {
        match self {
            ProcessedMessage::Forward(msg) => msg,
            ProcessedMessage::WithNotifications { message, .. } => message,
        }
    }

    pub fn get_notifications(&self) -> &[Message] {
        match self {
            ProcessedMessage::Forward(_) => &[],
            ProcessedMessage::WithNotifications { notifications, .. } => notifications,
        }
    }

    pub fn into_parts(self) -> (Message, Vec<Message>) {
        match self {
            ProcessedMessage::Forward(msg) => (msg, Vec::new()),
            ProcessedMessage::WithNotifications {
                message,
                notifications,
            } => (message, notifications),
        }
    }
}

async fn forward_messages<R>(
    proxy: &Proxy,
    mut reader: R,
    message_sender: smol::channel::Sender<Message>,
    notification_sender: smol::channel::Sender<Message>,
    from_client: bool,
) -> std::io::Result<()>
where
    R: AsyncReadExt + Unpin,
{
    loop {
        let message = match read_message(&mut reader).await {
            Ok(msg) => msg,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => return Err(e),
        };

        match proxy.process_message(message, from_client).await {
            Ok(processed) => {
                let (main_message, notifications) = processed.into_parts();

                if message_sender.send(main_message).await.is_err() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "Message channel closed",
                    ));
                }

                for notification in notifications {
                    if notification_sender.send(notification).await.is_err() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::BrokenPipe,
                            "Notification channel closed",
                        ));
                    }
                }
            }
            Err(e) => {
                eprintln!("Error processing message: {}", e);
            }
        }
    }

    Ok(())
}

pub struct ProxyBuilder {
    hooks: HashMap<String, Arc<dyn Hook>>,
}

impl ProxyBuilder {
    pub fn new() -> Self {
        Self {
            hooks: HashMap::new(),
        }
    }

    pub fn with_hook(mut self, method: &str, hook: Arc<dyn Hook>) -> Self {
        self.hooks.insert(method.to_owned(), hook);
        self
    }

    pub fn build(self) -> Proxy {
        Proxy::new(self.hooks)
    }
}

impl Default for ProxyBuilder {
    fn default() -> Self {
        Self::new()
    }
}
