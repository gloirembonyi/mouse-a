// Kudu Desktop - Browser bridge (WebSocket server).
//
// The desktop app is the ORCHESTRATOR. When an agent task needs something done
// on the web - open a page, fill a form, read a site - it can't do that itself
// reliably (screenshot+coordinate automation is fragile in a browser). Instead
// it delegates that piece to the Kudu Connect extension over a local
// WebSocket the desktop hosts here. The extension runs its DOM-level browser
// agent, streams progress back (shown inline in the desktop's own step log),
// and returns the final result - so the desktop and browser act as one system.
//
// Local-only + origin-gated: binds 127.0.0.1 and accepts only chrome-extension
// / moz-extension origins, so a random web page can't drive the desktop app.
//
// Two modes, exposed as agent tools (see agent.rs):
//   • run_task  - delegate and await the result inline.
//   • start_task + await_task - delegate, keep working, collect the result later
//     (true parallelism: the desktop agent continues while the browser works).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;

/// Uncommon ports, tried in order. The first is the one a lone desktop always gets;
/// the rest exist for a PC shared by several Windows/macOS users, where each user's
/// Kudu Desktop needs its own listener. The extension tries them in the same order
/// and keeps the one whose desktop is signed in to ITS account (see `link_proof`),
/// so users on one machine never end up driving each other's apps.
const PORTS: [u16; 5] = [47821, 47822, 47823, 47824, 47825];
const DEFAULT_TASK_TIMEOUT_SECS: u64 = 600;

#[derive(Clone)]
pub struct Bridge {
    inner: Arc<Inner>,
}

struct Inner {
    /// Every open extension connection (Chrome and Edge can both be linked at once).
    conns: Mutex<HashMap<u64, Conn>>,
    /// The connection frames go to by default: the browser that most recently paired
    /// or asked us for something. 0 = none.
    active: AtomicU64,
    conn_seq: AtomicU64,
    port: AtomicU16,
    pending: Mutex<HashMap<String, PendingTask>>,
    // Result receivers for delegated tasks, kept so await_task can collect them
    // after start_task returned (enables the desktop to work in parallel).
    results: Mutex<HashMap<String, oneshot::Receiver<Result<String, String>>>>,
    task_seq: AtomicU16,
}

struct Conn {
    tx: mpsc::UnboundedSender<Message>,
    /// How this browser earned trust: "account" (signed in to the same Kudu account,
    /// checked by Kudu Core) or "code" (the pairing code). None = connected only.
    paired_via: Option<&'static str>,
    /// Random challenge for account pairing; a proof is only valid for this nonce.
    nonce: String,
}

struct PendingTask {
    /// The connection the task went out on - its disconnect fails only its tasks.
    conn: u64,
    desktop_source: String,
    result_tx: Option<oneshot::Sender<Result<String, String>>>,
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

pub fn bridge() -> Bridge {
    BRIDGE
        .get_or_init(|| Bridge {
            inner: Arc::new(Inner {
                conns: Mutex::new(HashMap::new()),
                active: AtomicU64::new(0),
                conn_seq: AtomicU64::new(1),
                port: AtomicU16::new(0),
                pending: Mutex::new(HashMap::new()),
                results: Mutex::new(HashMap::new()),
                task_seq: AtomicU16::new(1),
            }),
        })
        .clone()
}

impl Bridge {
    pub fn is_connected(&self) -> bool {
        !self.inner.conns.lock().unwrap().is_empty()
    }

    pub fn port(&self) -> u16 {
        self.inner.port.load(Ordering::SeqCst)
    }

    pub fn is_paired(&self) -> bool {
        let active = self.inner.active.load(Ordering::SeqCst);
        self.conn_paired(active)
    }

    fn conn_paired(&self, conn: u64) -> bool {
        self.inner.conns.lock().unwrap().get(&conn).map_or(false, |c| c.paired_via.is_some())
    }

