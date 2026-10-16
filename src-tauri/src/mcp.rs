// Kudu Desktop - MCP (Model Context Protocol) client
//
// Lets the agent use tools from external MCP servers (filesystem, GitHub, Slack,
// Notion, databases, …) alongside its built-in tools. Two transports:
//   • stdio - a local subprocess speaking newline-delimited JSON-RPC 2.0
//             (`npx -y @modelcontextprotocol/server-filesystem C:\…`, `uvx …`);
//   • http  - a remote "Streamable HTTP" endpoint (`"url": "https://…/mcp"`), where
//             each JSON-RPC request is a POST answered with JSON or an SSE stream.
// Handshake for both: `initialize` → `notifications/initialized` → `tools/list`
// (paginated), then `tools/call` on demand.
//
// Design rule: MCP is strictly additive and fail-safe. If a server won't start,
// times out, or errors, it's skipped with a note that says WHY (its stderr tail)
// and the agent proceeds on its built-in tools - MCP problems must never break
// the core loop. That includes tool schemas: Gemini rejects the whole request if
// one declaration uses JSON-Schema it doesn't support, so every schema is reduced
// to Gemini's subset before it is offered (see `gemini_schema`).

use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

const INIT_TIMEOUT: Duration = Duration::from_secs(25); // npx may download the server first
const LIST_TIMEOUT: Duration = Duration::from_secs(20);
const CALL_TIMEOUT: Duration = Duration::from_secs(90);
const PROTOCOL_VERSION: &str = "2025-03-26";
const MAX_TOOL_NAME: usize = 64; // Gemini's function-name limit
const MAX_RESULT_CHARS: usize = 20_000;

/// One configured MCP server (persisted in the app config).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct McpServerConfig {
    pub name: String,
    /// stdio: the program to run (`npx`, `uvx`, `node`, a full path…).
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// http: the server's endpoint. When set, `command` is ignored.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Keep a server configured but switched off.
    #[serde(default)]
    pub disabled: bool,
}

/// Accept the formats people actually paste: our own array, or the
/// `{"mcpServers": {"name": {command, args, env, url}}}` object used by Claude
/// Desktop, Cursor, VS Code and most MCP READMEs. Returns a normalized array.
pub fn normalize_config(value: Value) -> Result<Value, String> {
    let map = match &value {
        Value::Array(_) => return Ok(value),
        Value::Object(obj) => obj
            .get("mcpServers")
            .or_else(|| obj.get("servers"))
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_else(|| obj.clone()),
        _ => return Err("MCP servers must be a JSON array or an {\"mcpServers\": {…}} object.".to_string()),
    };
    let mut out = Vec::new();
    for (name, cfg) in map {
        let Some(obj) = cfg.as_object() else { continue };
        let mut entry = obj.clone();
        entry.insert("name".to_string(), json!(name));
        // VS Code style: "type": "http" with "url"; Claude style: "serverUrl".
        if !entry.contains_key("url") {
            if let Some(u) = obj.get("serverUrl").cloned() {
                entry.insert("url".to_string(), u);
            }
        }
        out.push(Value::Object(entry));
    }
    if out.is_empty() {
        return Err("No MCP servers found in that JSON.".to_string());
    }
    Ok(Value::Array(out))
}

// ── Transports ──────────────────────────────────────────────────────────────

struct StdioServer {
    child: Child,
    stdin: ChildStdin,
    reader: Lines<BufReader<ChildStdout>>,
    stderr_tail: Arc<Mutex<Vec<String>>>,
}

struct HttpServer {
    client: reqwest::Client,
    url: String,
    headers: HashMap<String, String>,
    session_id: Option<String>,
}

enum Transport {
    Stdio(StdioServer),
    Http(HttpServer),
}

struct RunningServer {
    transport: Transport,
    next_id: i64,
}

