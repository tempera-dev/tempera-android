//! Local JSONL daemon for long-lived sessions and dashboard/MCP/Tempera Use clients.

use crate::command::{execute, CommandRequest, CommandResponse};
use crate::error::{AndroidError, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const MAX_REQUEST_BYTES: usize = 256 * 1024;
const MAX_CONNECTIONS: usize = 64;

pub fn serve(address: &str) -> Result<()> {
    let address: SocketAddr = address
        .parse()
        .map_err(|error| AndroidError::InvalidInput(format!("Invalid daemon address: {error}")))?;
    if !address.ip().is_loopback() {
        return Err(AndroidError::InvalidInput(
            "The Android control daemon is local-only and must bind to loopback".to_string(),
        ));
    }

    let listener = TcpListener::bind(address)?;
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => return Err(AndroidError::Io(error)),
        };
        if active.load(Ordering::Acquire) >= MAX_CONNECTIONS {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            continue;
        }
        active.fetch_add(1, Ordering::AcqRel);
        let active = Arc::clone(&active);
        std::thread::spawn(move || {
            let _guard = ConnectionGuard(active);
            let _ = handle(stream);
        });
    }
    Ok(())
}

struct ConnectionGuard(Arc<AtomicUsize>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn handle(mut stream: TcpStream) -> Result<()> {
    stream.set_nodelay(true)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    loop {
        let line = match read_bounded_line(&mut reader, MAX_REQUEST_BYTES)? {
            Some(line) => line,
            None => return Ok(()),
        };
        if line.iter().all(|byte| byte.is_ascii_whitespace()) {
            continue;
        }
        let response = match serde_json::from_slice::<CommandRequest>(&line) {
            Ok(request) => execute(request),
            Err(error) => CommandResponse::failure(
                "unknown".to_string(),
                format!("Invalid daemon request: {error}"),
            ),
        };
        serde_json::to_writer(&mut stream, &response)?;
        stream.write_all(b"\n")?;
        stream.flush()?;
    }
}

/// Read one newline-delimited frame while enforcing the cap before copying
/// bytes into the output buffer. `BufRead::read_until` would allocate an entire
/// attacker-controlled line and only let us reject it afterward.
fn read_bounded_line<R: BufRead>(reader: &mut R, max_bytes: usize) -> Result<Option<Vec<u8>>> {
    let mut output = Vec::with_capacity(8 * 1024);
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if output.is_empty() { Ok(None) } else { Ok(Some(output)) };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.unwrap_or(available.len());
        if output.len().saturating_add(take) > max_bytes {
            return Err(AndroidError::InvalidInput(format!(
                "Daemon request exceeds {max_bytes} bytes"
            )));
        }
        output.extend_from_slice(&available[..take]);
        reader.consume(take + usize::from(newline.is_some()));
        if newline.is_some() {
            if output.last() == Some(&b'\r') {
                output.pop();
            }
            return Ok(Some(output));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn non_loopback_bind_is_rejected_before_listening() {
        let error = serve("0.0.0.0:0").expect_err("non-loopback bind must fail");
        assert!(error.to_string().contains("loopback"));
    }

    #[test]
    fn bounded_reader_rejects_before_collecting_oversized_frame() {
        let mut reader = BufReader::with_capacity(4, Cursor::new(b"0123456789\n"));
        let error = read_bounded_line(&mut reader, 4)
            .expect_err("oversized frame must fail");
        assert!(error.to_string().contains("exceeds 4 bytes"));
    }

    #[test]
    fn bounded_reader_accepts_crlf_and_eof() -> Result<()> {
        let mut reader = BufReader::new(Cursor::new(b"{}\r\n{}"));
        assert_eq!(read_bounded_line(&mut reader, 16)?.as_deref(), Some(b"{}".as_slice()));
        assert_eq!(read_bounded_line(&mut reader, 16)?.as_deref(), Some(b"{}".as_slice()));
        assert!(read_bounded_line(&mut reader, 16)?.is_none());
        Ok(())
    }
}
