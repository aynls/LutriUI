// In-process MCP server: official rmcp Streamable HTTP transport on
// loopback, bearer authentication, a bounded bridge to the live WebView
// workspace, and direct LibraryStore access for gallery/task reads.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener as StdListener};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock, Weak};

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::Router;
use keyring::Entry;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData, Implementation,
    ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::ServerHandler;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use subtle::ConstantTimeEq;
use tauri::{AppHandle, Emitter};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tower_http::limit::RequestBodyLimitLayer;
use uuid::Uuid;

use crate::history::HistoryItem;
use crate::library::LibraryStore;
use crate::library_access::with_library;

pub const DEFAULT_PORT: u16 = 39631;
pub const MIN_PORT: u16 = 1024;
pub const MAX_PORT: u16 = 65535;
const MAX_PENDING: usize = 32;
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const BODY_LIMIT: usize = 96 * 1024 * 1024;
const KEYRING_SERVICE: &str = "app.lutriui.mcp";
const KEYRING_USER: &str = "access-token";
const MCP_EVENT: &str = "mcp-request";
const STATUS_EVENT: &str = "mcp-status-changed";
pub const LIBRARY_EVENT: &str = "mcp-library-changed";
const MAIN_WINDOW: &str = "main";

// ---------- Published tool contract (shared/mcp-tools.json) ----------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolsFile {
    instructions: String,
    tools: Vec<Tool>,
}

/// Published tools plus one compiled Draft-2020-12 validator per inputSchema.
pub struct ToolRegistry {
    instructions: String,
    tools: Vec<Tool>,
    validators: Vec<jsonschema::Validator>,
}

static TOOLS: OnceLock<Result<ToolRegistry, String>> = OnceLock::new();

fn tools_file() -> Result<&'static ToolRegistry, String> {
    TOOLS
        .get_or_init(|| {
            let parsed: ToolsFile =
                serde_json::from_str(include_str!("../../shared/mcp-tools.json"))
                    .map_err(|e| format!("invalid mcp-tools.json: {e}"))?;
            let mut validators = Vec::with_capacity(parsed.tools.len());
            for tool in &parsed.tools {
                let schema = Value::Object(tool.input_schema.as_ref().clone());
                let validator = jsonschema::options()
                    .with_draft(jsonschema::Draft::Draft202012)
                    .should_validate_formats(true)
                    .build(&schema)
                    .map_err(|e| format!("invalid schema for {}: {e}", tool.name))?;
                validators.push(validator);
            }
            Ok(ToolRegistry {
                instructions: parsed.instructions,
                tools: parsed.tools,
                validators,
            })
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// Arguments are checked against the published schema before dispatch; the
/// error carries a JSON-pointer path only, never the offending value (which
/// may be a multi-megabyte data URL).
fn validate_args(
    name: &str,
    validator: &jsonschema::Validator,
    args: &Value,
) -> Result<(), ErrorData> {
    if let Err(e) = validator.validate(args) {
        let path = e.instance_path().to_string();
        return Err(ErrorData::invalid_params(
            format!(
                "invalid arguments for {name} at {}",
                if path.is_empty() { "/" } else { &path }
            ),
            None,
        ));
    }
    Ok(())
}

// ---------- Dispatcher seam ----------

pub type McpDispatcher = Arc<
    dyn Fn(String, Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send>>
        + Send
        + Sync,
>;

pub type ToolResult = Result<ToolOutput, ToolError>;

#[derive(Debug)]
pub struct ToolOutput {
    pub structured: Value,
    pub images: Vec<(String, String)>, // (base64, mime)
}

#[derive(Debug, Clone)]
pub struct ToolError {
    pub code: String,
    pub message: String,
    pub details: Option<Value>,
}

impl ToolError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            details: None,
        }
    }
    fn with_details(code: &str, message: impl Into<String>, details: Option<Value>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            details,
        }
    }
}

/// The reply the main window posts back for a bridge request.
pub struct BridgeReply {
    pub result: Option<Value>,
    pub error: Option<ToolError>,
}

fn tool_response(output: ToolOutput) -> CallToolResponse {
    let mut content = vec![ContentBlock::text(output.structured.to_string())];
    for (data, mime) in output.images {
        content.push(ContentBlock::image(data, mime));
    }
    let mut result = CallToolResult::success(content);
    result.structured_content = Some(output.structured);
    result.into()
}

fn error_response(err: ToolError) -> CallToolResponse {
    let mut payload = Map::new();
    payload.insert("code".into(), Value::String(err.code));
    payload.insert("message".into(), Value::String(err.message));
    if let Some(details) = err.details {
        payload.insert("details".into(), details);
    }
    let payload = Value::Object(payload);
    let mut result = CallToolResult::error(vec![ContentBlock::text(payload.to_string())]);
    result.structured_content = Some(payload);
    result.into()
}

// ---------- HTTP boundary ----------

struct Auth {
    token: String,
    port: u16,
}
type SharedAuth = Arc<RwLock<Auth>>;

