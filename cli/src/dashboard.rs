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
    let path = request
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/");
    let (status, content_type, body): (&str, &str, Vec<u8>) = match path {
        "/api/sessions" => (
            "200 OK",
            "application/json; charset=utf-8",
            serde_json::to_vec_pretty(&SessionStore::from_environment()?.list()?)?,
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
            match SessionStore::from_environment()?.frame(id)? {
                Some(frame) => ("200 OK", "image/png", frame),
                None => (
                    "404 Not Found",
                    "text/plain; charset=utf-8",
                    b"Frame not found".to_vec(),
                ),
            }
        }
        _ => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"Not found".to_vec(),
        ),
    };
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\r\n", body.len())?;
    stream.write_all(&body)?;
    stream.flush()?;
    Ok(())
}

const HTML: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Tempera Android Inspector</title><style>
:root{color-scheme:dark}*{box-sizing:border-box}body{background:#090f1f;color:#e5e7eb;font:13px ui-monospace,SFMono-Regular,Menlo,monospace;margin:0;padding:22px}h1{font:600 24px system-ui;margin:0 0 6px}.muted{color:#9ca3af;margin:0 0 18px}.layout{display:grid;grid-template-columns:240px minmax(0,1.4fr) minmax(320px,1fr);gap:14px}.card{background:#111a2e;border:1px solid #27354f;border-radius:10px;padding:12px;min-height:110px}.wide{grid-column:span 2}h2{font:600 12px system-ui;letter-spacing:.06em;text-transform:uppercase;color:#9db5dc;margin:0 0 10px}.session,.node{width:100%;border:1px solid #2c3b56;background:#0b1324;color:#dce8ff;padding:8px;text-align:left;border-radius:6px;margin:3px 0;font:inherit;cursor:pointer}.session.active,.node.active{border-color:#6ea8fe;background:#16284b}.node small{display:block;color:#9ca3af;margin-top:3px}pre{white-space:pre-wrap;word-break:break-word;max-height:310px;overflow:auto;margin:0}img{display:block;max-width:100%;max-height:480px;object-fit:contain;background:#020617;border-radius:6px}.empty{color:#7f8da3;padding:12px 0}.pill{display:inline-block;border:1px solid #35517d;border-radius:999px;padding:2px 6px;margin-left:5px;color:#b9d6ff}@media(max-width:1000px){.layout{grid-template-columns:1fr}.wide{grid-column:auto}}</style></head><body>
<h1>Tempera Android Inspector</h1><p class="muted">Read-only persisted state. Refreshing this page never observes, screenshots, or controls a device.</p>
<div class="layout"><section class="card"><h2>Sessions / devices</h2><div id="sessions"></div></section><section class="card"><h2>Last captured frame</h2><div id="frame" class="empty">No screenshot captured for this session.</div></section><section class="card"><h2>Selected semantic node</h2><pre id="selected" class="empty">Select a node in the semantic tree.</pre></section><section class="card wide"><h2>Semantic tree <span id="revision" class="pill"></span></h2><div id="tree" class="empty">No semantic snapshot captured.</div></section><section class="card"><h2>Action receipts</h2><pre id="receipts" class="empty">No receipts.</pre></section><section class="card"><h2>Logcat (last explicit read)</h2><pre id="logs" class="empty">No logs captured.</pre></section><section class="card"><h2>Network (last explicit read)</h2><pre id="network" class="empty">No network diagnostic captured.</pre></section><section class="card wide"><h2>Model / eval activity</h2><pre id="activity" class="empty">No model or evaluation activity.</pre></section></div>
<script>let state=[],sessionId=null,nodeRef=null;const by=id=>document.getElementById(id),pretty=x=>JSON.stringify(x,null,2),empty=(id,msg)=>{by(id).textContent=msg;by(id).className='empty'},button=(className,title,detail,click)=>{const b=document.createElement('button'),s=document.createElement('small');b.className=className;b.append(document.createTextNode(title));s.textContent=detail;b.append(s);b.onclick=click;return b};function render(){const current=state.find(x=>x.session.sessionId===sessionId)||state[0];if(!current){empty('sessions','No sessions. Run snapshot or connect a target.');return}sessionId=current.session.sessionId;const sessions=by('sessions');sessions.replaceChildren(...state.map(x=>button(`session ${x.session.sessionId===sessionId?'active':''}`,x.session.sessionId,`${x.session.serial} · ${x.session.transport}`,()=>{sessionId=x.session.sessionId;nodeRef=null;render()})));const snapshot=current.snapshot;by('revision').textContent=snapshot?`r${snapshot.revision} · ${snapshot.stateHash.slice(0,16)}`:'no snapshot';const nodes=snapshot?.nodes||[],tree=by('tree');if(nodes.length){tree.className='';tree.replaceChildren(...nodes.map(n=>button(`node ${n.reference===nodeRef?'active':''}`,`${n.reference} ${n.role}: ${n.label||'(unlabeled)'}`,`${n.resourceId||''} · ${n.bounds.left},${n.bounds.top}–${n.bounds.right},${n.bounds.bottom}`,()=>{nodeRef=n.reference;render()})))}else{tree.className='empty';tree.textContent='No semantic snapshot captured.'}const node=nodes.find(n=>n.reference===nodeRef);if(node){by('selected').className='';by('selected').textContent=pretty(node)}else empty('selected','Select a node in the semantic tree.');by('receipts').className='';by('receipts').textContent=current.receipts?.length?pretty(current.receipts):'No receipts.';by('logs').className='';by('logs').textContent=current.logs?pretty(current.logs):'No logs captured.';by('network').className='';by('network').textContent=current.network?pretty(current.network):'No network diagnostic captured.';by('activity').className='';by('activity').textContent=current.activity?pretty(current.activity):'No model or evaluation activity.';const frame=by('frame');if(current.frameUrl){const image=document.createElement('img');image.alt='Last captured Android frame';image.src=`${current.frameUrl}?t=${Date.now()}`;frame.replaceChildren(image)}else{frame.className='empty';frame.textContent='No screenshot captured for this session.'}}async function refresh(){try{const response=await fetch('/api/state',{cache:'no-store'});state=await response.json();render()}catch(error){empty('sessions',`Dashboard read failed: ${error}`)}}refresh();setInterval(refresh,1500)</script></body></html>"#;
