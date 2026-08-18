//! Authenticated local JSONL daemon for long-lived sessions and dashboard/MCP/Tempera Use clients.

use crate::command::{execute, CommandResponse};
use crate::daemon_auth::DaemonAuthority;
use crate::error::{AndroidError, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const DAEMON_WORKERS: usize = 8;
const DAEMON_QUEUE: usize = 32;
const MAX_DAEMON_LINE_BYTES: usize = 256 * 1024;
const MAX_REJECTED_FRAMES_PER_CONNECTION: usize = 3;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);

pub fn serve(address: &str) -> Result<()> {
    let address: SocketAddr = address
        .parse()
        .map_err(|error| AndroidError::InvalidInput(format!("invalid daemon address: {error}")))?;
    if !address.ip().is_loopback() {
        return Err(AndroidError::InvalidInput(
            "the Android control daemon is local-only and must bind to loopback".to_string(),
        ));
    }
    let authority = Arc::new(DaemonAuthority::from_environment()?);
    let listener = TcpListener::bind(address)?;
    let (sender, receiver) = mpsc::sync_channel(DAEMON_QUEUE);
    let receiver = Arc::new(Mutex::new(receiver));
    for index in 0..DAEMON_WORKERS {
        let receiver = Arc::clone(&receiver);
        let authority = Arc::clone(&authority);
        std::thread::Builder::new()
            .name(format!("tempera-android-daemon-{index}"))
            .spawn(move || loop {
                let stream = match receiver.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => return,
                };
                match stream {
                    Ok(stream) => {
                        let _ = handle(stream, authority.as_ref());
                    }
                    Err(_) => return,
                }
            })
            .map_err(AndroidError::Io)?;
    }
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => match sender.try_send(stream) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(mut stream)) => {
                    let _ = write_busy(&mut stream);
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return Err(AndroidError::Backend(
                        "daemon worker pool unexpectedly stopped".to_string(),
                    ));
                }
            },
            Err(error) => return Err(AndroidError::Io(error)),
        }
    }
    Ok(())
}

