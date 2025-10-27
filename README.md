# LSP Proxy

A Rust library for proxying LSP (Language Server Protocol) messages with hooks for interception, modification, and notification generation.

## Features

- **Message Forwarding**: Proxy messages between LSP client and server
- **Request/Response Hooks**: Intercept and modify requests and responses by method name
- **Notification Generation**: Generate notifications while processing messages
- **Process Management**: Spawn and manage LSP server processes
- **Async**: Built with `smol` for async operations

## Installation

```bash
cargo add lsp_proxy
```

## Quick Start

```rust
use async_trait::async_trait;
use lsp_proxy::{Hook, HookOutput, HookResult, Message, Proxy};
use std::sync::Arc;
use serde_json::json;

// Define a hook
struct MyHook;

#[async_trait]
impl Hook for MyHook {
    async fn on_request(&self, message: Message) -> HookResult {
        // Optionally modify the request and generate notifications
        let notification = Message::notification(
            "window/logMessage"
            Some(json!({"type": 4, "message": "Processing request"}))
        );

        Ok(HookOutput::new(message).with_notification(notification))
    }

    // Default implementation for messages is to forward them unmodified
    // You only need to implement on_response if you want to process responses
    async fn on_response(&self, message: Message) -> HookResult {
        // Process the response
        Ok(HookOutput::new(message))
    }
}

fn main() -> std::io::Result<()> {
    smol::block_on(async {
        // Create and configure proxy
        let proxy = ProxyBuilder::new()
            .with_hook("textDocument/completion" Arc::new(MyHook));

        // Spawn LSP server and forward messages
        proxy.spawn(
            "rust-analyzer",
            &[/* Forrard LSP args */],
            client_reader,
            client_writer,
        ).await?;
        Ok(())
    }}
}
```

## Key Concepts

### Hooks
- Register hooks by LSP method name (`textDocument/completion`, `textDocument/hover`, etc.)
- Hooks process **requests** and **responses** only
- Notifications are forwarded without processing (by design)
- Responses are matched to hooks by tracking request IDs

### Message Flow
```
Client → Proxy → Server
  ↓        ↓
  └─ Hook processes request
     └─ Can generate notifications → Client

Server → Proxy → Client
         ↓
         └─ Hook processes response (matched by request ID)
            └─ Can generate notifications → Client
```

### API

**Proxy**
- `spawn(cmd, args, reader, writer)` - Spawn server and forward messages

**Hook Trait**
- `on_request(message) -> HookResult` - Process request
- `on_response(message) -> HookResult` - Process response

**HookOutput**
- `new(message)` - Create with modified message
- `with_notification(notif)` - Add notification (chainable)

**Message**
- `notification(method, params)` - Create notification
- `to_value()` - Convert to JSON
- `from_value(json)` - Parse from JSON

## License

This project is provided as-is for educational and development purposes.