    /// How the active browser is linked ("account" | "code"), for the Settings card.
    pub fn paired_via(&self) -> Option<&'static str> {
        let active = self.inner.active.load(Ordering::SeqCst);
        self.inner.conns.lock().unwrap().get(&active).and_then(|c| c.paired_via)
    }

    /// Browsers connected right now, and how many of them are paired.
    pub fn counts(&self) -> (usize, usize) {
        let conns = self.inner.conns.lock().unwrap();
        (conns.len(), conns.values().filter(|c| c.paired_via.is_some()).count())
    }

    /// A new pairing code: browsers that were trusted through the OLD code must
    /// enter the new one. Account-paired browsers stay paired - the code was never
    /// what vouched for them.
    pub fn unpair(&self) {
        for c in self.inner.conns.lock().unwrap().values_mut() {
            if c.paired_via == Some("code") {
                c.paired_via = None;
            }
        }
    }

    /// Make `conn` the default destination (it just paired or asked for something).
    fn set_active(&self, conn: u64) {
        self.inner.active.store(conn, Ordering::SeqCst);
    }

    /// After the active browser left: prefer another paired one, then any.
    fn reselect_active(&self) {
        let conns = self.inner.conns.lock().unwrap();
        let next = conns
            .iter()
            .filter(|(_, c)| c.paired_via.is_some())
            .map(|(id, _)| *id)
            .max()
            .or_else(|| conns.keys().copied().max())
            .unwrap_or(0);
        self.inner.active.store(next, Ordering::SeqCst);
    }

    fn send_to(&self, conn: u64, value: Value) -> bool {
        let conns = self.inner.conns.lock().unwrap();
        conns.get(&conn).map_or(false, |c| c.tx.send(Message::Text(value.to_string())).is_ok())
    }

    pub fn send_json(&self, value: Value) -> bool {
        self.send_frame(value)
    }

    /// Resolve a delegated task as failed (watcher give-up, lost task, cancel).
    pub fn fail_task(&self, id: &str, error: &str) {
        let task = self.inner.pending.lock().unwrap().remove(id);
        if let Some(mut task) = task {
            if let Some(txr) = task.result_tx.take() {
                let _ = txr.send(Err(error.to_string()));
            }
        }
        crate::link::task_finished(id, Err(error));
    }

    fn send_frame(&self, value: Value) -> bool {
        let active = self.inner.active.load(Ordering::SeqCst);
        self.send_to(active, value)
    }

    fn next_task_id(&self) -> String {
        let n = self.inner.task_seq.fetch_add(1, Ordering::SeqCst);
        format!("t{}-{}", n, uuid::Uuid::new_v4().simple())
    }

    /// Delegate a web task to the extension and return its task id immediately.
    /// The result is collected later via `await_task` - so the caller can keep
    /// working while the browser does its part.
    pub fn start_task(&self, source: &str, instruction: &str, options: Value) -> Result<String, String> {
        if !self.is_connected() {
            return Err(
                "No browser extension is connected. Open a Chromium browser (Chrome/Edge/Comet) \
with the Kudu Connect extension installed and signed in, then try again."
                    .to_string(),
            );
        }
        if !self.is_paired() {
            return Err(
                "The browser is connected but not linked to this Kudu Desktop. Sign in to the \
extension with the same account as Kudu Desktop (it links automatically), or enter the pairing code \
from Agents → Browser link in the extension's settings."
                    .to_string(),
            );
        }
        let conn = self.inner.active.load(Ordering::SeqCst);
        let id = self.next_task_id();
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().unwrap().insert(
            id.clone(),
            PendingTask { conn, desktop_source: source.to_string(), result_tx: Some(tx) },
        );
        self.inner.results.lock().unwrap().insert(id.clone(), rx);

        let sent = self.send_frame(json!({
            "v": 1,
            "type": "delegate",
            "id": id,
            "task": instruction,
            "options": options,
        }));
        if !sent {
            self.inner.pending.lock().unwrap().remove(&id);
            self.inner.results.lock().unwrap().remove(&id);
            return Err("Failed to send the task to the browser extension (link dropped).".to_string());
        }
        crate::link::task_started(&id, "to_browser", instruction, "running");
        Ok(id)
    }