fn allowed_host(host: &str, port: u16) -> bool {
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

fn allowed_origin(origin: &str, port: u16) -> bool {
    allowed_host(origin.strip_prefix("http://").unwrap_or(""), port)
}

/// Bearer + Host + Origin checks run before any MCP method is dispatched.
async fn boundary(
    State(auth): State<SharedAuth>,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    let (auth_port, token) = {
        let a = auth
            .read()
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "auth state"))?;
        (a.port, a.token.clone())
    };
    let headers = req.headers();
    let host_ok = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| allowed_host(v, auth_port));
    if !host_ok {
        return Err((StatusCode::FORBIDDEN, "Host not allowed"));
    }
    // A present Origin must be valid UTF-8 and a loopback origin; absent is ok.
    if let Some(origin) = headers.get(header::ORIGIN) {
        match origin.to_str() {
            Ok(v) if allowed_origin(v, auth_port) => {}
            _ => return Err((StatusCode::FORBIDDEN, "Origin not allowed")),
        }
    }
    let provided = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let ok = match provided {
        Some(p) if p.len() == token.len() => p.as_bytes().ct_eq(token.as_bytes()).into(),
        _ => false,
    };
    if !ok {
        return Err((StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    Ok(next.run(req).await)
}

struct McpHandler {
    dispatcher: McpDispatcher,
}

impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new("LutriUI", env!("CARGO_PKG_VERSION"));
        info.instructions = tools_file().map(|t| t.instructions.clone()).ok();
        info
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<rmcp::model::ListToolsResult, ErrorData>> + Send + '_
    {
        std::future::ready(match tools_file() {
            Ok(t) => Ok(rmcp::model::ListToolsResult {
                tools: t.tools.clone(),
                ..Default::default()
            }),
            Err(e) => Err(ErrorData::internal_error(e, None)),
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools_file()
            .ok()?
            .tools
            .iter()
            .find(|t| t.name.as_ref() == name)
            .cloned()
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResponse, ErrorData>> + Send + '_ {
        let dispatcher = self.dispatcher.clone();
        async move {
            let registry = match tools_file() {
                Ok(t) => t,
                Err(e) => return Err(ErrorData::internal_error(e.clone(), None)),
            };
            let name = request.name.to_string();
            let index = match registry.tools.iter().position(|t| t.name.as_ref() == name) {
                Some(i) => i,
                None => {
                    return Err(ErrorData::invalid_params(
                        format!("unknown tool {name}"),
                        None,
                    ))
                }
            };
            let args = Value::Object(request.arguments.unwrap_or_default());
            validate_args(&name, &registry.validators[index], &args)?;
            match dispatcher(name, args).await {
                Ok(out) => Ok(tool_response(out)),
                Err(e) => Ok(error_response(e)),
            }
        }
    }
}

fn make_service(
    dispatcher: McpDispatcher,
    cancel: CancellationToken,
) -> StreamableHttpService<McpHandler, LocalSessionManager> {
    let mut config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_max_request_body_bytes(BODY_LIMIT);
    config.cancellation_token = cancel.child_token();
    StreamableHttpService::new(
        move || {
            Ok(McpHandler {
                dispatcher: dispatcher.clone(),
            })
        },
        Arc::new(LocalSessionManager::default()),
        config,
    )
}

fn build_router(auth: SharedAuth, dispatcher: McpDispatcher, cancel: CancellationToken) -> Router {
    Router::new()
        .route_service("/mcp", make_service(dispatcher, cancel))
        .route_layer(middleware::from_fn_with_state(auth, boundary))
        .layer(RequestBodyLimitLayer::new(BODY_LIMIT))
}

// ---------- WebView bridge ----------

struct Pending {
    claimed: bool,
    /// idempotencyKey/expectedVersion hints for ambiguous outcomes; never bodies.
    hint: Value,
    reply: oneshot::Sender<ToolResult>,
}

struct BridgeState {
    instance: Option<String>,
    pending: HashMap<String, Pending>,
}

/// Removes the pending entry if the dispatch future is canceled or dropped
/// (shutdown, server swap) so dead futures cannot exhaust the bounded map.
struct PendingGuard {
    state: Weak<Mutex<BridgeState>>,
    request_id: String,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            if let Ok(mut s) = state.lock() {
                s.pending.remove(&self.request_id);
            }
        }
    }
}

/// Serializable event payload the main WebviewWindow consumes.
#[derive(Debug, Clone, serde::Serialize)]
struct BridgeEvent {
    #[serde(rename = "requestId")]
    request_id: String,
    #[serde(rename = "instanceId")]
    instance_id: String,
    operation: String,
    args: Value,
    #[serde(rename = "deadlineMs")]
    deadline_ms: u64,
}

pub struct Bridge {
    app: OnceLock<AppHandle>,
    state: Arc<Mutex<BridgeState>>,
}

fn lock<'a, T>(m: &'a Mutex<T>) -> Result<MutexGuard<'a, T>, ToolError> {
    m.lock()
        .map_err(|_| ToolError::new("INTERNAL", "bridge state poisoned"))
}

/// idempotencyKey/expectedVersion are safe to echo in errors; bodies are not.
fn pending_hint(args: &Value) -> Value {
    let mut hint = Map::new();
    for key in ["idempotencyKey", "expectedVersion"] {
        if let Some(v) = args.get(key).filter(|v| v.is_string()) {
            hint.insert(key.to_string(), v.clone());
        }
    }
    if hint.is_empty() {
        Value::Null
    } else {
        Value::Object(hint)
    }
}

/// Once the UI claimed a request its outcome is unknowable here; unclaimed
/// work is reported with the transport-level code instead.
fn fail_pending(pending: Pending, unclaimed_code: &str, message: &str) {
    let code = if pending.claimed {
        "OUTCOME_UNKNOWN"
    } else {
        unclaimed_code
    };
    let details = if pending.hint.is_null() {
        None
    } else {
        Some(pending.hint)
    };
    let _ = pending
        .reply
        .send(Err(ToolError::with_details(code, message, details)));
}

impl Bridge {
    fn new() -> Self {
        Self {
            app: OnceLock::new(),
            state: Arc::new(Mutex::new(BridgeState {
                instance: None,
                pending: HashMap::new(),
            })),
        }
    }

    fn attach(&self, app: &AppHandle) {
        let _ = self.app.set(app.clone());
    }

    fn ui_ready(&self) -> bool {
        self.state
            .lock()
            .map(|s| s.instance.is_some())
            .unwrap_or(false)
    }

    /// A different instance replaces the registration and cancels all pending
    /// work; re-registering the same instance is a no-op (StrictMode remounts).
    pub fn register(&self, instance: String) {
        let stale: Vec<Pending> = {
            let mut s = match self.state.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            if s.instance.as_deref() == Some(instance.as_str()) {
                return;
            }
            s.instance = Some(instance);
            s.pending.drain().map(|(_, p)| p).collect()
        };
        for p in stale {
            fail_pending(p, "UI_NOT_READY", "the UI bridge was replaced");
        }
    }

    /// Only a matching instance unregisters; its pending work is answered so
    /// callers learn claimed-vs-unclaimed status instead of timing out.
    pub fn unregister(&self, instance: &str) {
        let stale: Vec<Pending> = {
            let mut s = match self.state.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            if s.instance.as_deref() != Some(instance) {
                return;
            }
            s.instance = None;
            s.pending.drain().map(|(_, p)| p).collect()
        };
        for p in stale {
            fail_pending(p, "UI_NOT_READY", "the UI bridge disconnected");
        }
    }

