// Mouse AI Desktop — MCP (Model Context Protocol) client
//
// Lets the agent use tools from external MCP servers (filesystem, GitHub,
// databases, browser drivers, …) alongside its built-in tools. Servers are
// configured as stdio subprocesses; we speak newline-delimited JSON-RPC 2.0
// (`initialize` → `notifications/initialized` → `tools/list`, then `tools/call`
// on demand), exactly as the MCP stdio transport specifies.
//
// Design rule: MCP is strictly additive and fail-safe. If a server won't start,
// times out, or errors, it's skipped with a logged note and the agent proceeds
// on its built-in tools — MCP problems must never break the core loop.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

const INIT_TIMEOUT: Duration = Duration::from_secs(12);
const LIST_TIMEOUT: Duration = Duration::from_secs(20);
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
const PROTOCOL_VERSION: &str = "2024-11-05";

/// One configured MCP server (persisted in the app config).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

struct RunningServer {
    child: Child,
    stdin: ChildStdin,
    reader: Lines<BufReader<ChildStdout>>,
    next_id: i64,
}

impl RunningServer {
    async fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.write_line(&msg).await?;

        // Read lines until the response with our id arrives (skipping the
        // server's notifications/log lines), or we time out.
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(format!("MCP {} timed out", method));
            }
            let line = match tokio::time::timeout(remaining, self.reader.next_line()).await {
                Err(_) => return Err(format!("MCP {} timed out", method)),
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) => return Err("MCP server closed the connection".to_string()),
                Ok(Err(e)) => return Err(format!("MCP read error: {}", e)),
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                continue; // non-JSON stderr/log noise on stdout — ignore
            };
            if value.get("id").and_then(|v| v.as_i64()) != Some(id) {
                continue; // a notification or a different response
            }
            if let Some(err) = value.get("error") {
                return Err(format!("MCP error: {}", err));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.write_line(&msg).await
    }

    async fn write_line(&mut self, msg: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("MCP write error: {}", e))?;
        self.stdin.flush().await.map_err(|e| format!("MCP flush error: {}", e))
    }
}

/// A tool discovered from an MCP server, plus the routing info to call it.
pub struct McpTool {
    /// Gemini-safe function name: `mcp__<server>__<tool>`.
    pub api_name: String,
    pub server: String,
    pub tool: String,
    pub description: String,
    pub input_schema: Value,
}

pub struct McpManager {
    servers: HashMap<String, RunningServer>,
    tools: Vec<McpTool>,
}

fn sanitize(part: &str) -> String {
    part.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

impl McpManager {
    /// Start every configured server, run the handshake, and collect their
    /// tools. Returns the manager plus a list of human-readable status notes
    /// (one per server) for the activity log. Never fails as a whole.
    pub async fn start(configs: &[McpServerConfig]) -> (Self, Vec<String>) {
        let mut manager = McpManager { servers: HashMap::new(), tools: Vec::new() };
        let mut notes = Vec::new();

        for cfg in configs {
            match manager.start_one(cfg).await {
                Ok(n) => notes.push(format!("MCP '{}': {} tool(s)", cfg.name, n)),
                Err(e) => notes.push(format!("MCP '{}' skipped: {}", cfg.name, e)),
            }
        }
        (manager, notes)
    }

    async fn start_one(&mut self, cfg: &McpServerConfig) -> Result<usize, String> {
        let mut command = Command::new(&cfg.command);
        command
            .args(&cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for (k, v) in &cfg.env {
            command.env(k, v);
        }

        let mut child = command
            .spawn()
            .map_err(|e| format!("couldn't start '{}': {}", cfg.command, e))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let reader = BufReader::new(stdout).lines();

        let mut server = RunningServer { child, stdin, reader, next_id: 0 };

        // Handshake.
        let init_params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "mouse-ai-desktop", "version": "0.1.0" }
        });
        server.request("initialize", init_params, INIT_TIMEOUT).await?;
        let _ = server.notify("notifications/initialized", json!({})).await;

        // Discover tools.
        let list = server.request("tools/list", json!({}), LIST_TIMEOUT).await?;
        let tools = list.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        let mut count = 0;
        for t in tools {
            let Some(name) = t.get("name").and_then(|n| n.as_str()) else { continue };
            let api_name = format!("mcp__{}__{}", sanitize(&cfg.name), sanitize(name));
            self.tools.push(McpTool {
                api_name,
                server: cfg.name.clone(),
                tool: name.to_string(),
                description: t
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string(),
                input_schema: t.get("inputSchema").cloned().unwrap_or_else(|| json!({ "type": "object" })),
            });
            count += 1;
        }

        self.servers.insert(cfg.name.clone(), server);
        Ok(count)
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
                let mut schema = t.input_schema.clone();
                if !schema.is_object() {
                    schema = json!({ "type": "object" });
                }
                json!({
                    "name": t.api_name,
                    "description": format!("[MCP:{}] {}", t.server, t.description),
                    "parameters": schema
                })
            })
            .collect()
    }

    /// Invoke an MCP tool by its `mcp__server__tool` api name.
    pub async fn call(&mut self, api_name: &str, args: Value) -> Result<String, String> {
        let (server_name, tool_name) = match self
            .tools
            .iter()
            .find(|t| t.api_name == api_name)
            .map(|t| (t.server.clone(), t.tool.clone()))
        {
            Some(pair) => pair,
            None => return Err(format!("unknown MCP tool: {}", api_name)),
        };
        let server = self
            .servers
            .get_mut(&server_name)
            .ok_or_else(|| format!("MCP server '{}' not running", server_name))?;

        let result = server
            .request("tools/call", json!({ "name": tool_name, "arguments": args }), CALL_TIMEOUT)
            .await?;

        // Flatten MCP content blocks into text for the model.
        let mut out = String::new();
        if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
            for block in content {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    out.push_str(text);
                    out.push('\n');
                } else {
                    out.push_str(&block.to_string());
                    out.push('\n');
                }
            }
        }
        if out.trim().is_empty() {
            out = result.to_string();
        }
        Ok(out.trim().to_string())
    }

    pub async fn shutdown(&mut self) {
        for (_, mut server) in self.servers.drain() {
            let _ = server.child.start_kill();
        }
    }
}
