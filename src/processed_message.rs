use crate::{Message, message::Direction};

#[derive(Debug)]
pub enum ProcessedMessage {
    Forward(Message),
    WithMessages {
        message: Message,
        generated_messages: Vec<(Direction, Message)>,
    },
    Ignore {
        generated_messages: Vec<(Direction, Message)>,
    },
}

impl ProcessedMessage {
    pub fn get_message(&self) -> Option<&Message> {
        match self {
            ProcessedMessage::Forward(msg) => Some(msg),
            ProcessedMessage::WithMessages { message, .. } => Some(message),
            ProcessedMessage::Ignore { .. } => None,
        }
    }

    pub fn get_generated_messages(&self) -> &[(Direction, Message)] {
        match self {
            ProcessedMessage::Forward(_) => &[],
            ProcessedMessage::WithMessages {
                generated_messages, ..
            }
            | ProcessedMessage::Ignore { generated_messages } => generated_messages,
        }
    }

    pub fn into_parts(self) -> (Option<Message>, Vec<(Direction, Message)>) {
        match self {
            ProcessedMessage::Forward(msg) => (Some(msg), Vec::new()),
            ProcessedMessage::WithMessages {
                message,
                generated_messages,
            } => (Some(message), generated_messages),
            ProcessedMessage::Ignore { generated_messages } => (None, generated_messages),
        }
    }
}