impl RunningServer {
    async fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let reply = match &mut self.transport {
            Transport::Stdio(s) => s.request(id, &msg, timeout).await,
            Transport::Http(h) => h.request(id, &msg, timeout).await,
        };
        let value = reply.map_err(|e| format!("{} ({})", e, method))?;
        if let Some(err) = value.get("error") {
            let text = err.get("message").and_then(|m| m.as_str()).map(String::from).unwrap_or_else(|| err.to_string());
            return Err(format!("MCP error: {}", text));
        }
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        match &mut self.transport {
            Transport::Stdio(s) => {
                let _ = s.write_line(&msg).await;
            }
            Transport::Http(h) => {
                let _ = h.post(&msg, Duration::from_secs(10)).await;
            }
        }
    }

    fn stderr_hint(&self) -> String {
        match &self.transport {
            Transport::Stdio(s) => s.stderr_hint(),
            Transport::Http(_) => String::new(),
        }
    }

    async fn kill(&mut self) {
        if let Transport::Stdio(s) = &mut self.transport {
            let _ = s.child.start_kill();
        }
    }
}

impl StdioServer {
    async fn request(&mut self, id: i64, msg: &Value, timeout: Duration) -> Result<Value, String> {
        self.write_line(msg).await?;
        // Read lines until the response with our id arrives (skipping the server's
        // notifications/log lines), or we time out.
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(format!("timed out{}", self.stderr_hint()));
            }
            let line = match tokio::time::timeout(remaining, self.reader.next_line()).await {
                Err(_) => return Err(format!("timed out{}", self.stderr_hint())),
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) => return Err(format!("the server exited{}", self.stderr_hint())),
                Ok(Err(e)) => return Err(format!("read error: {}", e)),
            };
            let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
                continue; // log noise on stdout - ignore
            };
            if value.get("id").and_then(|v| v.as_i64()) == Some(id) {
                return Ok(value);
            }
        }
    }

    async fn write_line(&mut self, msg: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("write error: {}{}", e, self.stderr_hint()))?;
        self.stdin.flush().await.map_err(|e| format!("flush error: {}", e))
    }

    /// The last few stderr lines - the server's own explanation of what went wrong
    /// (missing env var, bad path, package not found). Used to be thrown away.
    fn stderr_hint(&self) -> String {
        let tail = self.stderr_tail.lock().map(|t| t.join(" | ")).unwrap_or_default();
        if tail.trim().is_empty() {
            String::new()
        } else {
            format!(" - server said: {}", truncate(&tail, 400))
        }
    }
}

impl HttpServer {
    async fn post(&mut self, msg: &Value, timeout: Duration) -> Result<reqwest::Response, String> {
        let mut req = self
            .client
            .post(&self.url)
            .timeout(timeout)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(msg);
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }
        if let Some(sid) = &self.session_id {
            req = req.header("Mcp-Session-Id", sid);
        }
        let resp = req.send().await.map_err(|e| format!("HTTP error: {}", e))?;
        if let Some(sid) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            self.session_id = Some(sid.to_string());
        }
        Ok(resp)
    }

    async fn request(&mut self, id: i64, msg: &Value, timeout: Duration) -> Result<Value, String> {
        let resp = self.post(msg, timeout).await?;
        let status = resp.status();
        let is_sse = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("text/event-stream"))
            .unwrap_or(false);
        let body = resp.text().await.map_err(|e| format!("HTTP read error: {}", e))?;
        if !status.is_success() {
            let hint = if status.as_u16() == 401 || status.as_u16() == 403 {
                " - check the server's API key / Authorization header in \"headers\""
            } else {
                ""
            };
            return Err(format!("HTTP {}{}: {}", status.as_u16(), hint, truncate(body.trim(), 300)));
        }
        let candidates: Vec<Value> = if is_sse {
            body.lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
                .collect()
        } else {
            match serde_json::from_str::<Value>(&body) {
                Ok(Value::Array(items)) => items,
                Ok(v) => vec![v],
                Err(_) => vec![],
            }
        };
        candidates
            .into_iter()
            .find(|v| v.get("id").and_then(|x| x.as_i64()) == Some(id))
            .ok_or_else(|| "the server sent no reply for this request".to_string())
    }
}

