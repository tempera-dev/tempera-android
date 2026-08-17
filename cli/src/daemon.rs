//! Local JSONL daemon for long-lived sessions and dashboard/MCP clients.

use crate::command::{execute, CommandRequest, CommandResponse};
use crate::error::{AndroidError, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

pub fn serve(address: &str) -> Result<()> {
    let listener = TcpListener::bind(address)?;
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                std::thread::spawn(move || {
                    let _ = handle(stream);
                });
            }
            Err(error) => return Err(AndroidError::Io(error)),
        }
    }
    Ok(())
}

fn handle(mut stream: TcpStream) -> Result<()> {
    let reader = BufReader::new(stream.try_clone()?);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<CommandRequest>(&line) {
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
    Ok(())
}