    /// The frontend claims a queued request right before dispatch; claimed
    /// work may still reply after disconnect and is reported as unknown.
    pub fn claim(&self, request_id: &str, instance: &str) -> bool {
        let mut s = match self.state.lock() {
            Ok(s) => s,
            Err(_) => return false,
        };
        if s.instance.as_deref() != Some(instance) {
            return false;
        }
        match s.pending.get_mut(request_id) {
            Some(p) if !p.claimed => {
                p.claimed = true;
                true
            }
            _ => false,
        }
    }

    pub fn reply(&self, request_id: &str, instance: &str, reply: BridgeReply) {
        let pending = self
            .state
            .lock()
            .ok()
            .filter(|s| s.instance.as_deref() == Some(instance))
            .and_then(|mut s| s.pending.remove(request_id));
        if let Some(p) = pending {
            let result = match (reply.result, reply.error) {
                (_, Some(error)) => Err(error),
                (Some(value), None) => Ok(ToolOutput {
                    structured: value.get("structured").cloned().unwrap_or(value.clone()),
                    images: value
                        .get("content")
                        .and_then(Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter(|item| {
                                    item.get("type").and_then(Value::as_str) == Some("image")
                                })
                                .filter_map(|item| {
                                    Some((
                                        item.get("data")?.as_str()?.to_string(),
                                        item.get("mimeType")?.as_str()?.to_string(),
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                }),
                (None, None) => Err(ToolError::new("INTERNAL", "the UI returned an empty reply")),
            };
            let _ = p.reply.send(result);
        }
    }

    /// Queued work that was never claimed can still be dropped cheaply
    /// (disable, stop); a paid task already submitted is never touched.
    pub fn cancel_unclaimed(&self, code: &str, message: &str) {
        let dropped: Vec<Pending> = self
            .state
            .lock()
            .map(|mut s| {
                let ids: Vec<String> = s
                    .pending
                    .iter()
                    .filter(|(_, p)| !p.claimed)
                    .map(|(id, _)| id.clone())
                    .collect();
                ids.iter().filter_map(|id| s.pending.remove(id)).collect()
            })
            .unwrap_or_default();
        for p in dropped {
            fail_pending(p, code, message);
        }
    }

    fn cancel_all(&self, code: &str, message: &str) {
        let stale: Vec<Pending> = self
            .state
            .lock()
            .map(|mut s| s.pending.drain().map(|(_, p)| p).collect())
            .unwrap_or_default();
        for p in stale {
            fail_pending(p, code, message);
        }
    }

    pub async fn dispatch(&self, operation: String, args: Value) -> ToolResult {
        let request_id = Uuid::new_v4().to_string();
        let hint = pending_hint(&args);
        let (tx, rx) = oneshot::channel();
        let instance = {
            let mut s = lock(&self.state)?;
            let instance = s.instance.clone().ok_or_else(|| {
                ToolError::new("UI_NOT_READY", "the workspace UI is not connected")
            })?;
            if s.pending.len() >= MAX_PENDING {
                return Err(ToolError::new(
                    "UI_NOT_READY",
                    "too many pending UI requests",
                ));
            }
            s.pending.insert(
                request_id.clone(),
                Pending {
                    claimed: false,
                    hint: hint.clone(),
                    reply: tx,
                },
            );
            instance
        };
        // Dropping this future (server stop/swap) must not leak the entry.
        let _guard = PendingGuard {
            state: Arc::downgrade(&self.state),
            request_id: request_id.clone(),
        };
        let app = self
            .app
            .get()
            .ok_or_else(|| ToolError::new("UI_NOT_READY", "the app is not attached"))?
            .clone();
        let deadline_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64 + REQUEST_TIMEOUT.as_millis() as u64)
            .unwrap_or(0);
        let event = BridgeEvent {
            request_id: request_id.clone(),
            instance_id: instance,
            operation,
            args,
            deadline_ms,
        };
        if app.emit_to(MAIN_WINDOW, MCP_EVENT, event).is_err() {
            let pending = lock(&self.state)
                .map(|mut s| s.pending.remove(&request_id))
                .unwrap_or(None);
            if let Some(p) = pending {
                fail_pending(p, "UI_NOT_READY", "the workspace UI is not connected");
            }
            return Err(ToolError::new(
                "UI_NOT_READY",
                "the workspace UI is not connected",
            ));
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(ToolError::new(
                "OUTCOME_UNKNOWN",
                "the UI request was interrupted after it was claimed",
            )),
            Err(_) => {
                let pending = lock(&self.state)
                    .map(|mut s| s.pending.remove(&request_id))
                    .unwrap_or(None);
                match pending {
                    Some(p) => {
                        let claimed = p.claimed;
                        let details = if p.hint.is_null() {
                            None
                        } else {
                            Some(p.hint.clone())
                        };
                        let _ = p.reply.send(Err(ToolError::new("UI_NOT_READY", "timeout")));
                        Err(ToolError::with_details(
                            if claimed {
                                "OUTCOME_UNKNOWN"
                            } else {
                                "UI_NOT_READY"
                            },
                            if claimed {
                                "the request timed out after it was claimed; check the task before retrying"
                            } else {
                                "the workspace UI did not respond in time"
                            },
                            details,
                        ))
                    }
                    None => Err(ToolError::new(
                        "OUTCOME_UNKNOWN",
                        "the request outcome is unknown",
                    )),
                }
            }
        }
    }
}

// ---------- Controller ----------

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct McpPrefs {
    enabled: bool,
    port: u16,
}

impl Default for McpPrefs {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    pub url: Option<String>,
    pub has_token: bool,
    pub ui_ready: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConnection {
    pub url: String,
    pub token: String,
}

/// Command result for `mcp_configure`. `issued_token` is set only when this
/// enable minted a credential (or is delivering one minted by a previous
/// failed enable). It is omitted from status events.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfigureResult {
    #[serde(flatten)]
    pub status: McpStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issued_token: Option<String>,
}

struct Running {
    id: u64,
    port: u16,
    cancel: CancellationToken,
    task: tauri::async_runtime::JoinHandle<()>,
}

static NEXT_SERVER_ID: AtomicU64 = AtomicU64::new(1);

struct McpInner {
    auth: SharedAuth,
    prefs: McpPrefs,
    server: Option<Running>,
    error: Option<String>,
    /// Minted by a user-initiated enable and not yet handed back to settings.
    /// A failed start keeps it for the next successful enable. Status events
    /// never include it.
    unshown_token: Option<String>,
}

pub struct Mcp {
    pub bridge: Bridge,
    inner: Arc<Mutex<McpInner>>,
    /// Serializes configure/start/stop/rotate so a half-applied change can
    /// never leave a new listener on the old token or a dead config.
    lifecycle: Mutex<()>,
    config_dir: PathBuf,
    app: OnceLock<AppHandle>,
}

fn prefs_path(dir: &std::path::Path) -> PathBuf {
    dir.join("mcp.json")
}

fn load_prefs(dir: &std::path::Path) -> (McpPrefs, Option<String>) {
    match std::fs::read(prefs_path(dir)) {
        Ok(bytes) => match serde_json::from_slice::<McpPrefs>(&bytes) {
            Ok(p) if !(MIN_PORT..=MAX_PORT).contains(&p.port) => (
                McpPrefs::default(),
                Some("stored MCP port is out of range".into()),
            ),
            Ok(p) => (p, None),
            Err(e) => (
                McpPrefs::default(),
                Some(format!("invalid MCP config: {e}")),
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (McpPrefs::default(), None),
        Err(e) => (
            McpPrefs::default(),
            Some(format!("cannot read MCP config: {e}")),
        ),
    }
}

fn save_prefs(dir: &std::path::Path, prefs: &McpPrefs) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create config dir: {e}"))?;
    let payload = serde_json::to_vec_pretty(prefs).map_err(|e| e.to_string())?;
    let tmp = dir.join("mcp.json.tmp");
    std::fs::write(&tmp, payload).map_err(|e| format!("cannot write MCP config: {e}"))?;
    std::fs::rename(&tmp, prefs_path(dir)).map_err(|e| format!("cannot save MCP config: {e}"))
}

fn generate_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// Returns the keyring token and whether this call minted it.
fn ensure_token() -> Result<(String, bool), String> {
    let entry = Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| e.to_string())?;
    match entry.get_password() {
        Ok(t) if !t.is_empty() => Ok((t, false)),
        Ok(_) | Err(keyring::Error::NoEntry) => {
            let token = generate_token();
            entry
                .set_password(&token)
                .map_err(|e| format!("cannot store MCP token: {e}"))?;
            Ok((token, true))
        }
        Err(e) => Err(format!("cannot read MCP token: {e}")),
    }
}

fn status_of(inner: &McpInner, ui_ready: bool) -> McpStatus {
    McpStatus {
        enabled: inner.prefs.enabled,
        running: inner.server.is_some(),
        port: inner.prefs.port,
        url: inner
            .server
            .is_some()
            .then(|| format!("http://127.0.0.1:{}/mcp", inner.prefs.port)),
        has_token: Entry::new(KEYRING_SERVICE, KEYRING_USER)
            .and_then(|e| e.get_password())
            .is_ok(),
        ui_ready,
        error: inner.error.clone(),
    }
}

impl Mcp {
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            bridge: Bridge::new(),
            inner: Arc::new(Mutex::new(McpInner {
                auth: Arc::new(RwLock::new(Auth {
                    token: String::new(),
                    port: DEFAULT_PORT,
                })),
                prefs: McpPrefs::default(),
                server: None,
                error: None,
                unshown_token: None,
            })),
            lifecycle: Mutex::new(()),
            config_dir,
            app: OnceLock::new(),
        }
    }

    /// Loads config and starts the server when enabled. A corrupt config or
    /// missing keyring leaves MCP off with a visible error instead of failing
    /// app startup.
    pub fn setup(&self, app: &AppHandle) {
        let _ = self.app.set(app.clone());
        self.bridge.attach(app);
        let (prefs, load_error) = load_prefs(&self.config_dir);
        let start_port = prefs.enabled.then_some(prefs.port);
        {
            if let Ok(mut inner) = self.inner.lock() {
                inner.prefs = prefs;
                inner.error = load_error;
            }
        }
        if let Some(port) = start_port {
            let _lifecycle = self.lifecycle.lock().ok();
            if let Err(e) = self.start_prepared(app, port, false) {
                if let Ok(mut inner) = self.inner.lock() {
                    inner.error = Some(e);
                    inner.prefs.enabled = false;
                }
            }
        }
        self.emit_status(app);
    }

    pub fn emit_status(&self, app: &AppHandle) {
        if let Ok(inner) = self.inner.lock() {
            let _ = app.emit(STATUS_EVENT, status_of(&inner, self.bridge.ui_ready()));
        }
    }

    pub fn status(&self) -> McpStatus {
        self.inner
            .lock()
            .map(|inner| status_of(&inner, self.bridge.ui_ready()))
            .unwrap_or_else(|_| McpStatus {
                enabled: false,
                running: false,
                port: DEFAULT_PORT,
                url: None,
                has_token: false,
                ui_ready: false,
                error: Some("MCP state unavailable".into()),
            })
    }

    /// Prepares token + listener + persisted config first, then swaps
    /// auth/server in one step so a failure leaves the old endpoint working
    /// with its own auth. Caller must hold the lifecycle lock.
    ///
    /// `reveal_minted` holds a newly created token until `configure` can
    /// return it to the settings page. Startup passes false so a token minted
    /// before the window exists is not forced onto the UI later.
    fn start_prepared(&self, app: &AppHandle, port: u16, reveal_minted: bool) -> Result<(), String> {
        let same_running = {
            let inner = self
                .inner
                .lock()
                .map_err(|_| "MCP state unavailable".to_string())?;
            inner.prefs.enabled
                && inner
                    .server
                    .as_ref()
                    .is_some_and(|s| s.port == port)
        };
        if same_running {
            return Ok(());
        }
        let (token, created) = ensure_token()?;
        if created && reveal_minted {
            self.inner
                .lock()
                .map_err(|_| "MCP state unavailable".to_string())?
                .unshown_token = Some(token.clone());
        }
        let listener = StdListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))
            .map_err(|e| format!("cannot bind 127.0.0.1:{port}: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("listener failed: {e}"))?;
        tools_file()?; // compile schemas before replacing a working server
                       // Listener-specific auth; the running endpoint keeps its own.
        let auth: SharedAuth = Arc::new(RwLock::new(Auth { token, port }));
        save_prefs(
            &self.config_dir,
            &McpPrefs {
                enabled: true,
                port,
            },
        )?;
        let cancel = CancellationToken::new();
        let router = build_router(auth.clone(), self.ui_dispatcher(), cancel.child_token());
        let (exit_tx, exit_rx) = oneshot::channel::<String>();
        let serve_cancel = cancel.clone();
        let task = tauri::async_runtime::spawn(async move {
            let listener = match TcpListener::from_std(listener) {
                Ok(l) => l,
                Err(e) => {
                    let _ = exit_tx.send(format!("listener failed: {e}"));
                    return;
                }
            };
            tokio::select! {
                _ = serve_cancel.cancelled() => {}
                result = axum::serve(listener, router.into_make_service()) => {
                    let reason = match result {
                        Ok(()) => "MCP server stopped unexpectedly".to_string(),
                        Err(e) => format!("MCP server failed: {e}"),
                    };
                    let _ = exit_tx.send(reason);
                }
            }
        });
        let server_id = NEXT_SERVER_ID.fetch_add(1, Ordering::Relaxed);
        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| "MCP state unavailable".to_string())?;
            if let Some(old) = inner.server.take() {
                old.cancel.cancel();
                old.task.abort();
            }
            inner.auth = auth;
            inner.server = Some(Running {
                id: server_id,
                port,
                cancel,
                task,
            });
            inner.prefs = McpPrefs {
                enabled: true,
                port,
            };
            inner.error = None;
        }
        // Reflect an unexpected serve failure as running:false + error/status.
        let inner_arc = self.inner.clone();
        let bridge_state = self.bridge.state.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Ok(reason) = exit_rx.await {
                let mut inner = match inner_arc.lock() {
                    Ok(i) => i,
                    Err(_) => return,
                };
                if inner.server.as_ref().is_some_and(|s| s.id == server_id) {
                    inner.server = None;
                    inner.error = Some(reason);
                    let status = status_of(
                        &inner,
                        bridge_state
                            .lock()
                            .map(|s| s.instance.is_some())
                            .unwrap_or(false),
                    );
                    drop(inner);
                    let _ = app.emit(STATUS_EVENT, status);
                }
            }
        });
        Ok(())
    }

    /// Caller must hold the lifecycle lock.
    fn stop_running(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(running) = inner.server.take() {
                running.cancel.cancel();
                running.task.abort();
            }
            inner.prefs.enabled = false;
        }
    }

    pub async fn configure(
        &self,
        app: &AppHandle,
        enabled: bool,
        port: u16,
    ) -> Result<McpConfigureResult, String> {
        if !(MIN_PORT..=MAX_PORT).contains(&port) {
            return Err(format!("port must be between {MIN_PORT} and {MAX_PORT}"));
        }
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| "MCP lifecycle unavailable".to_string())?;
        let issued_token = if enabled {
            self.start_prepared(app, port, true)?;
            self.inner
                .lock()
                .map_err(|_| "MCP state unavailable".to_string())?
                .unshown_token
                .take()
        } else {
            // Persist the disabled config first; a write failure keeps the
            // current server running rather than silently changing state.
            save_prefs(
                &self.config_dir,
                &McpPrefs {
                    enabled: false,
                    port,
                },
            )?;
            self.stop_running();
            if let Ok(mut inner) = self.inner.lock() {
                inner.prefs = McpPrefs {
                    enabled: false,
                    port,
                };
            }
            self.bridge
                .cancel_unclaimed("UI_NOT_READY", "the MCP server was disabled");
            None
        };
        self.emit_status(app);
        Ok(McpConfigureResult {
            status: self.status(),
            issued_token,
        })
    }

    /// Reveal/copy path: returns the endpoint plus the keyring token. Never
    /// logged or emitted.
    pub fn connection(&self) -> Result<McpConnection, String> {
        let port = self
            .inner
            .lock()
            .map_err(|_| "MCP state unavailable".to_string())?
            .prefs
            .port;
        let token = Entry::new(KEYRING_SERVICE, KEYRING_USER)
            .and_then(|e| e.get_password())
            .map_err(|e| format!("cannot read MCP token: {e}"))?;
        Ok(McpConnection {
            url: format!("http://127.0.0.1:{port}/mcp"),
            token,
        })
    }

    /// Rotates the token only after the secure-store write succeeds, then the
    /// active listener auth is swapped so the old bearer fails on next request.
    pub fn rotate_token(&self, app: &AppHandle) -> Result<McpStatus, String> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| "MCP lifecycle unavailable".to_string())?;
        let token = generate_token();
        Entry::new(KEYRING_SERVICE, KEYRING_USER)
            .and_then(|e| e.set_password(&token))
            .map_err(|e| format!("cannot store MCP token: {e}"))?;
        if let Ok(mut inner) = self.inner.lock() {
            inner.unshown_token = None;
            if let Ok(mut auth) = inner.auth.write() {
                auth.token = token;
            }
        }
        // Queued-but-unclaimed bridge work is dropped; claimed work may still
        // legitimately reply.
        self.bridge
            .cancel_unclaimed("UI_NOT_READY", "the MCP token was rotated");
        self.emit_status(app);
        Ok(self.status())
    }

    pub fn shutdown(&self) {
        {
            let _lifecycle = match self.lifecycle.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            self.stop_running();
        }
        self.bridge
            .cancel_all("UI_NOT_READY", "the application is shutting down");
    }

    // ---------- Dispatch plumbing ----------

    fn ui_dispatcher(&self) -> McpDispatcher {
        let bridge_state = self.bridge.state.clone();
        let app = self.app.get().cloned();
        Arc::new(move |operation: String, args: Value| {
            let app = app.clone();
            let bridge_state = bridge_state.clone();
            Box::pin(async move {
                let app = match &app {
                    Some(app) => app.clone(),
                    None => return Err(ToolError::new("UI_NOT_READY", "the app is not attached")),
                };
                match operation.as_str() {
                    "task_get" => native_task_get(&app, args).await,
                    "gallery_query" => native_gallery_query(&app, args).await,
                    "gallery_read" => native_gallery_read(&app, args).await,
                    "gallery_import" => native_gallery_import(&app, args).await,
                    "gallery_board" => native_gallery_board(&app, args).await,
                    _ => {
                        let bridge = Bridge {
                            app: OnceLock::new(),
                            state: bridge_state,
                        };
                        let _ = bridge.app.set(app);
                        bridge.dispatch(operation, args).await
                    }
                }
            })
        })
    }
}

