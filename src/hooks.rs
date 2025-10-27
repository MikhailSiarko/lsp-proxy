use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use std::fmt::Display;

#[derive(Debug, Clone, Serialize)]
pub enum Message {
    Request {
        id: Value,
        method: String,
        params: Option<Value>,
    },
    Response {
        id: Value,
        result: Option<Value>,
        error: Option<Value>,
    },
    Notification {
        method: String,
        params: Option<Value>,
    },
}

impl Message {
    pub fn from_value(value: Value) -> Result<Self, String> {
        let obj = value.as_object().ok_or("Message must be an object")?;

        let id = obj.get("id").cloned();
        let method = obj.get("method").and_then(|m| m.as_str()).map(String::from);
        let params = obj.get("params").cloned();
        let result = obj.get("result").cloned();
        let error = obj.get("error").cloned();

        match (id, method, result.is_some() || error.is_some()) {
            (Some(id), Some(method), false) => Ok(Message::Request { id, method, params }),
            (Some(id), None, true) => Ok(Message::Response { id, result, error }),
            (None, Some(method), false) => Ok(Message::Notification { method, params }),
            _ => Err("Invalid message format".to_string()),
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Message::Request { id, method, params } => {
                let mut obj = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": method,
                });
                if let Some(params) = params {
                    obj["params"] = params.clone();
                }
                obj
            }
            Message::Response { id, result, error } => {
                let mut obj = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                });
                if let Some(result) = result {
                    obj["result"] = result.clone();
                }
                if let Some(error) = error {
                    obj["error"] = error.clone();
                }
                obj
            }
            Message::Notification { method, params } => {
                let mut obj = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": method,
                });
                if let Some(params) = params {
                    obj["params"] = params.clone();
                }
                obj
            }
        }
    }

    pub fn get_method(&self) -> Option<&str> {
        match self {
            Message::Request { method, .. } => Some(method),
            Message::Response { .. } => None,
            Message::Notification { method, .. } => Some(method),
        }
    }

    pub fn get_id(&self) -> Option<&Value> {
        match self {
            Message::Request { id, .. } => Some(id),
            Message::Response { id, .. } => Some(id),
            Message::Notification { .. } => None,
        }
    }

    pub fn notification(method: &str, params: Option<Value>) -> Self {
        Message::Notification {
            method: method.to_owned(),
            params,
        }
    }
}

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
    pub message: Message,
    pub notifications: Vec<Message>,
}

impl HookOutput {
    pub fn new(message: Message) -> Self {
        Self {
            message,
            notifications: Vec::new(),
        }
    }

    pub fn with_notification(mut self, notification: Message) -> Self {
        self.notifications.push(notification);
        self
    }

    pub fn with_notifications(mut self, notifications: Vec<Message>) -> Self {
        self.notifications.extend(notifications);
        self
    }

    pub fn add_notification(&mut self, notification: Message) {
        self.notifications.push(notification);
    }
}

pub type HookResult = Result<HookOutput, HookError>;

#[async_trait]
pub trait Hook: Send + Sync {
    async fn on_request(&self, message: Message) -> HookResult {
        Ok(HookOutput::new(message))
    }

    async fn on_response(&self, message: Message) -> HookResult {
        Ok(HookOutput::new(message))
    }
}
