//! Dependency-free local inspector dashboard.
//!
//! The dashboard reads persisted session metadata only. It intentionally does
//! not participate in semantic observation or action execution, so a browser
//! tab can never add latency or authority to the Android control hot path.

use crate::error::{AndroidError, Result};
use crate::model::SessionV1;
use crate::session::SessionStore;
use serde_json::json;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const DASHBOARD_WORKERS: usize = 4;
const DASHBOARD_QUEUE: usize = 16;
const DASHBOARD_MAX_SESSIONS: usize = 64;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

pub fn serve(address: &str) -> Result<()> {
    let listener = TcpListener::bind(address)?;
    let (sender, receiver) = mpsc::sync_channel::<TcpStream>(DASHBOARD_QUEUE);
    let receiver = Arc::new(Mutex::new(receiver));
    for index in 0..DASHBOARD_WORKERS {
        let receiver = Arc::clone(&receiver);
        std::thread::Builder::new()
            .name(format!("tempera-android-dashboard-{index}"))
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
                        "dashboard worker pool unexpectedly stopped".to_string(),
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
    let mut request = [0_u8; 4096];
    let read = stream.read(&mut request)?;
    let request = String::from_utf8_lossy(&request[..read]);
    let path = request
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/");
    let (status, content_type, body): (&str, &str, Vec<u8>) = match path {
        "/api/sessions" => {
            let sessions = recent_sessions(SessionStore::from_environment()?.list()?);
            (
                "200 OK",
                "application/json; charset=utf-8",
                serde_json::to_vec_pretty(&sessions)?,
            )
        }
        "/api/state" => {
            let store = SessionStore::from_environment()?;
            let sessions = recent_sessions(store.list()?);
            let details = sessions
                .iter()
                .map(|session| {
                    Ok(json!({
                        "session": session,
                        "snapshot": store.snapshot(&session.session_id)?,
                        "receipts": store.receipts(&session.session_id)?,
                        "logs": store.diagnostic(&session.session_id, "logs")?,
                        "network": store.diagnostic(&session.session_id, "network")?,
                        "activity": store.diagnostic(&session.session_id, "activity")?,
                        "frameUrl": if store.has_frame(&session.session_id)? { Some(format!("/api/frame/{}", session.session_id)) } else { None::<String> },
                    }))
                })
                .collect::<Result<Vec<_>>>()?;
            (
                "200 OK",
                "application/json; charset=utf-8",
                serde_json::to_vec_pretty(&details)?,
            )
        }
        "/" => (
            "200 OK",
            "text/html; charset=utf-8",
            HTML.as_bytes().to_vec(),
        ),
        value if value.starts_with("/api/frame/") => {
            let id = value.trim_start_matches("/api/frame/");
            let store = SessionStore::from_environment()?;
            if let Some(length) = store.frame_len(id)? {
                write_headers(&mut stream, "200 OK", "image/png", length)?;
                if !store.copy_frame_to(id, &mut stream)? {
                    return Err(AndroidError::Backend(
                        "inspector frame disappeared while it was being served".to_string(),
                    ));
                }
                stream.flush()?;
                return Ok(());
            } else {
                (
                    "404 Not Found",
                    "text/plain; charset=utf-8",
                    b"Frame not found".to_vec(),
                )
            }
        }
        _ => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"Not found".to_vec(),
        ),
    };
    write_headers(&mut stream, status, content_type, body.len() as u64)?;
    stream.write_all(&body)?;
    stream.flush()?;
    Ok(())
}

fn recent_sessions(mut sessions: Vec<SessionV1>) -> Vec<SessionV1> {
    sessions.sort_by(|left, right| {
        right
            .updated_at_ms
            .cmp(&left.updated_at_ms)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    sessions.truncate(DASHBOARD_MAX_SESSIONS);
    sessions
}

fn write_headers(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    length: u64,
) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {length}\r\nCache-Control: no-store\r\n\r\n"
    )?;
    Ok(())
}