// ---------- Native library-backed tools ----------

fn project_gallery(item: &crate::gallery::GalleryItem) -> Value {
    let mut value = serde_json::to_value(item).unwrap_or(Value::Null);
    if let Some(map) = value.as_object_mut() {
        map.remove("filePath");
    }
    value
}

async fn with_store<T: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce(&mut LibraryStore) -> Result<T, String> + Send + 'static,
) -> Result<T, ToolError> {
    with_library(app.clone(), f).await.map_err(|e| {
        let code = if e.contains("migrating") || e.contains("migration") {
            "UI_BUSY"
        } else {
            "LIBRARY_ERROR"
        };
        ToolError::new(code, e)
    })
}

fn parse_gallery_query(args: &Value) -> Result<crate::gallery::GalleryQuery, ToolError> {
    let query = args.get("query").cloned().unwrap_or_else(|| json!({}));
    if !query.is_object() {
        return Err(ToolError::new("INVALID_ARGS", "query must be an object"));
    }
    serde_json::from_value(query)
        .map_err(|e| ToolError::new("INVALID_ARGS", format!("invalid query: {e}")))
}

async fn native_gallery_query(app: &AppHandle, args: Value) -> ToolResult {
    let query = parse_gallery_query(&args)?;
    let (page, boards) = with_store(app, move |store| {
        Ok((store.gallery_query(query)?, store.board_list()?))
    })
    .await?;
    Ok(ToolOutput {
        structured: json!({
            "assets": page.items.iter().map(project_gallery).collect::<Vec<_>>(),
            "boards": boards,
            "total": page.total,
            "offset": page.offset,
            "limit": page.limit,
        }),
        images: vec![],
    })
}