/// Windows can't spawn `npx`/`pnpm`/… directly: they are .cmd shims, which
/// CreateProcess won't run. Real executables (node.exe, uvx.exe, python.exe) are
/// started directly; anything else goes through `cmd /C`. No console window either.
fn build_command(cfg: &McpServerConfig) -> Command {
    #[cfg(windows)]
    {
        let direct = is_windows_exe(&cfg.command);
        let mut cmd = if direct {
            let mut c = Command::new(&cfg.command);
            c.args(&cfg.args);
            c
        } else {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&cfg.command).args(&cfg.args);
            c
        };
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new(&cfg.command);
        c.args(&cfg.args);
        c
    }
}

#[cfg(windows)]
fn is_windows_exe(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    if lower.ends_with(".exe") {
        return true;
    }
    if lower.ends_with(".cmd") || lower.ends_with(".bat") {
        return false;
    }
    let p = std::path::Path::new(command);
    if p.is_absolute() {
        return p.with_extension("exe").exists() || p.exists() && p.extension().is_none();
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(format!("{}.exe", command)).exists()))
        .unwrap_or(false)
}

// ── Schemas: JSON Schema → the subset Gemini's function calling accepts ────────

/// Reduce an MCP input schema to what Gemini accepts. Unsupported keywords
/// ($schema, additionalProperties, $ref, default, examples, patterns…) are dropped,
/// `type: ["string","null"]` becomes `type + nullable`, anyOf/oneOf collapse to their
/// first concrete branch, and an object with no properties becomes a JSON string the
/// model fills in (decoded again in `call`). Returns (schema, top-level keys that
/// must be JSON-decoded at call time).
fn gemini_schema(schema: &Value) -> (Value, HashSet<String>) {
    let mut json_keys = HashSet::new();
    let mut out = clean_schema(schema, 0);
    if out.get("type").and_then(|t| t.as_str()) != Some("object") {
        out = json!({ "type": "object", "properties": {} });
    }
    if let Some(props) = out.get("properties").and_then(|p| p.as_object()) {
        for (k, v) in props {
            if v.get("description").and_then(|d| d.as_str()).map(|d| d.contains("(JSON)")).unwrap_or(false)
                && v.get("type").and_then(|t| t.as_str()) == Some("string")
            {
                json_keys.insert(k.clone());
            }
        }
    }
    (out, json_keys)
}

