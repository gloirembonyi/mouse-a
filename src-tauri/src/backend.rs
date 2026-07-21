// Mouse AI Desktop — Thothai Backend HTTP Client
// Mirrors the extension's shared/backend-client.js but in Rust.
// Session + backend URL persist to a small config file so sign-in
// survives app restarts.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

const DEFAULT_BACKEND_URL: &str = "https://backend-thothai.vercel.app";
// Fail fast when the server is unreachable instead of hanging the UI.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
// Total budget — Gemini calls with screenshots can legitimately take a while.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthUser {
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub picture: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub success: bool,
    #[serde(default)]
    pub user: Option<AuthUser>,
    #[serde(rename = "sessionToken", default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub success: bool,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(rename = "conversationId", default)]
    pub conversation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    #[serde(rename = "userText")]
    pub user_text: String,
    #[serde(rename = "apiKey", skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(rename = "imageDataUrl", skip_serializing_if = "Option::is_none")]
    pub image_data_url: Option<String>,
    #[serde(rename = "webSearchNeeded")]
    pub web_search_needed: bool,
    #[serde(rename = "conversationId", skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

// ── Persisted config ─────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredConfig {
    #[serde(default)]
    backend_url: Option<String>,
    #[serde(default)]
    session_token: Option<String>,
    #[serde(default)]
    user: Option<AuthUser>,
    /// Stable per-install id so guest sign-in always maps back to the same
    /// backend user/history without ever touching Google OAuth.
    #[serde(default)]
    device_id: Option<String>,
    /// Optional user-supplied Gemini API key (BYOK). Forwarded to the backend
    /// so heavy agent use isn't throttled by the shared free-tier key.
    #[serde(default)]
    gemini_key: Option<String>,
    /// Configured MCP servers (a JSON array of {name, command, args, env}),
    /// stored opaquely here and interpreted by the agent's MCP client.
    #[serde(default)]
    mcp_servers: Option<serde_json::Value>,
}

fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("mouse-ai").join("config.json"))
}

fn load_config() -> StoredConfig {
    config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(cfg: &StoredConfig) {
    if let Some(path) = config_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(cfg) {
            let _ = std::fs::write(path, json);
        }
    }
}

fn normalize_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

#[derive(Clone)]
pub struct BackendClient {
    client: Client,
    base_url: Arc<RwLock<String>>,
    session_token: Arc<RwLock<Option<String>>>,
    current_user: Arc<RwLock<Option<AuthUser>>>,
    device_id: Arc<RwLock<String>>,
    gemini_key: Arc<RwLock<Option<String>>>,
    mcp_servers: Arc<RwLock<serde_json::Value>>,
}

impl BackendClient {
    pub fn new(base_url: Option<String>) -> Self {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("Failed to create HTTP client");

        // Priority: explicit arg > env var > saved config > default.
        let stored = load_config();
        let url = base_url
            .or_else(|| std::env::var("MOUSE_AI_BACKEND_URL").ok())
            .or(stored.backend_url.clone())
            .map(|u| normalize_url(&u))
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| DEFAULT_BACKEND_URL.to_string());

        let device_id = stored
            .device_id
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if stored.device_id.as_deref() != Some(device_id.as_str()) {
            // First run on this install — persist the freshly generated id
            // immediately so a crash before the next save can't lose it.
            let mut cfg = stored.clone();
            cfg.device_id = Some(device_id.clone());
            save_config(&cfg);
        }