async fn native_gallery_read(app: &AppHandle, args: Value) -> ToolResult {
    let asset_id = args
        .get("assetId")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("INVALID_ARGS", "assetId is required"))?
        .to_string();
    crate::library::validate_id(&asset_id).map_err(|e| ToolError::new("INVALID_ARGS", e))?;
    let variant = args
        .get("variant")
        .and_then(Value::as_str)
        .unwrap_or("thumbnail")
        .to_string();
    let (data, mime, meta) = match variant.as_str() {
        "original" => {
            let id = asset_id.clone();
            let image = with_store(app, move |store| store.gallery_read(&id)).await?;
            let (marker, data) = image
                .data_url
                .split_once(";base64,")
                .ok_or_else(|| ToolError::new("INTERNAL", "malformed image data"))?;
            let mime = marker.trim_start_matches("data:").to_string();
            (
                data.to_string(),
                mime,
                json!({
                    "assetId": image.asset_id,
                    "name": image.name,
                    "width": image.width,
                    "height": image.height,
                }),
            )
        }
        "thumbnail" => {
            let id = asset_id.clone();
            let item = with_store(app, move |store| {
                store.gallery_get(&[id]).map(|v| v.into_iter().next())
            })
            .await?
            .ok_or_else(|| ToolError::new("ASSET_NOT_FOUND", "no asset with this id"))?;
            let id2 = asset_id.clone();
            let bytes = with_store(app, move |store| store.thumbnail(&id2))
                .await?
                .ok_or_else(|| ToolError::new("ASSET_NOT_FOUND", "asset has no thumbnail"))?;
            use base64::Engine;
            (
                base64::engine::general_purpose::STANDARD.encode(bytes),
                "image/png".to_string(),
                json!({
                    "assetId": item.id,
                    "name": item.name,
                    "width": item.width,
                    "height": item.height,
                }),
            )
        }
        other => {
            return Err(ToolError::new(
                "INVALID_ARGS",
                format!("variant must be thumbnail or original, got {other}"),
            ))
        }
    };
    let mut structured = meta;
    if let Some(map) = structured.as_object_mut() {
        map.insert("mimeType".into(), Value::String(mime.clone()));
        map.insert("variant".into(), Value::String(variant));
    }
    Ok(ToolOutput {
        structured,
        images: vec![(data, mime)],
    })
}

