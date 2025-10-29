use crate::hooks::{Direction, Hook, HookError, Message};
use crate::transport::{read_message, write_message};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::join;
use tokio::sync::mpsc::{self, Sender};

pub struct Proxy {
    hooks: DashMap<String, Arc<dyn Hook>>,
    pending_requests: DashMap<i64, String>,
}

impl Proxy {
    fn new(hooks: DashMap<String, Arc<dyn Hook>>) -> Self {
        Self {
            hooks,
            pending_requests: DashMap::new(),
        }
    }

    async fn process_client_message(
        &self,
        message: Message,
    ) -> Result<ProcessedMessage, HookError> {
        match message {
            Message::Request(request) => match self.hooks.get(&request.method) {
                Some(hook) => {
                    self.pending_requests
                        .insert(request.id, request.method.clone());
                    let output = hook.on_request(request).await?;
                    Ok(ProcessedMessage::WithMessages {
                        message: output.message,
                        generated_messages: output.generated_messages,
                    })
                }
                None => Ok(ProcessedMessage::Forward(Message::Request(request))),
            },
            Message::Notification(notification) => match self.hooks.get(&notification.method) {
                Some(hook) => {
                    let output = hook.on_notification(notification).await?;
                    Ok(ProcessedMessage::Forward(output.message))
                }
                None => Ok(ProcessedMessage::Forward(Message::Notification(
                    notification,
                ))),
            },
            Message::Response { .. } => Ok(ProcessedMessage::Forward(message)),
        }
    }

    async fn process_server_message(
        &self,
        message: Message,
    ) -> Result<ProcessedMessage, HookError> {
        match message {
            Message::Response(response) => {
                let method = self
                    .pending_requests
                    .remove(&response.id)
                    .map(|(_, method)| method);

                if let Some(method) = method
                    && let Some(hook) = self.hooks.get(&method)
                {
                    let output = hook.on_response(response).await?;
                    return Ok(ProcessedMessage::WithMessages {
                        message: output.message,
                        generated_messages: output.generated_messages,
                    });
                }

                Ok(ProcessedMessage::Forward(Message::Response(response)))
            }
            Message::Notification(notification) => match self.hooks.get(&notification.method) {
                Some(hook) => {
                    let output = hook.on_notification(notification).await?;
                    Ok(ProcessedMessage::Forward(output.message))
                }
                None => Ok(ProcessedMessage::Forward(Message::Notification(
                    notification,
                ))),
            },
            Message::Request { .. } => Ok(ProcessedMessage::Forward(message)),
        }
    }

    pub async fn forward<SR, SW, CR, CW>(
        self,
        server_reader: SR,
        server_writer: SW,
        client_reader: CR,
        client_writer: CW,
    ) -> std::io::Result<()>
    where
        SR: AsyncReadExt + Unpin + Send + 'static,
        SW: AsyncWriteExt + Unpin + Send + 'static,
        CR: AsyncReadExt + Unpin + Send + 'static,
        CW: AsyncWriteExt + Unpin + Send + 'static,
    {
        let proxy = Arc::new(self);
        let server_to_client_proxy = proxy.clone();

        let (client_sender, mut client_receiver) = mpsc::channel::<Message>(100);
        let (server_sender, mut server_receiver) = mpsc::channel::<Message>(100);

        let server_message_sender = server_sender.clone();
        let client_message_sender = client_sender.clone();
        let server_to_client = tokio::spawn(async move {
            forward_to_client(
                &server_to_client_proxy,
                server_reader,
                server_message_sender,
                client_message_sender,
            )
            .await
        });

        let client_to_server = tokio::spawn(async move {
            forward_to_server(&proxy, client_reader, server_sender, client_sender).await
        });

        let write_to_server = tokio::spawn(async move {
            let mut writer = server_writer;
            while let Some(msg) = server_receiver.recv().await {
                if write_message(&mut writer, &msg.to_value()).await.is_err() {
                    break;
                }
            }
            Ok::<(), std::io::Error>(())
        });

        let write_to_client = tokio::spawn(async move {
            let mut writer = client_writer;
            while let Some(msg) = client_receiver.recv().await {
                if write_message(&mut writer, &msg.to_value()).await.is_err() {
                    break;
                }
            }
            Ok::<(), std::io::Error>(())
        });

        _ = join!(client_to_server, server_to_client);

        drop(write_to_server);
        drop(write_to_client);
        Ok(())
    }
}

