// Kudu Desktop - Google OAuth loopback flow
// Opens the user's default browser on Google's OAuth consent screen (implicit
// flow, same web client the Kudu Connect extension uses) and catches the redirect
// on a tiny localhost HTTP listener. The access token never leaves the machine
// except to be exchanged with Kudu Core for a session.
//
// One-time Google Cloud Console setup (same OAuth web client as the extension):
// add  http://127.0.0.1:8917/callback/  (with the trailing slash) to its Authorized redirect URIs.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const OAUTH_PORT: u16 = 8917;
const DEFAULT_WEB_CLIENT_ID: &str =
    "472048810163-5gn8amf57iq9in97hr9v0u039hfkfdkg.apps.googleusercontent.com";
const SCOPES: &str = "openid email profile";
// How long we wait for the user to finish the consent screen in the browser.
const FLOW_TIMEOUT: Duration = Duration::from_secs(240);

static FLOW_RUNNING: AtomicBool = AtomicBool::new(false);
/// Random per-flow value Google echoes back. Only a redirect carrying it is
/// accepted, so another page or local process can't push its own token into
/// the listener (login CSRF / token injection).
static FLOW_STATE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

fn client_id() -> String {
    std::env::var("MOUSE_AI_GOOGLE_CLIENT_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_WEB_CLIENT_ID.to_string())
}

/// Must match the OAuth client's Authorized redirect URI character for character -
/// Google rejects "/callback" when "/callback/" is registered ("This app's request is
/// invalid"). The listener accepts both paths.
pub fn redirect_uri() -> String {
    format!("http://127.0.0.1:{}/callback/", OAUTH_PORT)
}

pub fn build_auth_url() -> String {
    let state = uuid::Uuid::new_v4().simple().to_string();
    *FLOW_STATE.lock().unwrap() = state.clone();
    format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&response_type=token&redirect_uri={}&scope={}&prompt=select_account&state={}",
        urlencode(&client_id()),
        urlencode(&redirect_uri()),
        urlencode(SCOPES),
        state,
    )
}

fn state_matches(query: &str) -> bool {
    let expected = FLOW_STATE.lock().unwrap().clone();
    !expected.is_empty() && query_param(query, "state").is_some_and(|s| s == expected)
}

/// Blocking: serve the loopback listener until Google redirects back with an
/// access token (or we time out). Call from spawn_blocking.
/// Returns the Google access token.
pub fn wait_for_token(listener: TcpListener) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("Listener setup failed: {}", e))?;

    let deadline = Instant::now() + FLOW_TIMEOUT;
    let result = accept_loop(&listener, deadline);
    FLOW_RUNNING.store(false, Ordering::SeqCst);
    result
}

/// Release the in-progress flag - for callers that bound the listener but
/// failed before wait_for_token (e.g. the browser couldn't be opened).
pub fn release_flow() {
    FLOW_RUNNING.store(false, Ordering::SeqCst);
}

/// Bind the loopback port up front so we can fail fast (port in use / flow
/// already running) before opening the browser.
pub fn bind_listener() -> Result<TcpListener, String> {
    if FLOW_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("A Google sign-in is already in progress - finish it in your browser or wait a moment.".to_string());
    }
    match TcpListener::bind(("127.0.0.1", OAUTH_PORT)) {
        Ok(l) => Ok(l),
        Err(e) => {
            FLOW_RUNNING.store(false, Ordering::SeqCst);
            Err(format!(
                "Couldn't listen on 127.0.0.1:{} for the sign-in redirect: {}",
                OAUTH_PORT, e
            ))
        }
    }
}

fn accept_loop(listener: &TcpListener, deadline: Instant) -> Result<String, String> {
    loop {
        if Instant::now() >= deadline {
            return Err(format!(
                "Timed out waiting for Google sign-in. If the browser showed a 'redirect_uri_mismatch' error, add {} to the OAuth client's Authorized redirect URIs in Google Cloud Console - or use the manual token option instead.",
                redirect_uri()
            ));
        }

        let mut stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(120));
                continue;
            }
            Err(e) => return Err(format!("Sign-in listener error: {}", e)),
        };

        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..n]);
        let path = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/")
            .to_string();

        if path.starts_with("/callback") {
            // Token arrives in the URL *fragment*, which never reaches the
            // server - serve a page that re-sends it as a query string.
            respond_html(&mut stream, 200, CALLBACK_PAGE);
            continue;
        }

        if let Some(query) = path.strip_prefix("/finish?") {
            if let Some(token) = query_param(query, "access_token") {
                if !state_matches(query) {
                    // Not the redirect from the flow we started - ignore it and
                    // keep waiting for the real one.
                    respond_html(&mut stream, 200, &error_page("sign-in request did not match"));
                    continue;
                }
                FLOW_STATE.lock().unwrap().clear();
                respond_html(&mut stream, 200, SUCCESS_PAGE);
                return Ok(token);
            }
            let err = query_param(query, "error").unwrap_or_else(|| "no token returned".into());
            respond_html(&mut stream, 200, &error_page(&err));
            return Err(format!("Google sign-in failed: {}", err));
        }

        respond_html(&mut stream, 404, "Not found");
    }
}

fn respond_html(stream: &mut std::net::TcpStream, status: u16, body: &str) {
    let reason = if status == 200 { "OK" } else { "Not Found" };
    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        reason,
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let mut it = pair.splitn(2, '=');
        if it.next()? == name {
            Some(urldecode(it.next().unwrap_or("")))
        } else {
            None
        }
    })
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

const CALLBACK_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Kudu Desktop - Signing in…</title></head>
<body style="font-family:system-ui;background:#111;color:#eee;display:flex;align-items:center;justify-content:center;height:100vh">
<p>Finishing sign-in…</p>
<script>
  var h = window.location.hash ? window.location.hash.substring(1) : "";
  var q = window.location.search ? window.location.search.substring(1) : "";
  window.location.replace("/finish?" + (h || q || "error=no_response"));
</script>
</body></html>"#;

const SUCCESS_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Kudu Desktop - Signed in</title></head>
<body style="font-family:system-ui;background:#111;color:#eee;display:flex;align-items:center;justify-content:center;height:100vh;flex-direction:column">
<h2>✅ Signed in to Kudu Desktop</h2><p>You can close this tab and return to the app.</p>
</body></html>"#;

fn error_page(err: &str) -> String {
    let safe: String = err
        .chars()
        .filter(|c| c.is_alphanumeric() || " _-.:".contains(*c))
        .take(200)
        .collect();
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Kudu Desktop - Sign-in failed</title></head>
<body style="font-family:system-ui;background:#111;color:#eee;display:flex;align-items:center;justify-content:center;height:100vh;flex-direction:column">
<h2>⚠️ Sign-in failed</h2><p>{}</p><p>Return to Kudu Desktop and try again.</p>
</body></html>"#,
        safe
    )
}