async fn native_gallery_import(app: &AppHandle, args: Value) -> ToolResult {
    let data_url = args
        .get("dataUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("INVALID_ARGS", "dataUrl is required"))?
        .to_string();
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("MCP import")
        .to_string();
    let item = with_store(app, move |store| {
        store.gallery_import(&data_url, &name, "file")
    })
    .await?;
    let _ = app.emit_to(MAIN_WINDOW, LIBRARY_EVENT, json!({}));
    Ok(ToolOutput {
        structured: json!({ "asset": project_gallery(&item) }),
        images: vec![],
    })
}

fn uuid_list(value: Option<&Value>) -> Result<Vec<String>, ToolError> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| ToolError::new("INVALID_ARGS", "assetIds is required"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| ToolError::new("INVALID_ARGS", "assetIds must be UUID strings"))
        })
        .collect()
}

fn emit_library(app: &AppHandle) {
    let _ = app.emit_to(MAIN_WINDOW, LIBRARY_EVENT, json!({}));
}

async fn native_gallery_board(app: &AppHandle, args: Value) -> ToolResult {
    let op = args
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("INVALID_ARGS", "op is required"))?
        .to_string();
    match op.as_str() {
        "list" => {
            let boards = with_store(app, |store| store.board_list()).await?;
            Ok(ToolOutput {
                structured: json!({ "boards": boards }),
                images: vec![],
            })
        }
        "create" => {
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::new("INVALID_ARGS", "name is required"))?
                .to_string();
            let board = with_store(app, move |store| store.board_create(&name)).await?;
            emit_library(app);
            Ok(ToolOutput {
                structured: json!({ "board": board }),
                images: vec![],
            })
        }
        "rename" => {
            let id = args
                .get("boardId")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::new("INVALID_ARGS", "boardId is required"))?
                .to_string();
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::new("INVALID_ARGS", "name is required"))?
                .to_string();
            let board = with_store(app, move |store| store.board_rename(&id, &name)).await?;
            emit_library(app);
            Ok(ToolOutput {
                structured: json!({ "board": board }),
                images: vec![],
            })
        }
        "delete" => {
            let id = args
                .get("boardId")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::new("INVALID_ARGS", "boardId is required"))?
                .to_string();
            with_store(app, move |store| store.board_delete(&id)).await?;
            emit_library(app);
            Ok(ToolOutput {
                structured: json!({ "ok": true }),
                images: vec![],
            })
        }
        "add" | "remove" => {
            let id = args
                .get("boardId")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::new("INVALID_ARGS", "boardId is required"))?
                .to_string();
            let ids = uuid_list(args.get("assetIds"))?;
            let add = op == "add";
            let result = with_store(app, move |store| {
                if add {
                    store.board_add(&id, &ids)
                } else {
                    store.board_remove(&id, &ids)
                }
            })
            .await?;
            emit_library(app);
            Ok(ToolOutput {
                structured: serde_json::to_value(result).unwrap_or(Value::Null),
                images: vec![],
            })
        }
        "move" => {
            let from = args
                .get("fromBoardId")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::new("INVALID_ARGS", "fromBoardId is required"))?
                .to_string();
            let to = args
                .get("toBoardId")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::new("INVALID_ARGS", "toBoardId is required"))?
                .to_string();
            let ids = uuid_list(args.get("assetIds"))?;
            let result =
                with_store(app, move |store| store.board_move(&from, &to, &ids)).await?;
            emit_library(app);
            Ok(ToolOutput {
                structured: serde_json::to_value(result).unwrap_or(Value::Null),
                images: vec![],
            })
        }
        other => Err(ToolError::new(
            "INVALID_ARGS",
            format!("unknown board op {other}"),
        )),
    }
}