    /// Await a previously started task's result (with a timeout).
    pub async fn await_task(&self, id: &str, timeout_secs: u64) -> Result<String, String> {
        let rx = self.inner.results.lock().unwrap().remove(id);
        let Some(rx) = rx else {
            return Err(format!("No pending browser task with id \"{}\".", id));
        };
        match tokio::time::timeout(Duration::from_secs(timeout_secs.max(5)), rx).await {
            Ok(Ok(res)) => {
                match &res {
                    Ok(r) => crate::link::task_finished(id, Ok(r)),
                    Err(e) => crate::link::task_finished(id, Err(e)),
                }
                res
            }
            Ok(Err(_)) => {
                self.inner.pending.lock().unwrap().remove(id);
                Err("The browser closed the task before returning a result.".to_string())
            }
            Err(_) => {
                self.inner.pending.lock().unwrap().remove(id);
                let msg = format!("The browser task timed out after {}s.", timeout_secs);
                crate::link::task_finished(id, Err(&msg));
                Err(msg)
            }
        }
    }

    /// Delegate and await inline (the common case).
    pub async fn run_task(&self, source: &str, instruction: &str, options: Value) -> Result<String, String> {
        let id = self.start_task(source, instruction, options)?;
        self.await_task(&id, DEFAULT_TASK_TIMEOUT_SECS).await
    }

    /// Ask the extension running it to cancel a task.
    pub fn cancel_task(&self, id: &str) {
        let conn = self.inner.pending.lock().unwrap().get(id).map(|t| t.conn);
        let frame = json!({ "v": 1, "type": "cancel", "id": id });
        match conn {
            Some(c) => {
                let _ = self.send_to(c, frame);
            }
            None => {
                let _ = self.send_frame(frame);
            }
        }
    }

    /// Cancel every pending task started by a given desktop agent run (its
    /// "source"). Called when the user hits Stop on that run.
    pub fn cancel_source(&self, source: &str) {
        let ids: Vec<String> = self
            .inner
            .pending
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, t)| t.desktop_source == source)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.cancel_task(&id);
        }
    }
}

fn emit_progress(app: &AppHandle, source: &str, kind: &str, message: String) {
    let _ = app.emit(
        "agent-progress",
        json!({ "source": source, "kind": kind, "message": message }),
    );
}

fn emit_status(app: &AppHandle) {
    let b = bridge();
    let (browsers, paired_browsers) = b.counts();
    let _ = app.emit(
        "bridge-status",
        json!({
            "connected": browsers > 0,
            "paired": b.is_paired(),
            "via": b.paired_via(),
            "browsers": browsers,
            "pairedBrowsers": paired_browsers,
            "port": b.port(),
        }),
    );
}

/// Start the local WebSocket server. Spawned once at app setup; loops forever
/// accepting extension connections. Several browsers may be linked at once; frames
/// without a specific destination go to the active one (see `Inner::active`).
pub async fn serve(app: AppHandle) {
    let listener = match bind_first_free().await {
        Some(l) => l,
        None => {
            eprintln!("[bridge] no free port in {:?} - browser bridge disabled", PORTS);
            return;
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    bridge().inner.port.store(port, Ordering::SeqCst);
    eprintln!("[bridge] listening on 127.0.0.1:{}", port);

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let app2 = app.clone();
                tauri::async_runtime::spawn(handle_connection(stream, app2));
            }
            Err(e) => {
                eprintln!("[bridge] accept error: {}", e);
            }
        }
    }
}

async fn bind_first_free() -> Option<TcpListener> {
    for p in PORTS {
        if let Ok(l) = TcpListener::bind(("127.0.0.1", p)).await {
            return Some(l);
        }
    }
    None
}

