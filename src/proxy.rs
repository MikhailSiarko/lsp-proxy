use crate::Message;
use crate::hooks::{Hook, HookError};
use crate::message::Direction;
use crate::processed_message::ProcessedMessage;
use crate::transport::{read_message, write_message};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::select;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{self, UnboundedSender};

pub struct Proxy {
    hooks: HashMap<String, Arc<dyn Hook>>,
    pending_requests: HashMap<i64, String>,
}

impl Proxy {
    fn new(hooks: HashMap<String, Arc<dyn Hook>>) -> Self {
        Self {
            hooks,
            pending_requests: HashMap::new(),
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
        let hooks = Arc::new(self.hooks);
        let pending_requests = Arc::new(Mutex::new(self.pending_requests));

        let (client_sender, mut client_receiver) = mpsc::unbounded_channel::<Message>();
        let (server_sender, mut server_receiver) = mpsc::unbounded_channel::<Message>();

        let server_message_sender = server_sender.clone();
        let client_message_sender = client_sender.clone();
        let hooks_client = Arc::clone(&hooks);
        let pending_requests_client = Arc::clone(&pending_requests);
        let server_to_client_task = tokio::spawn(async move {
            forward_to_client(
                hooks_client,
                pending_requests_client,
                server_reader,
                server_message_sender,
                client_message_sender,
            )
            .await
        });

        let hooks_server = Arc::clone(&hooks);
        let pending_requests_server = Arc::clone(&pending_requests);
        let client_to_server_task = tokio::spawn(async move {
            forward_to_server(
                hooks_server,
                pending_requests_server,
                client_reader,
                server_sender,
                client_sender,
            )
            .await
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

        select! {
            client_to_server = client_to_server_task => {
                client_to_server?
            },
            server_to_client = server_to_client_task => {
                server_to_client?
            },
            write_server = write_to_server => {
                write_server?
            },
            write_client = write_to_client => {
                write_client?
            }
        }
    }
}

async fn process_message(
    hooks: &HashMap<String, Arc<dyn Hook>>,
    pending_requests: &Mutex<HashMap<i64, String>>,
    message: Message,
) -> Result<ProcessedMessage, HookError> {
    match message {
        Message::Request(request) => match hooks.get(&request.method) {
            Some(hook) => {
                pending_requests
                    .lock()
                    .await
                    .insert(request.id, request.method.clone());

                Ok(hook.on_request(request).await?.as_processed())
            }
            None => Ok(ProcessedMessage::Forward(Message::Request(request))),
        },
        Message::Notification(notification) => match hooks.get(&notification.method) {
            Some(hook) => Ok(hook.on_notification(notification).await?.as_processed()),
            None => Ok(ProcessedMessage::Forward(Message::Notification(
                notification,
            ))),
        },
        Message::Response(response) => {
            let method = { pending_requests.lock().await.remove(&response.id) };

            if let Some(method) = method
                && let Some(hook) = hooks.get(&method)
            {
                return Ok(hook.on_response(response).await?.as_processed());
            }

            Ok(ProcessedMessage::Forward(Message::Response(response)))
        }
    }
}

impl Default for Proxy {
    fn default() -> Self {
        Self::new(HashMap::new())
    }
}

async fn forward_to_server<R>(
    hooks: Arc<HashMap<String, Arc<dyn Hook>>>,
    pending_requests: Arc<Mutex<HashMap<i64, String>>>,
    mut client_reader: R,
    server_message_sender: UnboundedSender<Message>,
    client_message_sender: UnboundedSender<Message>,
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
            match process_message(&hooks, &pending_requests, message).await {
                Ok(processed) => {
                    let (main_message, generated_messages) = processed.into_parts();

                    if let Some(main_message) = main_message
                        && server_message_sender.send(main_message).is_err()
                    {
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

                        if result.is_err() {
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
    hooks: Arc<HashMap<String, Arc<dyn Hook>>>,
    pending_requests: Arc<Mutex<HashMap<i64, String>>>,
    mut server_reader: R,
    server_message_sender: UnboundedSender<Message>,
    client_message_sender: UnboundedSender<Message>,
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
            match process_message(&hooks, &pending_requests, message).await {
                Ok(processed) => {
                    let (main_message, generated_messages) = processed.into_parts();

                    if let Some(main_message) = main_message
                        && client_message_sender.send(main_message).is_err()
                    {
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

                        if result.is_err() {
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
