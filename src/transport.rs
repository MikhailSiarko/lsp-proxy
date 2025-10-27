use serde_json::Value;
use smol::io::{AsyncReadExt, AsyncWriteExt};
use std::io;

pub async fn read_message<R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<Value> {
    let mut buffer = Vec::new();
    let mut header_buf = [0u8; 1];
    let mut content_length: Option<usize> = None;

    // Read headers
    loop {
        buffer.clear();

        // Read line character by character until we hit \r\n
        loop {
            reader.read_exact(&mut header_buf).await?;
            if header_buf[0] == b'\r' {
                reader.read_exact(&mut header_buf).await?;
                if header_buf[0] == b'\n' {
                    break;
                }
                buffer.push(b'\r');
                buffer.push(header_buf[0]);
            } else {
                buffer.push(header_buf[0]);
            }
        }

        // Empty line marks end of headers
        if buffer.is_empty() {
            break;
        }

        let header = String::from_utf8_lossy(&buffer);
        if let Some(value) = header.strip_prefix("Content-Length: ") {
            content_length = value.trim().parse().ok();
        }
    }

    let content_length = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "Missing Content-Length header")
    })?;

    // Read the JSON content
    let mut content = vec![0u8; content_length];
    reader.read_exact(&mut content).await?;

    serde_json::from_slice(&content)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("Invalid JSON: {}", e)))
}

pub async fn write_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    message: &Value,
) -> io::Result<()> {
    let content = serde_json::to_string(message).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Failed to serialize JSON: {}", e),
        )
    })?;

    let header = format!("Content-Length: {}\r\n\r\n", content.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(content.as_bytes()).await?;
    writer.flush().await?;

    Ok(())
}
