use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone)]
pub enum Direction {
    ToClient,
    ToServer,
}

#[derive(Debug, Clone, Serialize)]
pub struct Request {
    pub id: i64,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Response {
    pub id: i64,
    pub result: Option<Value>,
    pub error: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Notification {
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Request(Request),
    Response(Response),
    Notification(Notification),
}

impl Message {
    pub fn from_value(value: Value) -> Result<Self, String> {
        let obj = value.as_object().ok_or("Message must be an object")?;

        let id = obj.get("id").and_then(|id| id.as_i64());
        let method = obj.get("method").and_then(|m| m.as_str()).map(String::from);
        let params = obj.get("params").cloned();
        let result = obj.get("result").cloned();
        let error = obj.get("error").cloned();

        match (id, method, result.is_some() || error.is_some()) {
            (Some(id), Some(method), false) => Ok(Message::Request(Request { id, method, params })),
            (Some(id), None, true) => Ok(Message::Response(Response { id, result, error })),
            (None, Some(method), false) => {
                Ok(Message::Notification(Notification { method, params }))
            }
            _ => Err("Invalid message format".to_string()),
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Message::Request(Request { id, method, params }) => {
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
            Message::Response(Response { id, result, error }) => {
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
            Message::Notification(Notification { method, params }) => {
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
            Message::Request(Request { method, .. }) => Some(method),
            Message::Response(Response { .. }) => None,
            Message::Notification(Notification { method, .. }) => Some(method),
        }
    }

    pub fn get_id(&self) -> Option<&i64> {
        match self {
            Message::Request(Request { id, .. }) => Some(id),
            Message::Response(Response { id, .. }) => Some(id),
            Message::Notification(Notification { .. }) => None,
        }
    }

    pub fn notification(method: &str, params: Option<Value>) -> Self {
        Message::Notification(Notification {
            method: method.to_owned(),
            params,
        })
    }
}