impl Default for Proxy {
    fn default() -> Self {
        Self::new(DashMap::new())
    }
}

#[derive(Debug)]
pub enum ProcessedMessage {
    Forward(Message),
    WithMessages {
        message: Message,
        generated_messages: Vec<(Direction, Message)>,
    },
}

impl ProcessedMessage {
    pub fn get_message(&self) -> &Message {
        match self {
            ProcessedMessage::Forward(msg) => msg,
            ProcessedMessage::WithMessages { message, .. } => message,
        }
    }

    pub fn get_generated_messages(&self) -> &[(Direction, Message)] {
        match self {
            ProcessedMessage::Forward(_) => &[],
            ProcessedMessage::WithMessages {
                generated_messages: messages,
                ..
            } => messages,
        }
    }

    pub fn into_parts(self) -> (Message, Vec<(Direction, Message)>) {
        match self {
            ProcessedMessage::Forward(msg) => (msg, Vec::new()),
            ProcessedMessage::WithMessages {
                message,
                generated_messages: messages,
            } => (message, messages),
        }
    }
}

async fn forward_to_server<R>(
    proxy: &Proxy,
    mut client_reader: R,
    server_message_sender: Sender<Message>,
    client_message_sender: Sender<Message>,
) -> std::io::Result<()>
where
    R: AsyncReadExt + Unpin,
{
    loop {
        let message = match read_message(&mut client_reader).await {
            Ok(msg) => Message::from_value(msg),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => return Err(e),
        };

        if let Ok(message) = message {
            match proxy.process_client_message(message).await {
                Ok(processed) => {
                    let (main_message, generated_messages) = processed.into_parts();

                    if server_message_sender.send(main_message).await.is_err() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::BrokenPipe,
                            "Message channel closed",
                        ));
                    }

                    for (direction, message) in generated_messages {
                        let result = match direction {
                            Direction::ToClient => client_message_sender.send(message),
                            Direction::ToServer => server_message_sender.send(message),
                        };

                        if result.await.is_err() {
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
    }

    Ok(())
}

async fn forward_to_client<R>(
    proxy: &Proxy,
    mut server_reader: R,
    server_message_sender: Sender<Message>,
    client_message_sender: Sender<Message>,
) -> std::io::Result<()>
where
    R: AsyncReadExt + Unpin,
{
    loop {
        let message = match read_message(&mut server_reader).await {
            Ok(msg) => Message::from_value(msg),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => return Err(e),
        };

        if let Ok(message) = message {
            match proxy.process_server_message(message).await {
                Ok(processed) => {
                    let (main_message, generated_messages) = processed.into_parts();

                    if client_message_sender.send(main_message).await.is_err() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::BrokenPipe,
                            "Message channel closed",
                        ));
                    }

                    for (direction, message) in generated_messages {
                        let result = match direction {
                            Direction::ToClient => client_message_sender.send(message),
                            Direction::ToServer => server_message_sender.send(message),
                        };

                        if result.await.is_err() {
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
    }

    Ok(())
}

pub struct ProxyBuilder {
    hooks: DashMap<String, Arc<dyn Hook>>,
}

impl ProxyBuilder {
    pub fn new() -> Self {
        Self {
            hooks: DashMap::new(),
        }
    }

    pub fn with_hook(self, method: &str, hook: Arc<dyn Hook>) -> Self {
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