fn clean_schema(schema: &Value, depth: usize) -> Value {
    let Some(obj) = schema.as_object() else { return json!({ "type": "string" }) };
    if depth > 6 {
        return json!({ "type": "string", "description": "(JSON) nested value" });
    }
    // anyOf / oneOf / allOf: take the first branch that isn't just null.
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(branches) = obj.get(key).and_then(|b| b.as_array()) {
            if let Some(first) = branches.iter().find(|b| b.get("type").and_then(|t| t.as_str()) != Some("null")) {
                let mut merged = first.as_object().cloned().unwrap_or_default();
                if let Some(d) = obj.get("description") {
                    merged.entry("description".to_string()).or_insert(d.clone());
                }
                return clean_schema(&Value::Object(merged), depth);
            }
        }
    }

    let mut nullable = false;
    let mut ty = match obj.get("type") {
        Some(Value::String(t)) => t.clone(),
        Some(Value::Array(ts)) => {
            nullable = ts.iter().any(|t| t.as_str() == Some("null"));
            ts.iter().filter_map(|t| t.as_str()).find(|t| *t != "null").unwrap_or("string").to_string()
        }
        _ if obj.contains_key("properties") => "object".to_string(),
        _ if obj.contains_key("items") => "array".to_string(),
        _ if obj.contains_key("enum") => "string".to_string(),
        _ => "string".to_string(),
    };
    if !["string", "number", "integer", "boolean", "array", "object"].contains(&ty.as_str()) {
        ty = "string".to_string();
    }

    let mut out = Map::new();
    let mut desc = obj.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string();

    match ty.as_str() {
        "object" => {
            let props = obj.get("properties").and_then(|p| p.as_object());
            match props {
                Some(props) if !props.is_empty() => {
                    let mut cleaned = Map::new();
                    for (k, v) in props {
                        cleaned.insert(k.clone(), clean_schema(v, depth + 1));
                    }
                    let required: Vec<Value> = obj
                        .get("required")
                        .and_then(|r| r.as_array())
                        .map(|r| r.iter().filter(|k| k.as_str().map(|s| cleaned.contains_key(s)).unwrap_or(false)).cloned().collect())
                        .unwrap_or_default();
                    out.insert("type".into(), json!("object"));
                    out.insert("properties".into(), Value::Object(cleaned));
                    if !required.is_empty() {
                        out.insert("required".into(), Value::Array(required));
                    }
                }
                _ if depth == 0 => {
                    out.insert("type".into(), json!("object"));
                    out.insert("properties".into(), json!({}));
                }
                _ => {
                    // Free-form object: Gemini rejects OBJECT with no properties.
                    out.insert("type".into(), json!("string"));
                    desc = format!("{} (JSON) a JSON object", desc).trim().to_string();
                }
            }
        }
        "array" => {
            out.insert("type".into(), json!("array"));
            let items = obj.get("items").map(|i| clean_schema(i, depth + 1)).unwrap_or_else(|| json!({ "type": "string" }));
            out.insert("items".into(), items);
            for k in ["minItems", "maxItems"] {
                if let Some(v) = obj.get(k).filter(|v| v.is_u64()) {
                    out.insert(k.into(), v.clone());
                }
            }
        }
        "string" => {
            out.insert("type".into(), json!("string"));
            if let Some(vals) = obj.get("enum").and_then(|e| e.as_array()) {
                let strings: Vec<Value> = vals.iter().filter_map(|v| v.as_str().map(|s| json!(s))).collect();
                if !strings.is_empty() && strings.len() == vals.len() {
                    out.insert("enum".into(), Value::Array(strings));
                }
            }
            if let Some(f) = obj.get("format").and_then(|f| f.as_str()) {
                if f == "date-time" || f == "enum" {
                    out.insert("format".into(), json!(f));
                } else if !f.is_empty() {
                    desc = format!("{} (format: {})", desc, f).trim().to_string();
                }
            }
        }
        other => {
            out.insert("type".into(), json!(other));
            for k in ["minimum", "maximum"] {
                if let Some(v) = obj.get(k).filter(|v| v.is_number()) {
                    out.insert(k.into(), v.clone());
                }
            }
        }
    }
    if let Some(def) = obj.get("default") {
        if !def.is_null() {
            desc = format!("{} (default: {})", desc, truncate(&def.to_string(), 60)).trim().to_string();
        }
    }
    if !desc.is_empty() {
        out.insert("description".into(), json!(truncate(&desc, 900)));
    }
    if nullable {
        out.insert("nullable".into(), json!(true));
    }
    Value::Object(out)
}

// ── Manager ─────────────────────────────────────────────────────────────────

/// A tool discovered from an MCP server, plus the routing info to call it.
pub struct McpTool {
    /// Gemini-safe function name: `mcp__<server>__<tool>`, ≤ 64 chars, unique.
    pub api_name: String,
    pub server: String,
    pub tool: String,
    pub description: String,
    pub input_schema: Value,
    json_keys: HashSet<String>,
}

pub struct McpManager {
    servers: HashMap<String, RunningServer>,
    tools: Vec<McpTool>,
}