/// Pure projection seam so the empty-result branch is testable without Tauri.
pub(crate) fn task_projection(store: &LibraryStore, item: &HistoryItem) -> Result<Value, String> {
    let assets: HashMap<_, _> = if item.result_asset_ids.is_empty() {
        HashMap::new()
    } else {
        store
            .gallery_get(&item.result_asset_ids)?
            .into_iter()
            .map(|asset| (asset.id.clone(), asset))
            .collect()
    };
    let results: Vec<_> = item
        .result_asset_ids
        .iter()
        .map(|asset_id| {
            let availability = match assets.get(asset_id) {
                Some(asset) if asset.pending_delete => "pendingDelete",
                Some(asset) => asset.availability.as_str(),
                None => "deleted",
            };
            json!({ "assetId": asset_id, "availability": availability })
        })
        .collect();
    Ok(json!({
        "id": item.id,
        "status": item.status,
        "phase": item.phase,
        "taskId": item.task_id,
        "createdAt": item.created_at,
        "provider": item.provider,
        "model": item.model,
        "prompt": item.prompt,
        "finalPrompt": item.final_prompt,
        "params": item.params,
        "batch": item.batch,
        "error": item.error,
        "results": results,
    }))
}

async fn native_task_get(app: &AppHandle, args: Value) -> ToolResult {
    let task_id = args
        .get("taskId")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("INVALID_ARGS", "taskId is required"))?
        .to_string();
    crate::library::validate_id(&task_id).map_err(|e| ToolError::new("INVALID_ARGS", e))?;
    let projected = with_store(app, move |store| {
        let item = store
            .history_find(&task_id)?
            .ok_or_else(|| "task not found".to_string())?;
        task_projection(store, &item)
    })
    .await
    .map_err(|e| {
        if e.message == "task not found" {
            ToolError::new("TASK_NOT_FOUND", "task not found")
        } else {
            e
        }
    })?;
    Ok(ToolOutput {
        structured: projected,
        images: vec![],
    })
}

