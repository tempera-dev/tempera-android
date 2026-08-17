//! Dependency-free local inspector dashboard.
//!
//! The dashboard reads persisted session metadata only. It intentionally does
//! not participate in semantic observation or action execution, so a browser
//! tab can never add latency or authority to the Android control hot path.

use crate::error::{AndroidError, Result};
use crate::session::SessionStore;
use serde_json::json;
use std::io::{Read, Write};
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
    let mut request = [0_u8; 4096];
    let read = stream.read(&mut request)?;
    let request = String::from_utf8_lossy(&request[..read]);
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let (content_type, body) = match path {
        "/api/sessions" => (
            "application/json; charset=utf-8",
            serde_json::to_string_pretty(&SessionStore::from_environment()?.list()?)?,
        ),
        "/api/state" => {
            let store = SessionStore::from_environment()?;
            let sessions = store.list()?;
            let details = sessions
                .iter()
                .map(|session| {
                    Ok(json!({
                        "session": session,
                        "snapshot": store.snapshot(&session.session_id)?,
                        "receipts": store.receipts(&session.session_id)?,
                    }))
                })
                .collect::<Result<Vec<_>>>()?;
            (
                "application/json; charset=utf-8",
                serde_json::to_string_pretty(&details)?,
            )
        }
        "/" => ("text/html; charset=utf-8", HTML.to_string()),
        _ => ("text/plain; charset=utf-8", "Not found".to_string()),
    };
    let status = if path == "/" || path == "/api/sessions" || path == "/api/state" {
        "200 OK"
    } else {
        "404 Not Found"
    };
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\r\n{}", body.len(), body)?;
    stream.flush()?;
    Ok(())
}

const HTML: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Tempera Android Inspector</title><style>body{background:#111827;color:#e5e7eb;font:14px ui-monospace,SFMono-Regular,Menlo,monospace;margin:0;padding:28px}h1{font:600 24px system-ui;margin:0 0 8px}.muted{color:#9ca3af}.grid{display:grid;grid-template-columns:1fr 2fr;gap:16px}pre{background:#030712;border:1px solid #374151;border-radius:8px;padding:16px;overflow:auto;max-height:70vh}</style></head><body><h1>Tempera Android Inspector</h1><p class="muted">Read-only dashboard: session/device state, current semantic tree, and action receipts. Streaming never sits on the control path.</p><div class="grid"><pre id="sessions">Loading sessions…</pre><pre id="state">Loading state…</pre></div><script>async function refresh(){const [a,b]=await Promise.all([fetch('/api/sessions',{cache:'no-store'}),fetch('/api/state',{cache:'no-store'})]);sessions.textContent=JSON.stringify(await a.json(),null,2);state.textContent=JSON.stringify(await b.json(),null,2)}refresh();setInterval(refresh,1500)</script></body></html>"#;