fn sanitize(part: &str) -> String {
    part.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// `mcp__server__tool`, shortened (and made unique) to fit Gemini's 64-char limit.
fn api_name_for(server: &str, tool: &str, taken: &HashSet<String>) -> String {
    let full = format!("mcp__{}__{}", sanitize(server), sanitize(tool));
    if full.len() <= MAX_TOOL_NAME && !taken.contains(&full) {
        return full;
    }
    let mut h: u32 = 2166136261;
    for b in full.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    let suffix = format!("_{:06x}", h & 0xff_ffff);
    let base: String = full.chars().take(MAX_TOOL_NAME - suffix.len()).collect();
    format!("{}{}", base, suffix)
}

async fn start_server(cfg: &McpServerConfig) -> Result<(RunningServer, Vec<Value>), String> {
    let transport = if let Some(url) = cfg.url.as_ref().map(|u| u.trim()).filter(|u| !u.is_empty()) {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(format!("\"url\" must start with http:// or https:// (got {})", url));
        }
        Transport::Http(HttpServer {
            client: reqwest::Client::new(),
            url: url.to_string(),
            headers: cfg.headers.clone(),
            session_id: None,
        })
    } else {
        if cfg.command.trim().is_empty() {
            return Err("needs a \"command\" (stdio) or a \"url\" (HTTP)".to_string());
        }
        let mut command = build_command(cfg);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &cfg.env {
            command.env(k, v);
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("couldn't start '{}': {} - is it installed and on PATH?", cfg.command, e))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr_tail = Arc::new(Mutex::new(Vec::<String>::new()));
        if let Some(stderr) = child.stderr.take() {
            let tail = stderr_tail.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Ok(mut t) = tail.lock() {
                        t.push(truncate(line.trim(), 200));
                        if t.len() > 6 {
                            t.remove(0);
                        }
                    }
                }
            });
        }
        Transport::Stdio(StdioServer { child, stdin, reader: BufReader::new(stdout).lines(), stderr_tail })
    };

    let mut server = RunningServer { transport, next_id: 0 };
    let init_params = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "mouse-ai-desktop", "version": env!("CARGO_PKG_VERSION") }
    });
    if let Err(e) = server.request("initialize", init_params, INIT_TIMEOUT).await {
        server.kill().await;
        return Err(e);
    }
    server.notify("notifications/initialized", json!({})).await;

    // Discover tools, following pagination.
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..20 {
        let params = match &cursor {
            Some(c) => json!({ "cursor": c }),
            None => json!({}),
        };
        let list = match server.request("tools/list", params, LIST_TIMEOUT).await {
            Ok(l) => l,
            Err(e) => {
                let hint = server.stderr_hint();
                server.kill().await;
                return Err(format!("{}{}", e, hint));
            }
        };
        tools.extend(list.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default());
        cursor = list.get("nextCursor").and_then(|c| c.as_str()).map(String::from).filter(|c| !c.is_empty());
        if cursor.is_none() {
            break;
        }
    }
    Ok((server, tools))
}

impl McpManager {
    /// Start every enabled server (in parallel - one slow `npx` download no longer
    /// delays the rest), run the handshake, and collect their tools. Returns the
    /// manager plus one human-readable note per server. Never fails as a whole.
    pub async fn start(configs: &[McpServerConfig]) -> (Self, Vec<String>) {
        let mut manager = McpManager { servers: HashMap::new(), tools: Vec::new() };
        let mut notes = Vec::new();
        let enabled: Vec<&McpServerConfig> = configs.iter().filter(|c| !c.disabled && !c.name.trim().is_empty()).collect();
        let started = futures_util::future::join_all(enabled.iter().map(|cfg| start_server(cfg))).await;

        let mut taken: HashSet<String> = HashSet::new();
        for (cfg, outcome) in enabled.into_iter().zip(started) {
            match outcome {
                Ok((server, tools)) => {
                    let mut count = 0;
                    for t in tools {
                        let Some(name) = t.get("name").and_then(|n| n.as_str()) else { continue };
                        let api_name = api_name_for(&cfg.name, name, &taken);
                        taken.insert(api_name.clone());
                        let (schema, json_keys) =
                            gemini_schema(t.get("inputSchema").unwrap_or(&json!({ "type": "object" })));
                        manager.tools.push(McpTool {
                            api_name,
                            server: cfg.name.clone(),
                            tool: name.to_string(),
                            description: t.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string(),
                            input_schema: schema,
                            json_keys,
                        });
                        count += 1;
                    }
                    manager.servers.insert(cfg.name.clone(), server);
                    notes.push(format!("MCP '{}': connected, {} tool(s)", cfg.name, count));
                }
                Err(e) => notes.push(format!("MCP '{}' skipped: {}", cfg.name, e)),
            }
        }
        (manager, notes)
    }

    /// True if a tool name belongs to an MCP server (so the agent routes it here).
    pub fn owns(&self, api_name: &str) -> bool {
        self.tools.iter().any(|t| t.api_name == api_name)
    }

