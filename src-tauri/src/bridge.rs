// Mouse AI Desktop - Browser bridge (WebSocket server).
//
// The desktop app is the ORCHESTRATOR. When an agent task needs something done
// on the web - open a page, fill a form, read a site - it can't do that itself
// reliably (screenshot+coordinate automation is fragile in a browser). Instead
// it delegates that piece to the Thothai browser extension over a local
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
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;

/// A single deterministic, uncommon port both sides agree on. One fixed port
/// (not a range) means the extension makes at most one connection attempt, so a
/// desktop that isn't running costs the extension at most one quiet console log
/// per retry. Well outside the common dev-server ports that caused earlier 404s.
const PORTS: [u16; 1] = [47821];
const DEFAULT_TASK_TIMEOUT_SECS: u64 = 600;

#[derive(Clone)]
pub struct Bridge {
    inner: Arc<Inner>,
}

struct Inner {
    connected: AtomicBool,
    /// The live connection presented this app's pairing code (see link.rs) -
    /// required before the browser may hand the desktop tasks.
    paired: AtomicBool,
    port: AtomicU16,
    client_tx: Mutex<Option<mpsc::UnboundedSender<Message>>>,
    pending: Mutex<HashMap<String, PendingTask>>,
    // Result receivers for delegated tasks, kept so await_task can collect them
    // after start_task returned (enables the desktop to work in parallel).
    results: Mutex<HashMap<String, oneshot::Receiver<Result<String, String>>>>,
    task_seq: AtomicU16,
}

struct PendingTask {
    desktop_source: String,
    result_tx: Option<oneshot::Sender<Result<String, String>>>,
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

pub fn bridge() -> Bridge {
    BRIDGE
        .get_or_init(|| Bridge {
            inner: Arc::new(Inner {
                connected: AtomicBool::new(false),
                paired: AtomicBool::new(false),
                port: AtomicU16::new(0),
                client_tx: Mutex::new(None),
                pending: Mutex::new(HashMap::new()),
                results: Mutex::new(HashMap::new()),
                task_seq: AtomicU16::new(1),
            }),
        })
        .clone()
}

impl Bridge {
    pub fn is_connected(&self) -> bool {
        self.inner.connected.load(Ordering::SeqCst)
    }

    pub fn port(&self) -> u16 {
        self.inner.port.load(Ordering::SeqCst)
    }

    pub fn is_paired(&self) -> bool {
        self.is_connected() && self.inner.paired.load(Ordering::SeqCst)
    }

    pub fn unpair(&self) {
        self.inner.paired.store(false, Ordering::SeqCst);
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
        let guard = self.inner.client_tx.lock().unwrap();
        if let Some(tx) = guard.as_ref() {
            tx.send(Message::Text(value.to_string())).is_ok()
        } else {
            false
        }
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
with the Thothai extension installed and signed in, then try again."
                    .to_string(),
            );
        }
        let id = self.next_task_id();
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().unwrap().insert(
            id.clone(),
            PendingTask { desktop_source: source.to_string(), result_tx: Some(tx) },
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

    /// Ask the extension to cancel a running task.
    pub fn cancel_task(&self, id: &str) {
        let _ = self.send_frame(json!({ "v": 1, "type": "cancel", "id": id }));
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

fn emit_status(app: &AppHandle, connected: bool) {
    let _ = app.emit(
        "bridge-status",
        json!({ "connected": connected, "paired": connected && bridge().is_paired(), "port": bridge().port() }),
    );
}

/// Start the local WebSocket server. Spawned once at app setup; loops forever
/// accepting extension connections. Only one extension is treated as live at a
/// time (the most recent to connect).
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
    // probe. Answering it lets the extension check "is Mouse AI running?" with
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

    // Mark this connection live (unpaired until its hello proves otherwise).
    {
        let b = bridge();
        b.inner.paired.store(false, Ordering::SeqCst);
        *b.inner.client_tx.lock().unwrap() = Some(tx.clone());
        b.inner.connected.store(true, Ordering::SeqCst);
    }
    emit_status(&app, true);

    while let Some(Ok(msg)) = read.next().await {
        match msg {
            Message::Text(txt) => handle_incoming(&app, &txt, &tx),
            Message::Ping(payload) => {
                let _ = tx.send(Message::Pong(payload));
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Disconnected - clear live state and fail any still-pending tasks.
    {
        let b = bridge();
        b.inner.connected.store(false, Ordering::SeqCst);
        b.inner.paired.store(false, Ordering::SeqCst);
        *b.inner.client_tx.lock().unwrap() = None;
        let drained: Vec<(String, PendingTask)> = b.inner.pending.lock().unwrap().drain().collect();
        for (id, task) in drained {
            if let Some(txr) = task.result_tx {
                let _ = txr.send(Err("The browser extension disconnected.".to_string()));
            }
            crate::link::task_finished(&id, Err("The browser extension disconnected."));
        }
        // Nobody is left to receive the results of tasks the browser asked for.
        crate::link::cancel_incoming("The browser extension disconnected.");
    }
    emit_status(&app, false);
    writer.abort();
}

fn handle_incoming(app: &AppHandle, txt: &str, tx: &mpsc::UnboundedSender<Message>) {
    let v: Value = match serde_json::from_str(txt) {
        Ok(v) => v,
        Err(_) => return,
    };
    match v.get("type").and_then(|t| t.as_str()) {
        Some("hello") => {
            let code = v.get("pairCode").and_then(|x| x.as_str()).unwrap_or("");
            let paired = !code.trim().is_empty() && crate::link::code_matches(code);
            bridge().inner.paired.store(paired, Ordering::SeqCst);
            let _ = tx.send(Message::Text(
                json!({
                    "v": 1,
                    "type": "hello_ack",
                    "desktop": "mouse-ai",
                    "version": env!("CARGO_PKG_VERSION"),
                    "protocol": 2,
                    "paired": paired,
                    "acceptsTasks": crate::link::accepts_tasks(),
                })
                .to_string(),
            ));
            emit_status(app, true);
            if paired {
                crate::auth_sync::on_paired(app);
            }
        }
        Some("auth_request") => crate::auth_sync::handle_request(app, bridge().is_paired()),
        Some("auth_offer") => crate::auth_sync::handle_offer(app, &v, bridge().is_paired()),
        Some("desktop_task") => crate::link::handle_desktop_task(&v, bridge().is_paired()),
        Some("desktop_cancel") => crate::link::handle_desktop_cancel(&v),
        Some("status_request") => crate::link::handle_status_request(&v),
        Some("status") => crate::link::handle_status_reply(&v),
        Some("ping") => {
            let _ = tx.send(Message::Text(json!({ "v": 1, "type": "pong" }).to_string()));
        }
        Some("progress") => {
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
            let kind = v.get("kind").and_then(|x| x.as_str()).unwrap_or("tool");
            let message = v.get("message").and_then(|x| x.as_str()).unwrap_or("").to_string();
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
                // desktop's own tool calls in the shared live log.
                let label = if kind == "thinking" { message } else { format!("🌐 {}", message) };
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
                Ok(r) => crate::link::task_finished(&id, Ok(r)),
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
