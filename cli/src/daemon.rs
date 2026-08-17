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
    let mut line = Vec::with_capacity(8 * 1024);
    loop {
        line.clear();
        let bytes = reader.read_until(b'\n', &mut line)?;
        if bytes == 0 {
            return Ok(());
        }
        if line.len() > MAX_REQUEST_BYTES {
            let response = CommandResponse::failure(
                "unknown".to_string(),
                format!("Daemon request exceeds {MAX_REQUEST_BYTES} bytes"),
            );
            serde_json::to_writer(&mut stream, &response)?;
            stream.write_all(b"\n")?;
            stream.flush()?;
            return Ok(());
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.iter().all(u8::is_ascii_whitespace) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_loopback_bind_is_rejected_before_listening() {
        let error = serve("0.0.0.0:0").expect_err("non-loopback bind must fail");
        assert!(error.to_string().contains("loopback"));
    }
}
