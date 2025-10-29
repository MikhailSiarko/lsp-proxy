use serde_json::Value;
use std::io;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

pub async fn read_message<R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<Value> {
    let mut buffer = BufReader::new(reader);
    let mut header_buf = Vec::new();
    let mut content_length: Option<usize> = None;

    header_buf.clear();
    let bytes_len = buffer.read_until(b'\n', &mut header_buf).await?;
    if bytes_len == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Unexpected EOF while reading headers",
        ));
    }

    let header = String::from_utf8_lossy(&header_buf);
    if let Some(value) = header.strip_prefix("Content-Length: ") {
        content_length = value.trim().parse().ok();
    }

    let content_length = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "Missing Content-Length header")
    })?;

    let bytes_len = buffer.read_exact(&mut [0u8; 2]).await?;
    if bytes_len == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Unexpected EOF while reading headers",
        ));
    }

    let mut content_buf = vec![0u8; content_length];
    buffer.read_exact(&mut content_buf).await?;

    serde_json::from_slice(&content_buf)
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