fn write_busy(stream: &mut TcpStream) -> Result<()> {
    stream.set_write_timeout(Some(CONNECTION_TIMEOUT))?;
    let body = b"Dashboard is at its concurrent request limit; retry shortly";
    write_headers(
        stream,
        "503 Service Unavailable",
        "text/plain; charset=utf-8",
        body.len() as u64,
    )?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

const HTML: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Tempera Android Inspector</title><style>
:root{color-scheme:dark}*{box-sizing:border-box}body{background:#090f1f;color:#e5e7eb;font:13px ui-monospace,SFMono-Regular,Menlo,monospace;margin:0;padding:22px}h1{font:600 24px system-ui;margin:0 0 6px}.muted{color:#9ca3af;margin:0 0 18px}.layout{display:grid;grid-template-columns:240px minmax(0,1.4fr) minmax(320px,1fr);gap:14px}.card{background:#111a2e;border:1px solid #27354f;border-radius:10px;padding:12px;min-height:110px}.wide{grid-column:span 2}h2{font:600 12px system-ui;letter-spacing:.06em;text-transform:uppercase;color:#9db5dc;margin:0 0 10px}.session,.node{width:100%;border:1px solid #2c3b56;background:#0b1324;color:#dce8ff;padding:8px;text-align:left;border-radius:6px;margin:3px 0;font:inherit;cursor:pointer}.session.active,.node.active{border-color:#6ea8fe;background:#16284b}.node small{display:block;color:#9ca3af;margin-top:3px}pre{white-space:pre-wrap;word-break:break-word;max-height:310px;overflow:auto;margin:0}img{display:block;max-width:100%;max-height:480px;object-fit:contain;background:#020617;border-radius:6px}.empty{color:#7f8da3;padding:12px 0}.pill{display:inline-block;border:1px solid #35517d;border-radius:999px;padding:2px 6px;margin-left:5px;color:#b9d6ff}@media(max-width:1000px){.layout{grid-template-columns:1fr}.wide{grid-column:auto}}</style></head><body>
<h1>Tempera Android Inspector</h1><p class="muted">Read-only persisted state. Refreshing this page never observes, screenshots, or controls a device.</p>
<div class="layout"><section class="card"><h2>Sessions / devices</h2><div id="sessions"></div></section><section class="card"><h2>Last captured frame</h2><div id="frame" class="empty">No screenshot captured for this session.</div></section><section class="card"><h2>Selected semantic node</h2><pre id="selected" class="empty">Select a node in the semantic tree.</pre></section><section class="card wide"><h2>Semantic tree <span id="revision" class="pill"></span></h2><div id="tree" class="empty">No semantic snapshot captured.</div></section><section class="card"><h2>Action receipts</h2><pre id="receipts" class="empty">No receipts.</pre></section><section class="card"><h2>Logcat (last explicit read)</h2><pre id="logs" class="empty">No logs captured.</pre></section><section class="card"><h2>Network (last explicit read)</h2><pre id="network" class="empty">No network diagnostic captured.</pre></section><section class="card wide"><h2>Model / eval activity</h2><pre id="activity" class="empty">No model or evaluation activity.</pre></section></div>
<script>let state=[],sessionId=null,nodeRef=null;const by=id=>document.getElementById(id),pretty=x=>JSON.stringify(x,null,2),empty=(id,msg)=>{by(id).textContent=msg;by(id).className='empty'},button=(className,title,detail,click)=>{const b=document.createElement('button'),s=document.createElement('small');b.className=className;b.append(document.createTextNode(title));s.textContent=detail;b.append(s);b.onclick=click;return b};function render(){const current=state.find(x=>x.session.sessionId===sessionId)||state[0];if(!current){empty('sessions','No sessions. Run snapshot or connect a target.');return}sessionId=current.session.sessionId;const sessions=by('sessions');sessions.replaceChildren(...state.map(x=>button(`session ${x.session.sessionId===sessionId?'active':''}`,x.session.sessionId,`${x.session.serial} · ${x.session.transport}`,()=>{sessionId=x.session.sessionId;nodeRef=null;render()})));const snapshot=current.snapshot;by('revision').textContent=snapshot?`r${snapshot.revision} · ${snapshot.stateHash.slice(0,16)}`:'no snapshot';const nodes=snapshot?.nodes||[],tree=by('tree');if(nodes.length){tree.className='';tree.replaceChildren(...nodes.map(n=>button(`node ${n.reference===nodeRef?'active':''}`,`${n.reference} ${n.role}: ${n.label||'(unlabeled)'}`,`${n.resourceId||''} · ${n.bounds.left},${n.bounds.top}–${n.bounds.right},${n.bounds.bottom}`,()=>{nodeRef=n.reference;render()})))}else{tree.className='empty';tree.textContent='No semantic snapshot captured.'}const node=nodes.find(n=>n.reference===nodeRef);if(node){by('selected').className='';by('selected').textContent=pretty(node)}else empty('selected','Select a node in the semantic tree.');by('receipts').className='';by('receipts').textContent=current.receipts?.length?pretty(current.receipts):'No receipts.';by('logs').className='';by('logs').textContent=current.logs?pretty(current.logs):'No logs captured.';by('network').className='';by('network').textContent=current.network?pretty(current.network):'No network diagnostic captured.';by('activity').className='';by('activity').textContent=current.activity?pretty(current.activity):'No model or evaluation activity.';const frame=by('frame');if(current.frameUrl){const image=document.createElement('img');image.alt='Last captured Android frame';image.src=`${current.frameUrl}?t=${Date.now()}`;frame.replaceChildren(image)}else{frame.className='empty';frame.textContent='No screenshot captured for this session.'}}async function refresh(){try{const response=await fetch('/api/state',{cache:'no-store'});state=await response.json();render()}catch(error){empty('sessions',`Dashboard read failed: ${error}`)}}refresh();setInterval(refresh,1500)</script></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CONTROL_SCHEMA_V1;

    fn session(id: &str, updated_at_ms: u128) -> SessionV1 {
        SessionV1 {
            schema_version: CONTROL_SCHEMA_V1.to_string(),
            session_id: id.to_string(),
            serial: "emulator-5554".to_string(),
            target_kind: "emulator".to_string(),
            transport: "adb".to_string(),
            created_at_ms: 1,
            updated_at_ms,
            last_revision: 0,
            last_state_hash: None,
            backend_session_id: None,
        }
    }

    #[test]
    fn dashboard_keeps_only_the_most_recent_sessions() {
        let sessions = (0..(DASHBOARD_MAX_SESSIONS + 2))
            .map(|index| session(&format!("s{index:03}"), index as u128))
            .collect();
        let recent = recent_sessions(sessions);
        assert_eq!(recent.len(), DASHBOARD_MAX_SESSIONS);
        assert_eq!(recent[0].session_id, "s065");
        assert_eq!(recent.last().unwrap().session_id, "s002");
    }
}