        Self {
            client,
            base_url: Arc::new(RwLock::new(url)),
            session_token: Arc::new(RwLock::new(stored.session_token)),
            current_user: Arc::new(RwLock::new(stored.user)),
            device_id: Arc::new(RwLock::new(device_id)),
            gemini_key: Arc::new(RwLock::new(
                stored.gemini_key.filter(|k| !k.trim().is_empty()),
            )),
            mcp_servers: Arc::new(RwLock::new(
                stored.mcp_servers.unwrap_or_else(|| serde_json::json!([])),
            )),
        }
    }

    async fn persist(&self) {
        let cfg = StoredConfig {
            backend_url: Some(self.base_url.read().await.clone()),
            session_token: self.session_token.read().await.clone(),
            user: self.current_user.read().await.clone(),
            device_id: Some(self.device_id.read().await.clone()),
            gemini_key: self.gemini_key.read().await.clone(),
            mcp_servers: Some(self.mcp_servers.read().await.clone()),
        };
        // Blocking file IO is tiny here; fine off the hot path.
        tokio::task::spawn_blocking(move || save_config(&cfg))
            .await
            .ok();
    }

    pub async fn get_backend_url(&self) -> String {
        self.base_url.read().await.clone()
    }

    pub async fn set_backend_url(&self, url: String) {
        let normalized = normalize_url(&url);
        if normalized.is_empty() {
            return;
        }
        *self.base_url.write().await = normalized;
        self.persist().await;
    }

    /// Whether a BYOK Gemini key is configured (never returns the key itself).
    pub async fn has_gemini_key(&self) -> bool {
        self.gemini_key.read().await.as_deref().map(|k| !k.trim().is_empty()).unwrap_or(false)
    }

    pub async fn set_gemini_key(&self, key: Option<String>) {
        let cleaned = key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
        *self.gemini_key.write().await = cleaned;
        self.persist().await;
    }

    /// The configured MCP servers as a JSON array (opaque to the backend client).
    pub async fn get_mcp_servers(&self) -> serde_json::Value {
        self.mcp_servers.read().await.clone()
    }

    pub async fn set_mcp_servers(&self, servers: serde_json::Value) {
        *self.mcp_servers.write().await = servers;
        self.persist().await;
    }

    async fn gemini_key_value(&self) -> Option<String> {
        self.gemini_key.read().await.clone()
    }

    pub async fn set_session(&self, token: String, user: AuthUser) {
        *self.session_token.write().await = Some(token);
        *self.current_user.write().await = Some(user);
        self.persist().await;
    }

    pub async fn clear_session(&self) {
        *self.session_token.write().await = None;
        *self.current_user.write().await = None;
        self.persist().await;
    }

    pub async fn get_session_token(&self) -> Option<String> {
        self.session_token.read().await.clone()
    }

    pub async fn get_current_user(&self) -> Option<AuthUser> {
        self.current_user.read().await.clone()
    }

    pub async fn is_signed_in(&self) -> bool {
        self.session_token.read().await.is_some()
    }

    fn friendly_network_error(e: &reqwest::Error) -> String {
        if e.is_connect() || e.is_timeout() {
            "Can't reach the Thothai backend. Check your internet, or set a working Backend URL in Settings.".to_string()
        } else {
            format!("Network error: {}", e)
        }
    }

    /// Quick reachability check against the backend's /api/health endpoint.
    pub async fn ping(&self) -> Result<String, String> {
        let base = self.base_url.read().await.clone();
        let url = format!("{}/api/health", base);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Self::friendly_network_error(&e))?;
        if resp.status().is_success() {
            Ok(base)
        } else {
            Err(format!(
                "Backend responded with HTTP {} — is {} the right URL?",
                resp.status().as_u16(),
                base
            ))
        }
    }

    /// Anonymous device sign-in — no Google OAuth involved. Used so the app
    /// is usable immediately; `google_sign_in` remains available separately
    /// for whoever wants to link a real account later.
    pub async fn guest_sign_in(&self) -> Result<AuthResponse, String> {
        let device_id = self.device_id.read().await.clone();
        let url = format!("{}/api/auth/guest", self.base_url.read().await);
        let body = serde_json::json!({ "deviceId": device_id });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Self::friendly_network_error(&e))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Guest sign-in failed ({}): {}", status, text));
        }

        let data: AuthResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse auth response: {}", e))?;

        if data.success {
            if let (Some(token), Some(user)) = (data.session_token.clone(), data.user.clone()) {
                self.set_session(token, user).await;
            }
        }

        Ok(data)
    }

    /// Sign in with a Google access token (obtained via OAuth flow).
    pub async fn google_sign_in(&self, access_token: &str) -> Result<AuthResponse, String> {
        let url = format!("{}/api/auth/google", self.base_url.read().await);
        let body = serde_json::json!({ "accessToken": access_token });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Self::friendly_network_error(&e))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Auth failed ({}): {}", status, text));
        }

        let data: AuthResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse auth response: {}", e))?;

        if data.success {
            if let (Some(token), Some(user)) = (data.session_token.clone(), data.user.clone()) {
                self.set_session(token, user).await;
            }
        }

        Ok(data)
    }

    /// Verify the current session.
    pub async fn get_me(&self) -> Result<AuthResponse, String> {
        let token = self.get_session_token().await.ok_or("Not signed in")?;
        let url = format!("{}/api/auth/me", self.base_url.read().await);

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| Self::friendly_network_error(&e))?;

        if !resp.status().is_success() {
            if resp.status().as_u16() == 401 {
                self.clear_session().await;
            }
            return Err("Session invalid".to_string());
        }

        let data: AuthResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        Ok(data)
    }

    /// Send a chat message to the Thothai backend.
    pub async fn chat(&self, mut request: ChatRequest) -> Result<ChatResponse, String> {
        let token = self
            .get_session_token()
            .await
            .ok_or("Not signed in — open the Mouse AI window (tray icon) and sign in first.")?;
        // BYOK: use the user's own Gemini key if they've set one.
        if request.api_key.is_none() {
            request.api_key = self.gemini_key_value().await;
        }
        let url = format!("{}/api/chat", self.base_url.read().await);

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&request)
            .send()
            .await
            .map_err(|e| Self::friendly_network_error(&e))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            if status == 401 {
                self.clear_session().await;
                return Err("Session expired — please sign in again.".to_string());
            }
            return Err(format!("Chat failed ({}): {}", status, truncate(&text, 300)));
        }

        let data: ChatResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse chat response: {}", e))?;

        Ok(data)
    }

    // ── Agent endpoints ──────────────────────────────────────────
    // These power the local agent loop (see agent.rs): the model's decision
    // step, plus server-side web search/fetch so research stays where the
    // Brave key and CORS-free fetching live.

    /// One agent decision step. Sends the running gemini-format `contents` and
    /// the tool catalog; returns Gemini's raw candidate `parts` (assistant text
    /// and/or functionCalls).
    pub async fn agent_step(
        &self,
        system: &str,
        contents: serde_json::Value,
        tools: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let token = self
            .get_session_token()
            .await
            .ok_or("Not signed in — open the Mouse AI window and try again.")?;
        let url = format!("{}/api/agent/step", self.base_url.read().await);

        let mut body = serde_json::json!({
            "system": system,
            "contents": contents,
            "tools": tools,
            "model": "gemini-2.5-flash",
        });
        // BYOK: forward the user's own Gemini key so the agent isn't throttled
        // by the shared free-tier key.
        if let Some(key) = self.gemini_key_value().await {
            body["apiKey"] = serde_json::Value::String(key);
        }

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await
            .map_err(|e| Self::friendly_network_error(&e))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            if status == 401 {
                self.clear_session().await;
                return Err("Session expired — please try again.".to_string());
            }
            return Err(format!("Agent step failed ({}): {}", status, truncate(&text, 300)));
        }

        resp.json()
            .await
            .map_err(|e| format!("Failed to parse agent step response: {}", e))
    }

    /// Server-side web search for the agent's web_search tool.
    pub async fn agent_search(&self, query: &str) -> Result<serde_json::Value, String> {
        let token = self.get_session_token().await.ok_or("Not signed in")?;
        let url = format!("{}/api/agent/search", self.base_url.read().await);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&serde_json::json!({ "query": query }))
            .send()
            .await
            .map_err(|e| Self::friendly_network_error(&e))?;
        if !resp.status().is_success() {
            return Err(format!("Search failed ({})", resp.status().as_u16()));
        }
        resp.json().await.map_err(|e| format!("Failed to parse search: {}", e))
    }

    /// Server-side page fetch+extract for the agent's web_fetch tool.
    pub async fn agent_fetch(&self, page_url: &str) -> Result<String, String> {
        let token = self.get_session_token().await.ok_or("Not signed in")?;
        let url = format!("{}/api/agent/fetch", self.base_url.read().await);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&serde_json::json!({ "url": page_url }))
            .send()
            .await
            .map_err(|e| Self::friendly_network_error(&e))?;
        if !resp.status().is_success() {
            return Err(format!("Fetch failed ({})", resp.status().as_u16()));
        }
        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse fetch: {}", e))?;
        Ok(data.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string())
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        s.chars().take(n).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}
