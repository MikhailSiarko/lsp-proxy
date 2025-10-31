use async_trait::async_trait;
use std::fmt::Display;

use crate::{
    Message, Notification, Request, Response, message::Direction,
    processed_message::ProcessedMessage,
};

#[derive(Debug)]
pub enum HookError {
    ProcessingFailed(String),
}

impl Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookError::ProcessingFailed(msg) => write!(f, "Hook processing failed: {}", msg),
        }
    }
}

impl std::error::Error for HookError {}

#[derive(Debug)]
pub struct HookOutput {
    pub message: Option<Message>,
    pub generated_messages: Vec<(Direction, Message)>,
}

impl HookOutput {
    pub fn new(message: Message) -> Self {
        Self {
            message: Some(message),
            generated_messages: Vec::new(),
        }
    }

    pub fn empty() -> Self {
        Self {
            message: None,
            generated_messages: Vec::new(),
        }
    }

    pub fn with_message(mut self, direction: Direction, message: Message) -> Self {
        self.generated_messages.push((direction, message));
        self
    }

    pub fn with_messages(mut self, messages: Vec<(Direction, Message)>) -> Self {
        self.generated_messages.extend(messages);
        self
    }

    pub fn as_processed(self) -> ProcessedMessage {
        match self.message {
            Some(message) => {
                if self.generated_messages.is_empty() {
                    return ProcessedMessage::Forward(message);
                }

                ProcessedMessage::WithMessages {
                    message,
                    generated_messages: self.generated_messages,
                }
            }
            None => ProcessedMessage::Ignore {
                generated_messages: self.generated_messages,
            },
        }
    }
}

pub type HookResult = Result<HookOutput, HookError>;

#[async_trait]
pub trait Hook: Send + Sync {
    async fn on_request(&self, request: Request) -> HookResult {
        Ok(HookOutput::new(Message::Request(request)))
    }

    async fn on_response(&self, response: Response) -> HookResult {
        Ok(HookOutput::new(Message::Response(response)))
    }

    async fn on_notification(&self, notification: Notification) -> HookResult {
        Ok(HookOutput::new(Message::Notification(notification)))
    }
}