async fn handle_connection(stream: tokio::net::TcpStream, app: AppHandle) {
    // Plain HTTP request (no WebSocket upgrade) = the extension's reachability
    // probe. Answering it lets the extension check "is Kudu Desktop running?" with
    // a fetch before opening a WebSocket - a refused WebSocket is logged by
    // Chrome as an extension error on every retry when the app is closed.
    {
        let mut head = [0u8; 1024];
        let n = match tokio::time::timeout(Duration::from_secs(3), stream.peek(&mut head)).await {
            Ok(Ok(n)) => n,
            _ => return,
        };
        let text = String::from_utf8_lossy(&head[..n]).to_ascii_lowercase();
        if text.starts_with("get ") && !text.contains("upgrade: websocket") {
            use tokio::io::AsyncWriteExt;
            let mut stream = stream;
            let _ = stream
                .write_all(b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                .await;
            let _ = stream.shutdown().await;
            return;
        }
    }
    // Reject any origin that isn't a browser extension - a web page must never
    // be able to drive the desktop app through this socket. Returning Err from
    // the handshake callback fails the upgrade, so a rejected origin never
    // reaches the read loop below.
    let callback = |req: &Request, response: Response| -> Result<Response, ErrorResponse> {
        let origin = req
            .headers()
            .get("origin")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        // Empty origin = non-browser local client (a page always sends one).
        if origin.is_empty()
            || origin.starts_with("chrome-extension://")
            || origin.starts_with("moz-extension://")
        {
            Ok(response)
        } else {
            let mut err = ErrorResponse::new(Some("origin not allowed".to_string()));
            *err.status_mut() = StatusCode::FORBIDDEN;
            Err(err)
        }
    };

    let ws = match accept_hdr_async(stream, callback).await {
        Ok(w) => w,
        Err(_) => return,
    };

    let (mut write, mut read) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    // Single writer task owns the sink; everyone else sends via `tx`.
    let writer = tauri::async_runtime::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if write.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Register the connection (unpaired until its hello or link_proof proves otherwise).
    let conn = {
        let b = bridge();
        let id = b.inner.conn_seq.fetch_add(1, Ordering::SeqCst);
        b.inner.conns.lock().unwrap().insert(
            id,
            Conn { tx: tx.clone(), paired_via: None, nonce: uuid::Uuid::new_v4().simple().to_string() },
        );
        // A newcomer only takes over when nothing paired is active.
        if !b.is_paired() {
            b.set_active(id);
        }
        id
    };
    emit_status(&app);

    while let Some(Ok(msg)) = read.next().await {
        match msg {
            Message::Text(txt) => handle_incoming(&app, conn, &txt, &tx),
            Message::Ping(payload) => {
                let _ = tx.send(Message::Pong(payload));
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Disconnected - forget this connection and fail only ITS pending tasks.
    {
        let b = bridge();
        b.inner.conns.lock().unwrap().remove(&conn);
        let was_active = b.inner.active.load(Ordering::SeqCst) == conn;
        if was_active {
            b.reselect_active();
        }
        let drained: Vec<(String, PendingTask)> = {
            let mut pending = b.inner.pending.lock().unwrap();
            let ids: Vec<String> = pending.iter().filter(|(_, t)| t.conn == conn).map(|(id, _)| id.clone()).collect();
            ids.into_iter().filter_map(|id| pending.remove(&id).map(|t| (id, t))).collect()
        };
        for (id, task) in drained {
            if let Some(txr) = task.result_tx {
                let _ = txr.send(Err("The browser extension disconnected.".to_string()));
            }
            crate::link::task_finished(&id, Err("The browser extension disconnected."));
        }
        // Tasks a browser asked us for answer on the active connection; when the
        // active browser leaves and no paired one remains, nobody can receive them.
        if was_active && !b.is_paired() {
            crate::link::cancel_incoming("The browser extension disconnected.");
        }
    }
    emit_status(&app);
    writer.abort();
}

/// Trust a connection and make it the active one.
fn mark_paired(app: &AppHandle, conn: u64, via: &'static str) {
    let b = bridge();
    if let Some(c) = b.inner.conns.lock().unwrap().get_mut(&conn) {
        c.paired_via = Some(via);
    }
    b.set_active(conn);
    emit_status(app);
    crate::auth_sync::on_paired(app);
    crate::analytics::track(app, "desktop_link", json!({ "via": via, "browsers": b.counts().0 }));
}

/// Account pairing: the extension sent a proof for our nonce; ask Kudu Core whether
/// it was minted for the account signed in here.
fn verify_link_proof(app: &AppHandle, conn: u64, v: &Value) {
    let proof = v.get("proof").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let exp = v.get("exp").and_then(|x| x.as_i64()).unwrap_or(0);
    let Some(nonce) = bridge().inner.conns.lock().unwrap().get(&conn).map(|c| c.nonce.clone()) else { return };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<crate::commands::AppState>();
        let verdict = if proof.is_empty() {
            Err("no proof".to_string())
        } else {
            state.backend.verify_link_proof(&nonce, exp, &proof).await
        };
        let (ok, reason) = match verdict {
            Ok(true) => (true, ""),
            Ok(false) => (false, "different_account"),
            Err(_) if !state.backend.has_account().await => (false, "desktop_signed_out"),
            Err(_) => (false, "unverified"),
        };
        if ok {
            mark_paired(&app, conn, "account");
        }
        let _ = bridge().send_to(
            conn,
            json!({
                "v": 1,
                "type": "link_result",
                "ok": ok,
                "reason": reason,
                "acceptsTasks": crate::link::accepts_tasks(),
            }),
        );
    });
}

fn handle_incoming(app: &AppHandle, conn: u64, txt: &str, tx: &mpsc::UnboundedSender<Message>) {
    let v: Value = match serde_json::from_str(txt) {
        Ok(v) => v,
        Err(_) => return,
    };
    let b = bridge();
    let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    // A paired browser that asks for something becomes the one replies go to.
    let paired = b.conn_paired(conn);
    if paired
        && matches!(
            kind,
            "desktop_task" | "open_copilot" | "reveal_path" | "auth_request" | "auth_offer" | "status_request" | "desktop_cancel"
        )
    {
        b.set_active(conn);
    }
    match v.get("type").and_then(|t| t.as_str()) {
        Some("hello") => {
            let code = v.get("pairCode").and_then(|x| x.as_str()).unwrap_or("");
            let by_code = !code.trim().is_empty() && crate::link::code_matches(code);
            let nonce = b.inner.conns.lock().unwrap().get(&conn).map(|c| c.nonce.clone()).unwrap_or_default();
            let _ = tx.send(Message::Text(
                json!({
                    "v": 1,
                    "type": "hello_ack",
                    "desktop": "mouse-ai",
                    "version": env!("CARGO_PKG_VERSION"),
                    // 3: account pairing (challenge -> link_proof -> link_result),
                    // several browsers at once, ports 47821-47825.
                    "protocol": 3,
                    "paired": by_code,
                    "acceptsTasks": crate::link::accepts_tasks(),
                    "challenge": nonce,
                })
                .to_string(),
            ));
            if by_code {
                mark_paired(app, conn, "code");
            } else {
                emit_status(app);
            }
        }
        Some("link_proof") => verify_link_proof(app, conn, &v),
        Some("auth_request") => crate::auth_sync::handle_request(app, paired),
        Some("auth_offer") => crate::auth_sync::handle_offer(app, &v, paired),
        Some("desktop_task") => crate::link::handle_desktop_task(&v, paired),
        Some("desktop_cancel") => crate::link::handle_desktop_cancel(&v),
        Some("status_request") => crate::link::handle_status_request(&v),
        Some("status") => crate::link::handle_status_reply(&v),
        Some("ping") => {
            let _ = tx.send(Message::Text(json!({ "v": 1, "type": "pong" }).to_string()));
        }
        // Undercover from the browser: the extension hands the page it is on to the
        // copilot bar, which (with hide-from-screen-share on) no share or recording
        // shows. Paired connections only, like reveal_path.
        Some("open_copilot") => {
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let s = |k: &str, cap: usize| -> String {
                v.get(k).and_then(|x| x.as_str()).unwrap_or("").chars().take(cap).collect()
            };
            let ok = paired;
            if ok {
                crate::meeting::show_copilot(app);
                let _ = app.emit_to(
                    "copilot",
                    "copilot-context",
                    json!({
                        "url": s("url", 2_000),
                        "title": s("title", 300),
                        "text": s("text", 12_000),
                        "selection": s("selection", 4_000),
                    }),
                );
            }
            let _ = tx.send(Message::Text(
                json!({
                    "v": 1,
                    "type": "copilot_opened",
                    "id": id,
                    "ok": ok,
                    "error": if ok { Value::Null } else { json!("The extension isn't paired with Kudu Desktop.") },
                })
                .to_string(),
            ));
        }
        // "Show in folder" next to a file path in the extension: open Explorer on it.
        // Paired connections only; it only ever opens a folder view, never runs a file.
        Some("reveal_path") => {
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("");
            let result = if paired {
                crate::fs_browser::reveal_path(path)
            } else {
                Err("This browser isn't paired with Kudu Desktop.".to_string())
            };
            let _ = tx.send(Message::Text(
                json!({ "v": 1, "type": "reveal_result", "id": id, "ok": result.is_ok(), "error": result.err() }).to_string(),
            ));
        }
        Some("progress") => {
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
            let kind = v.get("kind").and_then(|x| x.as_str()).unwrap_or("tool");
            let message = v.get("message").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            // An empty step used to render as a bare "🌐" row; a step that quotes the
            // browser's answer must never carry a file's base64 into the live log.
            if message.is_empty() {
                return;
            }
            let message = crate::kudu_files::summarize(&message);
            let message = if message.chars().count() > 600 {
                format!("{}…", message.chars().take(600).collect::<String>())
            } else {
                message
            };
            let source = bridge()
                .inner
                .pending
                .lock()
                .unwrap()
                .get(id)
                .map(|t| t.desktop_source.clone());
            crate::link::task_step(id, kind, &message);
            if let Some(src) = source {
                // Prefix so the browser's steps are visually distinct from the
                // desktop's own tool calls in the shared live log. Its narration
                // ("thinking") used to go out bare, so it read as the desktop agent's
                // own output; it is marked as the browser's too.
                let label = if kind == "thinking" {
                    format!("🌐 Browser: {}", message)
                } else {
                    format!("🌐 {}", message)
                };
                emit_progress(app, &src, "tool-result", label);
            }
        }
        Some("result") => {
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
            let payload = if ok {
                Ok(v.get("result").and_then(|x| x.as_str()).unwrap_or("").to_string())
            } else {
                Err(v
                    .get("error")
                    .and_then(|x| x.as_str())
                    .unwrap_or("The browser task failed.")
                    .to_string())
            };
            match &payload {
                // The board shows a short summary: file blocks become "[file: name, size]".
                Ok(r) => crate::link::task_finished(&id, Ok(&crate::kudu_files::summarize(r))),
                Err(e) => crate::link::task_finished(&id, Err(e)),
            }
            let task = bridge().inner.pending.lock().unwrap().remove(&id);
            if let Some(task) = task {
                if let Some(txr) = task.result_tx {
                    let _ = txr.send(payload);
                }
            }
        }
        // "accepted", "pong", "notify" - informational, nothing to do.
        _ => {}
    }
}
