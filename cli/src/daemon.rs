//! Local JSONL daemon for long-lived sessions and dashboard/MCP clients.

use crate::command::{execute, CommandRequest, CommandResponse};
use crate::error::{AndroidError, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const DAEMON_WORKERS: usize = 8;
const DAEMON_QUEUE: usize = 32;
const MAX_DAEMON_LINE_BYTES: usize = 1024 * 1024;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);

pub fn serve(address: &str) -> Result<()> {
    let listener = TcpListener::bind(address)?;
    let (sender, receiver) = mpsc::sync_channel(DAEMON_QUEUE);
    let receiver = Arc::new(Mutex::new(receiver));
    for index in 0..DAEMON_WORKERS {
        let receiver = Arc::clone(&receiver);
        std::thread::Builder::new()
            .name(format!("tempera-android-daemon-{index}"))
            .spawn(move || loop {
                let stream = match receiver.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => return,
                };
                match stream {
                    Ok(stream) => {
                        let _ = handle(stream);
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

fn handle(mut stream: TcpStream) -> Result<()> {
    stream.set_read_timeout(Some(CONNECTION_TIMEOUT))?;
    stream.set_write_timeout(Some(CONNECTION_TIMEOUT))?;
    let reader = BufReader::new(stream.try_clone()?);
    let mut reader = reader;
    let mut line = Vec::with_capacity(4096);
    while read_bounded_line(&mut reader, &mut line)? {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let response = match std::str::from_utf8(&line)
            .ok()
            .and_then(|line| serde_json::from_str::<CommandRequest>(line).ok())
        {
            Some(request) => execute(request),
            None => CommandResponse::failure(
                "unknown".to_string(),
                "Invalid daemon request: expected a UTF-8 CommandRequest JSON object".to_string(),
            ),
        };
        serde_json::to_writer(&mut stream, &response)?;
        stream.write_all(b"\n")?;
        stream.flush()?;
    }
    Ok(())
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
            if line.len() + copied > MAX_DAEMON_LINE_BYTES {
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
    serde_json::to_writer(&mut *stream, &response)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

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
}