    /// Gemini functionDeclarations for all discovered MCP tools.
    pub fn tool_declarations(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.api_name,
                    "description": truncate(&format!("[MCP:{}] {}", t.server, t.description), 1000),
                    "parameters": t.input_schema
                })
            })
            .collect()
    }

    /// Invoke an MCP tool by its `mcp__server__tool` api name.
    pub async fn call(&mut self, api_name: &str, args: Value) -> Result<String, String> {
        let (server_name, tool_name, json_keys) = match self.tools.iter().find(|t| t.api_name == api_name) {
            Some(t) => (t.server.clone(), t.tool.clone(), t.json_keys.clone()),
            None => return Err(format!("unknown MCP tool: {}", api_name)),
        };
        let server = self
            .servers
            .get_mut(&server_name)
            .ok_or_else(|| format!("MCP server '{}' not running", server_name))?;

        // Free-form objects were offered to the model as JSON strings - decode them back.
        let mut args = if args.is_object() { args } else { json!({}) };
        if let Some(obj) = args.as_object_mut() {
            for key in &json_keys {
                if let Some(Value::String(s)) = obj.get(key) {
                    if let Ok(parsed) = serde_json::from_str::<Value>(s) {
                        obj.insert(key.clone(), parsed);
                    }
                }
            }
        }

        let result = server
            .request("tools/call", json!({ "name": tool_name, "arguments": args }), CALL_TIMEOUT)
            .await?;

        // Flatten MCP content blocks into text for the model.
        let mut out = String::new();
        if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
            for block in content {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("text") => out.push_str(block.get("text").and_then(|t| t.as_str()).unwrap_or("")),
                    Some("image") => out.push_str(&format!(
                        "[image returned: {}]",
                        block.get("mimeType").and_then(|m| m.as_str()).unwrap_or("image")
                    )),
                    Some("resource") => {
                        let r = block.get("resource").cloned().unwrap_or(Value::Null);
                        let uri = r.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                        match r.get("text").and_then(|t| t.as_str()) {
                            Some(text) => out.push_str(&format!("[resource {}]\n{}", uri, text)),
                            None => out.push_str(&format!("[resource {}]", uri)),
                        }
                    }
                    _ => out.push_str(&block.to_string()),
                }
                out.push('\n');
            }
        }
        if out.trim().is_empty() {
            if let Some(structured) = result.get("structuredContent") {
                out = structured.to_string();
            } else {
                out = result.to_string();
            }
        }
        let text = truncate(out.trim(), MAX_RESULT_CHARS);
        if result.get("isError").and_then(|e| e.as_bool()).unwrap_or(false) {
            return Err(text);
        }
        Ok(text)
    }

    pub async fn shutdown(&mut self) {
        for (_, mut server) in self.servers.drain() {
            server.kill().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_reduced_to_gemini_subset() {
        let raw = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "path": { "type": "string", "description": "File path" },
                "mode": { "type": ["string", "null"], "enum": ["a", "b"] },
                "opts": { "type": "object", "additionalProperties": true },
                "tags": { "type": "array", "items": { "anyOf": [{ "type": "null" }, { "type": "string" }] } }
            },
            "required": ["path", "missing"]
        });
        let (s, keys) = gemini_schema(&raw);
        assert!(s.get("$schema").is_none() && s.get("additionalProperties").is_none());
        assert_eq!(s["required"], json!(["path"]));
        assert_eq!(s["properties"]["mode"]["nullable"], json!(true));
        assert_eq!(s["properties"]["opts"]["type"], json!("string"));
        assert!(keys.contains("opts"));
        assert_eq!(s["properties"]["tags"]["items"]["type"], json!("string"));
    }

    #[test]
    fn long_names_fit_and_stay_unique() {
        let mut taken = HashSet::new();
        let a = api_name_for("my-very-long-server-name", "an_extremely_long_tool_name_that_goes_on_and_on", &taken);
        taken.insert(a.clone());
        assert!(a.len() <= MAX_TOOL_NAME);
        let b = api_name_for("my-very-long-server-name", "an_extremely_long_tool_name_that_goes_on_and_on", &taken);
        assert!(b.len() <= MAX_TOOL_NAME);
    }

    #[test]
    fn claude_desktop_config_is_accepted() {
        let v = normalize_config(json!({ "mcpServers": { "fs": { "command": "npx", "args": ["-y", "x"] } } })).unwrap();
        assert_eq!(v[0]["name"], json!("fs"));
        assert_eq!(v[0]["command"], json!("npx"));
    }
}