// ---------- Tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use serde_json::{json, Value};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener as StdListener};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, RwLock};
    use tokio::net::TcpListener;
    use tokio_util::sync::CancellationToken;

    const TEST_TOKEN: &str = "test-token-0000-1111-2222-333344445555";

    fn test_dispatcher(calls: Arc<AtomicUsize>, result: Value) -> McpDispatcher {
        Arc::new(move |name: String, _args: Value| {
            let calls = calls.clone();
            let result = result.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::Relaxed);
                Ok(ToolOutput {
                    structured: json!({ "tool": name, "result": result }),
                    images: vec![],
                })
            })
        })
    }

    /// Binds a real rmcp Streamable HTTP service on an ephemeral loopback port
    /// with the same boundary middleware as production.
    async fn spawn_test_server(
        dispatcher: McpDispatcher,
    ) -> (String, SharedAuth, CancellationToken) {
        let listener =
            StdListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().unwrap().port();
        let auth: SharedAuth = Arc::new(RwLock::new(Auth {
            token: TEST_TOKEN.into(),
            port,
        }));
        let cancel = CancellationToken::new();
        let service_cancel = cancel.child_token();
        let router = build_router(auth.clone(), dispatcher, cancel.child_token());
        tauri::async_runtime::spawn(async move {
            let listener = TcpListener::from_std(listener).unwrap();
            tokio::select! {
                _ = service_cancel.cancelled() => {}
                _ = axum::serve(listener, router.into_make_service()) => {}
            }
        });
        (format!("http://127.0.0.1:{port}/mcp"), auth, cancel)
    }

    fn client() -> reqwest::Client {
        reqwest::Client::new()
    }

    fn post(url: &str, token: Option<&str>, body: Value) -> reqwest::RequestBuilder {
        let mut req = client()
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&body);
        if let Some(t) = token {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        req
    }

    fn initialize_body() -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "0" }
            }
        })
    }

    /// Runs the initialize handshake; the stateless service issues no session
    /// id, so subsequent requests only carry the protocol version header.
    async fn session(url: &str) -> reqwest::header::HeaderMap {
        let resp = post(url, Some(TEST_TOKEN), initialize_body())
            .send()
            .await
            .expect("initialize");
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = resp.json().await.expect("initialize result");
        assert!(body["result"]["protocolVersion"].is_string(), "{body}");
        // Clients discover tools through the advertised capabilities.
        assert!(
            body["result"]["capabilities"]["tools"].is_object(),
            "{body}"
        );
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("mcp-protocol-version", "2025-03-26".parse().unwrap());
        headers
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejects_before_dispatch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (url, _auth, cancel) =
            spawn_test_server(test_dispatcher(calls.clone(), json!({}))).await;
        let body = initialize_body();

        // Missing bearer.
        let r = post(&url, None, body.clone()).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

        // Wrong token.
        let r = post(
            &url,
            Some("test-token-9999-8888-7777-666655554444"),
            body.clone(),
        )
        .send()
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

        // Foreign Host.
        let r = post(&url, Some(TEST_TOKEN), body.clone())
            .header("Host", "evil.example.com")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        // Web Origin.
        let r = post(&url, Some(TEST_TOKEN), body.clone())
            .header("Origin", "https://evil.example.com")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        assert_eq!(calls.load(Ordering::Relaxed), 0, "dispatcher not called");
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serves_initialize_list_and_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (url, _auth, cancel) =
            spawn_test_server(test_dispatcher(calls.clone(), json!({"ok": true}))).await;

        let headers = session(&url).await;

        // tools/list through the real SDK negotiation.
        let resp = post(
            &url,
            Some(TEST_TOKEN),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        )
        .headers(headers.clone())
        .send()
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = resp.json().await.unwrap();
        let tools = body["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 12);
        assert!(tools.iter().any(|t| t["name"] == "workspace_get"));
        assert!(tools.iter().any(|t| t["name"] == "gallery_board"));
        let query = tools
            .iter()
            .find(|t| t["name"] == "gallery_query")
            .unwrap();
        assert!(query["inputSchema"]["properties"]["query"]["properties"]
            .get("favorite")
            .is_none());
        assert!(query["inputSchema"]["properties"]["query"]["properties"]
            .get("boardId")
            .is_some());

        // One bridged workspace_get succeeds and reaches the dispatcher.
        let resp = post(
            &url,
            Some(TEST_TOKEN),
            json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": {"name": "workspace_get", "arguments": {}}
            }),
        )
        .headers(headers.clone())
        .send()
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = resp.json().await.unwrap();
        assert!(body.get("error").is_none(), "{body}");
        assert_eq!(body["result"]["structuredContent"]["tool"], "workspace_get");

        // Nested malformed args are rejected by the schema validator: the
        // dispatcher never sees them (patch.prompt must be a string).
        let before = calls.load(Ordering::Relaxed);
        let resp = post(
            &url,
            Some(TEST_TOKEN),
            json!({
                "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                "params": {
                    "name": "workspace_patch",
                    "arguments": {
                        "expectedVersion": "v1",
                        "patch": {"prompt": 5}
                    }
                }
            }),
        )
        .headers(headers.clone())
        .send()
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = resp.json().await.unwrap();
        let err = body.get("error").expect("schema error expected");
        assert_eq!(err["code"], -32602);
        assert!(err["message"].as_str().unwrap().contains("/patch/prompt"));
        assert_eq!(
            calls.load(Ordering::Relaxed),
            before,
            "malformed args must not reach the dispatcher"
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rotated_token_rejects_old_bearer() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (url, auth, cancel) =
            spawn_test_server(test_dispatcher(calls.clone(), json!({}))).await;
        assert_eq!(
            post(&url, Some(TEST_TOKEN), initialize_body())
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        // Rotating the shared auth invalidates the old bearer immediately.
        auth.write().unwrap().token = "rotated-token-0000-111122223333".into();
        assert_eq!(
            post(&url, Some(TEST_TOKEN), initialize_body())
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        cancel.cancel();
        // A disabled service rejects subsequent requests outright.
        let outcome = post(&url, Some(TEST_TOKEN), initialize_body()).send().await;
        match outcome {
            Ok(r) => assert_ne!(r.status(), StatusCode::OK),
            Err(_) => {}
        }
    }

    /// A new/running task has no result assets: task_get must project an
    /// empty list instead of calling gallery_get with an empty id set.
    #[test]
    fn task_projection_handles_empty_results() {
        let dir = std::env::temp_dir().join(format!("mcp-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = LibraryStore::new(dir.clone()).unwrap();
        let item = HistoryItem {
            id: "task-running".into(),
            status: "running".into(),
            ..Default::default()
        };
        let projected = task_projection(&store, &item).unwrap();
        assert_eq!(projected["id"], "task-running");
        assert_eq!(projected["status"], "running");
        assert_eq!(projected["results"], json!([]));
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn issued_token_is_only_on_the_configure_result() {
        let status = McpStatus {
            enabled: true,
            running: true,
            port: DEFAULT_PORT,
            url: Some(format!("http://127.0.0.1:{DEFAULT_PORT}/mcp")),
            has_token: true,
            ui_ready: true,
            error: None,
        };
        let issued = McpConfigureResult {
            status: status.clone(),
            issued_token: Some("minted-token".into()),
        };
        let value = serde_json::to_value(&issued).unwrap();
        assert_eq!(value["issuedToken"], "minted-token");
        assert!(value.get("unshownToken").is_none());
        let quiet = McpConfigureResult {
            status: status.clone(),
            issued_token: None,
        };
        assert!(serde_json::to_value(&quiet)
            .unwrap()
            .get("issuedToken")
            .is_none());
        assert!(serde_json::to_value(&status)
            .unwrap()
            .get("issuedToken")
            .is_none());
    }

    fn validator(name: &str) -> &'static jsonschema::Validator {
        let registry = tools_file().unwrap();
        let index = registry
            .tools
            .iter()
            .position(|t| t.name.as_ref() == name)
            .unwrap();
        &registry.validators[index]
    }

    #[test]
    fn gallery_query_schema_is_board_only() {
        let v = validator("gallery_query");
        let board = Uuid::new_v4().to_string();
        assert!(v.validate(&json!({ "query": { "boardId": board } })).is_ok());
        assert!(v.validate(&json!({ "query": { "favorite": true } })).is_err());
    }

    #[test]
    fn gallery_board_schema_accepts_ops() {
        let v = validator("gallery_board");
        let board = Uuid::new_v4().to_string();
        let asset = Uuid::new_v4().to_string();
        let other = Uuid::new_v4().to_string();
        assert!(v.validate(&json!({ "op": "list" })).is_ok());
        assert!(v.validate(&json!({ "op": "create", "name": "Looks" })).is_ok());
        assert!(v
            .validate(&json!({ "op": "rename", "boardId": board, "name": "Kept" }))
            .is_ok());
        assert!(v.validate(&json!({ "op": "delete", "boardId": board })).is_ok());
        assert!(v
            .validate(&json!({ "op": "add", "boardId": board, "assetIds": [asset] }))
            .is_ok());
        assert!(v
            .validate(&json!({
                "op": "move",
                "fromBoardId": board,
                "toBoardId": other,
                "assetIds": [asset]
            }))
            .is_ok());
        assert!(v.validate(&json!({ "op": "list", "name": "Looks" })).is_err());
        assert!(v.validate(&json!({ "op": "favorite" })).is_err());
    }
}