fn handle(mut stream: TcpStream, authority: &DaemonAuthority) -> Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(CONNECTION_TIMEOUT))?;
    stream.set_write_timeout(Some(CONNECTION_TIMEOUT))?;
    let reader = BufReader::new(stream.try_clone()?);
    let mut reader = reader;
    let mut line = Vec::with_capacity(4096);
    let mut rejected_frames = 0_usize;
    loop {
        let has_line = match read_bounded_line(&mut reader, &mut line) {
            Ok(has_line) => has_line,
            Err(AndroidError::InvalidInput(message)) => {
                // Framing violations happen before command dispatch. Return one
                // bounded rejection and close so no effect can be ambiguous.
                write_response(
                    &mut stream,
                    &CommandResponse::failure("unknown".to_string(), message),
                )?;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if !has_line {
            return Ok(());
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let request = match authority.authenticate_frame(&line) {
            Ok(request) => request,
            Err(_) => {
                rejected_frames = rejected_frames.saturating_add(1);
                // Do not reveal whether the token, session, device, transport,
                // or command scope failed. A small cumulative budget preserves
                // persistent-client recovery from one malformed frame without
                // creating an unbounded token-guessing channel.
                write_response(
                    &mut stream,
                    &CommandResponse::failure(
                        "unknown".to_string(),
                        "Android daemon request rejected".to_string(),
                    ),
                )?;
                if rejected_frames >= MAX_REJECTED_FRAMES_PER_CONNECTION {
                    return Ok(());
                }
                continue;
            }
        };
        write_response(&mut stream, &execute(request))?;
    }
}

fn read_bounded_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> Result<bool> {
    line.clear();
    loop {
        let (consume, complete) = {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                return Ok(!line.is_empty());
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let consume = newline.map_or(available.len(), |index| index + 1);
            let copied = newline.unwrap_or(available.len());
            if line.len().saturating_add(copied) > MAX_DAEMON_LINE_BYTES {
                return Err(AndroidError::InvalidInput(format!(
                    "daemon request exceeds the {MAX_DAEMON_LINE_BYTES}-byte limit"
                )));
            }
            line.extend_from_slice(&available[..copied]);
            (consume, newline.is_some())
        };
        reader.consume(consume);
        if complete {
            return Ok(true);
        }
    }
}

fn write_busy(stream: &mut TcpStream) -> Result<()> {
    stream.set_write_timeout(Some(CONNECTION_TIMEOUT))?;
    let response = CommandResponse::failure(
        "unknown".to_string(),
        "daemon is at its concurrent request limit; retry shortly".to_string(),
    );
    write_response(stream, &response)?;
    Ok(())
}

fn write_response(stream: &mut TcpStream, response: &CommandResponse) -> Result<()> {
    serde_json::to_writer(&mut *stream, response)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_auth::DaemonScope;
    use serde_json::json;
    use std::io::{Cursor, Read};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn rejected_frame() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "capabilityToken": "fedcba9876543210fedcba9876543210",
            "id": "request-1",
            "sessionId": "session-1",
            "transport": "auto",
            "command": {"name": "state"},
        }))
        .unwrap_or_else(|error| panic!("frame encoding: {error}"))
    }

    #[test]
    fn daemon_lines_are_incremental_and_bounded() {
        let mut reader = Cursor::new(b"first\nsecond\n".as_slice());
        let mut line = Vec::new();
        assert!(read_bounded_line(&mut reader, &mut line).unwrap());
        assert_eq!(line, b"first");
        assert!(read_bounded_line(&mut reader, &mut line).unwrap());
        assert_eq!(line, b"second");
        assert!(!read_bounded_line(&mut reader, &mut line).unwrap());
    }

    #[test]
    fn daemon_rejects_oversized_lines_before_unbounded_growth() {
        let oversized = vec![b'x'; MAX_DAEMON_LINE_BYTES + 1];
        let mut reader = Cursor::new(oversized);
        let mut line = Vec::new();
        assert!(read_bounded_line(&mut reader, &mut line).is_err());
        assert!(line.len() <= MAX_DAEMON_LINE_BYTES);
    }

    #[test]
    fn daemon_rejects_non_loopback_binds_before_reading_authority() {
        let error = serve("0.0.0.0:0").unwrap_err();
        assert!(error.to_string().contains("loopback"));
    }

    #[test]
    fn rejected_frames_are_generic_persistent_and_bounded() -> Result<()> {
        let authority = Arc::new(DaemonAuthority::for_test(
            TOKEN,
            DaemonScope::TemperaUse,
            Some("session-1"),
            None,
        )?);
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let server_authority = Arc::clone(&authority);
        let server = std::thread::spawn(move || -> Result<()> {
            let (stream, _) = listener.accept()?;
            handle(stream, server_authority.as_ref())
        });

        let mut client = TcpStream::connect(address)?;
        client.set_read_timeout(Some(CONNECTION_TIMEOUT))?;
        let frame = rejected_frame();
        for _ in 0..MAX_REJECTED_FRAMES_PER_CONNECTION {
            client.write_all(&frame)?;
            client.write_all(b"\n")?;
        }
        client.flush()?;

        let mut reader = BufReader::new(client);
        for _ in 0..MAX_REJECTED_FRAMES_PER_CONNECTION {
            let mut line = Vec::new();
            assert!(read_bounded_line(&mut reader, &mut line)?);
            let response: CommandResponse = serde_json::from_slice(&line)?;
            assert!(!response.ok);
            assert_eq!(response.id, "unknown");
            assert_eq!(
                response.error.as_deref(),
                Some("Android daemon request rejected")
            );
        }
        let mut trailing = [0_u8; 1];
        assert_eq!(reader.read(&mut trailing)?, 0);
        server
            .join()
            .map_err(|_| AndroidError::Backend("daemon test server panicked".to_string()))??;
        Ok(())
    }
}
