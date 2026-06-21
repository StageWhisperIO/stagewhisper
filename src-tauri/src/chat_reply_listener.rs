use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::{anyhow, Context, Result};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine;
use getrandom::fill as getrandom_fill;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use subtle::ConstantTimeEq;
use sw_notes::SessionStore;
use tauri::{AppHandle, Emitter, Manager};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::state::app_state::AppState;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessagePayload {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub status: String,
    pub tool_calls: Option<Vec<Value>>,
    pub tool_result_payload: Option<Value>,
    pub parent_message_id: Option<String>,
    pub suggestion_id: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub finalized_at: Option<String>,
}

pub fn generate_callback_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom_fill(&mut bytes).expect("OS RNG failed while generating callback token");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

const PENDING_CAPACITY: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReserveResult {
    Reserved,
    Duplicate,
    Unregistered,
    SessionMismatch,
}

#[derive(Default)]
struct PendingInner {
    pending: HashMap<String, String>,
    order: VecDeque<String>,
    reserved: HashSet<String>,
    finalized: HashSet<String>,
    finalized_order: VecDeque<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct PersistState {
    pending: Vec<(String, String)>,
    finalized: Vec<String>,
}

pub struct PendingReplies {
    inner: std::sync::Mutex<PendingInner>,
    path: Option<PathBuf>,
}

impl Default for PendingReplies {
    fn default() -> Self {
        Self {
            inner: std::sync::Mutex::new(PendingInner::default()),
            path: None,
        }
    }
}

impl PendingReplies {
    pub fn load(path: PathBuf) -> Self {
        let mut inner = PendingInner::default();
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(state) = serde_json::from_str::<PersistState>(&raw) {
                for (task_id, session_id) in state.pending {
                    if !inner.pending.contains_key(&task_id) {
                        inner.pending.insert(task_id.clone(), session_id);
                        inner.order.push_back(task_id);
                    }
                }
                for task_id in state.finalized {
                    if inner.finalized.insert(task_id.clone()) {
                        inner.finalized_order.push_back(task_id);
                    }
                }
            }
        }
        Self {
            inner: std::sync::Mutex::new(inner),
            path: Some(path),
        }
    }

    pub fn register(&self, task_id: String, session_id: String) {
        if let Ok(mut guard) = self.inner.lock() {
            if !guard.pending.contains_key(&task_id) {
                guard.pending.insert(task_id.clone(), session_id);
                guard.order.push_back(task_id);
                while guard.order.len() > PENDING_CAPACITY {
                    if let Some(old) = guard.order.pop_front() {
                        guard.pending.remove(&old);
                        guard.reserved.remove(&old);
                    }
                }
            }
            self.persist(&guard);
        }
    }

    pub fn reserve(&self, task_id: &str, session_id: &str) -> ReserveResult {
        if let Ok(mut guard) = self.inner.lock() {
            if guard.finalized.contains(task_id) {
                return ReserveResult::Duplicate;
            }
            match guard.pending.get(task_id) {
                None => return ReserveResult::Unregistered,
                Some(expected) if expected != session_id => {
                    return ReserveResult::SessionMismatch
                }
                Some(_) => {}
            }
            if guard.reserved.insert(task_id.to_string()) {
                return ReserveResult::Reserved;
            }
            return ReserveResult::Duplicate;
        }
        ReserveResult::Unregistered
    }

    pub fn release(&self, task_id: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.reserved.remove(task_id);
        }
    }

    pub fn complete(&self, task_id: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.reserved.remove(task_id);
            let was_pending = guard.pending.remove(task_id).is_some();
            if was_pending {
                guard.order.retain(|t| t != task_id);
            }
            let newly_finalized = guard.finalized.insert(task_id.to_string());
            if newly_finalized {
                guard.finalized_order.push_back(task_id.to_string());
                while guard.finalized_order.len() > PENDING_CAPACITY {
                    if let Some(old) = guard.finalized_order.pop_front() {
                        guard.finalized.remove(&old);
                    }
                }
            }
            if was_pending || newly_finalized {
                self.persist(&guard);
            }
        }
    }

    fn persist(&self, inner: &PendingInner) {
        let Some(path) = &self.path else {
            return;
        };
        let state = PersistState {
            pending: inner
                .order
                .iter()
                .filter_map(|task_id| {
                    inner
                        .pending
                        .get(task_id)
                        .map(|session_id| (task_id.clone(), session_id.clone()))
                })
                .collect(),
            finalized: inner.finalized_order.iter().cloned().collect(),
        };
        let Ok(raw) = serde_json::to_string(&state) else {
            return;
        };
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, raw).is_err() {
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&tmp) {
                let mut perms = meta.permissions();
                perms.set_mode(0o600);
                let _ = fs::set_permissions(&tmp, perms);
            }
        }
        let _ = fs::rename(&tmp, path);
    }
}

#[derive(Clone, Debug)]
pub struct ProbeOutcome {
    pub status: String,
    pub reply_text: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Default)]
pub struct ProbeRegistry {
    inner: std::sync::Mutex<HashMap<String, oneshot::Sender<ProbeOutcome>>>,
}

impl ProbeRegistry {
    pub fn register(&self, task_id: String) -> oneshot::Receiver<ProbeOutcome> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(task_id, tx);
        }
        rx
    }

    pub fn cancel(&self, task_id: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(task_id);
        }
    }

    fn take(&self, task_id: &str) -> Option<oneshot::Sender<ProbeOutcome>> {
        self.inner.lock().ok().and_then(|mut g| g.remove(task_id))
    }
}

pub trait ReplySink: Send + Sync + 'static {
    fn current_session_id(&self) -> Option<String>;
    fn session_known(&self, session_id: &str) -> bool;
    fn append_message(&self, message: ChatMessagePayload) -> bool;
    fn emit_created(&self, payload: &ChatMessagePayload);
    fn emit_errored(&self, payload: &Value);
    fn reserve_terminal(&self, task_id: &str, session_id: &str) -> ReserveResult;
    fn release_terminal(&self, task_id: &str);
    fn complete_terminal(&self, task_id: &str);
    fn resolve_probe(&self, _task_id: &str, _outcome: ProbeOutcome) -> bool {
        false
    }
}

struct TauriReplySink {
    app: AppHandle,
}

impl ReplySink for TauriReplySink {
    fn current_session_id(&self) -> Option<String> {
        let state = self.app.try_state::<std::sync::Mutex<AppState>>()?;
        let guard = state.lock().ok()?;
        guard.session_id.clone()
    }

    fn session_known(&self, session_id: &str) -> bool {
        self.app
            .try_state::<Arc<SessionStore>>()
            .map(|s| s.inner().clone())
            .and_then(|store| store.load(session_id).ok().flatten())
            .is_some()
    }

    fn append_message(&self, message: ChatMessagePayload) -> bool {
        match self
            .app
            .try_state::<Arc<SessionStore>>()
            .map(|s| s.inner().clone())
        {
            Some(store) => store
                .record_reply(
                    &message.session_id,
                    message.parent_message_id.as_deref(),
                    &message.id,
                    &message.content,
                    &message.status,
                    message.error_code.as_deref(),
                    message.error_message.as_deref(),
                    &message.created_at,
                )
                .is_ok(),
            None => true,
        }
    }

    fn emit_created(&self, payload: &ChatMessagePayload) {
        let _ = self.app.emit("chat-message-created", payload);
    }

    fn emit_errored(&self, payload: &Value) {
        let _ = self.app.emit("chat-message-errored", payload);
    }

    fn reserve_terminal(&self, task_id: &str, session_id: &str) -> ReserveResult {
        match self.app.try_state::<Arc<PendingReplies>>() {
            Some(pending) => pending.reserve(task_id, session_id),
            None => ReserveResult::Reserved,
        }
    }

    fn release_terminal(&self, task_id: &str) {
        if let Some(pending) = self.app.try_state::<Arc<PendingReplies>>() {
            pending.release(task_id);
        }
    }

    fn complete_terminal(&self, task_id: &str) {
        if let Some(pending) = self.app.try_state::<Arc<PendingReplies>>() {
            pending.complete(task_id);
        }
    }

    fn resolve_probe(&self, task_id: &str, outcome: ProbeOutcome) -> bool {
        match self.app.try_state::<Arc<ProbeRegistry>>() {
            Some(registry) => match registry.take(task_id) {
                Some(tx) => {
                    let _ = tx.send(outcome);
                    true
                }
                None => false,
            },
            None => false,
        }
    }
}

#[derive(Clone)]
struct ListenerState {
    sink: Arc<dyn ReplySink>,
    token: Arc<RwLock<String>>,
}

#[derive(Debug, Deserialize)]
struct ReplyBody {
    task_id: Option<String>,
    session_id: String,
    user_message_id: Option<String>,
    status: String,
    #[serde(default)]
    reply_text: Option<String>,
    #[serde(default)]
    error_code: Option<String>,
    #[serde(default)]
    error_message: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

pub struct ChatReplyListener {
    addr: SocketAddr,
    advertised_url: Option<String>,
    token: Arc<RwLock<String>>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl ChatReplyListener {
    pub async fn start(
        app: AppHandle,
        token: String,
        port_override: Option<u16>,
        advertised_override: Option<String>,
    ) -> Result<Self> {
        let port: u16 = port_override
            .or_else(|| {
                std::env::var("STAGEWHISPER_CALLBACK_PORT")
                    .ok()
                    .and_then(|value| value.trim().parse().ok())
            })
            .unwrap_or(0);
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .with_context(|| format!("binding chat reply listener to 127.0.0.1:{port}"))?;
        let addr = listener
            .local_addr()
            .context("reading chat reply listener local addr")?;
        if !addr.ip().is_loopback() {
            return Err(anyhow!("listener bound to non-loopback address {addr}"));
        }

        let advertised_url = advertised_override
            .or_else(|| std::env::var("STAGEWHISPER_CALLBACK_URL").ok())
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty());

        let sink: Arc<dyn ReplySink> = Arc::new(TauriReplySink { app });
        let token = Arc::new(RwLock::new(token));
        let router = build_router(sink, token.clone());
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        tokio::spawn(async move {
            let serve = axum::serve(listener, router).with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            });
            if let Err(err) = serve.await {
                eprintln!("[chat-reply-listener] server exited: {err}");
            }
        });

        Ok(Self {
            addr,
            advertised_url,
            token,
            shutdown: Some(shutdown_tx),
        })
    }

    pub fn callback_url(&self) -> String {
        match &self.advertised_url {
            Some(url) => url.clone(),
            None => format!("http://{}", self.addr),
        }
    }

    pub fn local_port(&self) -> u16 {
        self.addr.port()
    }

    pub fn set_token(&self, token: String) {
        *self.token.write().expect("callback token lock poisoned") = token;
    }

    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for ChatReplyListener {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn build_router(sink: Arc<dyn ReplySink>, token: Arc<RwLock<String>>) -> Router {
    Router::new()
        .route("/tasks/{task_id}", post(handle_reply))
        .with_state(ListenerState { sink, token })
}

enum TerminalEmit {
    Created,
    Errored(Value),
}

async fn handle_reply(
    State(state): State<ListenerState>,
    Path(path_task_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ReplyBody>,
) -> impl IntoResponse {
    let expected_token = state
        .token
        .read()
        .expect("callback token lock poisoned")
        .clone();
    if !bearer_matches(&headers, &expected_token) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"ok": false}))).into_response();
    }

    if uuid::Uuid::parse_str(&path_task_id).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"ok": false, "error": "invalid_task_id"})),
        )
            .into_response();
    }

    if let Some(body_task_id) = body.task_id.as_ref() {
        if body_task_id != &path_task_id {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "task_id_mismatch"})),
            )
                .into_response();
        }
    }

    if body.session_id.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"ok": false, "error": "session_id required"})),
        )
            .into_response();
    }
    let known_statuses = ["completed", "errored", "silent", "typing", "tool_call", "message"];
    if !known_statuses.contains(&body.status.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"ok": false, "error": "invalid status"})),
        )
            .into_response();
    }

    let effective_task_id = body.task_id.clone().unwrap_or_else(|| path_task_id.clone());

    if matches!(body.status.as_str(), "completed" | "errored" | "message") {
        let outcome = ProbeOutcome {
            status: body.status.clone(),
            reply_text: body.reply_text.clone(),
            error_message: body.error_message.clone(),
        };
        if state.sink.resolve_probe(&effective_task_id, outcome) {
            return (StatusCode::OK, Json(json!({"ok": true, "probe": true}))).into_response();
        }
    }

    let current_session = state.sink.current_session_id();
    let session_matches = current_session
        .as_ref()
        .map(|s| s == &body.session_id)
        .unwrap_or(false);

    if !session_matches && !state.sink.session_known(&body.session_id) {
        let reason = if current_session.is_none() {
            "session_ended"
        } else {
            "session_mismatch"
        };
        eprintln!(
            "[chat-reply-listener] dropping late callback task_id={path_task_id} status={} reason={reason}",
            body.status,
        );
        return (
            StatusCode::OK,
            Json(json!({"ok": true, "ignored": true, "reason": reason})),
        )
            .into_response();
    }

    if matches!(body.status.as_str(), "typing" | "tool_call") {
        return (StatusCode::OK, Json(json!({"ok": true}))).into_response();
    }

    if body.status == "message" {
        let text = body.reply_text.clone().unwrap_or_default();
        if text.trim().is_empty() {
            return (
                StatusCode::OK,
                Json(json!({"ok": true, "ignored": true, "reason": "empty_message"})),
            )
                .into_response();
        }
        let message_id = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            effective_task_id.hash(&mut hasher);
            body.session_id.hash(&mut hasher);
            text.hash(&mut hasher);
            format!("{effective_task_id}:msg:{:016x}", hasher.finish())
        };
        let now = chrono::Utc::now().to_rfc3339();
        let payload = ChatMessagePayload {
            id: message_id,
            session_id: body.session_id.clone(),
            role: "assistant".to_string(),
            content: text,
            status: "completed".to_string(),
            tool_calls: None,
            tool_result_payload: body.model.as_ref().map(|m| json!({ "model": m })),
            parent_message_id: body.user_message_id.clone(),
            suggestion_id: None,
            error_code: None,
            error_message: None,
            created_at: now.clone(),
            updated_at: now.clone(),
            finalized_at: Some(now),
        };
        if !state.sink.append_message(payload.clone()) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "error": "reply_persist_failed"})),
            )
                .into_response();
        }
        state.sink.emit_created(&payload);
        return (StatusCode::OK, Json(json!({"ok": true}))).into_response();
    }

    let now = chrono::Utc::now().to_rfc3339();
    let is_terminal = matches!(body.status.as_str(), "completed" | "errored" | "silent");

    let to_persist: Option<(ChatMessagePayload, TerminalEmit)> = match body.status.as_str() {
        "completed" => {
            let reply_text = body.reply_text.clone().unwrap_or_default();
            let payload = ChatMessagePayload {
                id: effective_task_id.clone(),
                session_id: body.session_id.clone(),
                role: "assistant".to_string(),
                content: reply_text,
                status: "completed".to_string(),
                tool_calls: None,
                tool_result_payload: body.model.as_ref().map(|m| json!({ "model": m })),
                parent_message_id: body.user_message_id.clone(),
                suggestion_id: None,
                error_code: None,
                error_message: None,
                created_at: now.clone(),
                updated_at: now.clone(),
                finalized_at: Some(now.clone()),
            };
            Some((payload, TerminalEmit::Created))
        }
        "errored" => {
            let placeholder = ChatMessagePayload {
                id: effective_task_id.clone(),
                session_id: body.session_id.clone(),
                role: "assistant".to_string(),
                content: String::new(),
                status: "errored".to_string(),
                tool_calls: None,
                tool_result_payload: None,
                parent_message_id: body.user_message_id.clone(),
                suggestion_id: None,
                error_code: body.error_code.clone(),
                error_message: body.error_message.clone(),
                created_at: now.clone(),
                updated_at: now.clone(),
                finalized_at: Some(now.clone()),
            };
            let event = json!({
                "task_id": effective_task_id.clone(),
                "session_id": body.session_id,
                "user_message_id": body.user_message_id,
                "error_code": body.error_code,
                "error_message": body.error_message,
            });
            Some((placeholder, TerminalEmit::Errored(event)))
        }
        "silent" if body.user_message_id.is_some() => {
            let placeholder = ChatMessagePayload {
                id: effective_task_id.clone(),
                session_id: body.session_id.clone(),
                role: "assistant".to_string(),
                content: String::new(),
                status: "cancelled".to_string(),
                tool_calls: None,
                tool_result_payload: None,
                parent_message_id: body.user_message_id,
                suggestion_id: None,
                error_code: None,
                error_message: None,
                created_at: now.clone(),
                updated_at: now.clone(),
                finalized_at: Some(now),
            };
            Some((placeholder, TerminalEmit::Created))
        }
        _ => None,
    };

    if is_terminal {
        match state.sink.reserve_terminal(&effective_task_id, &body.session_id) {
            ReserveResult::Reserved => {}
            ReserveResult::Duplicate => {
                return (
                    StatusCode::OK,
                    Json(json!({"ok": true, "ignored": true, "reason": "already_finalized"})),
                )
                    .into_response();
            }
            ReserveResult::Unregistered => {
                eprintln!(
                    "[chat-reply-listener] rejecting terminal callback for unknown or evicted task_id={effective_task_id} status={}",
                    body.status,
                );
                return (
                    StatusCode::GONE,
                    Json(json!({"ok": false, "error": "unregistered_task"})),
                )
                    .into_response();
            }
            ReserveResult::SessionMismatch => {
                eprintln!(
                    "[chat-reply-listener] rejecting terminal callback with session mismatch task_id={effective_task_id} status={}",
                    body.status,
                );
                return (
                    StatusCode::CONFLICT,
                    Json(json!({"ok": false, "error": "session_mismatch"})),
                )
                    .into_response();
            }
        }
    }

    if let Some((payload, _)) = to_persist.as_ref() {
        if !state.sink.append_message(payload.clone()) {
            if is_terminal {
                state.sink.release_terminal(&effective_task_id);
            }
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "error": "reply_persist_failed"})),
            )
                .into_response();
        }
    }

    if is_terminal {
        state.sink.complete_terminal(&effective_task_id);
    }

    if let Some((payload, emit)) = to_persist {
        match emit {
            TerminalEmit::Created => state.sink.emit_created(&payload),
            TerminalEmit::Errored(event) => {
                state.sink.emit_created(&payload);
                state.sink.emit_errored(&event);
            }
        }
    }

    (StatusCode::OK, Json(json!({"ok": true}))).into_response()
}

fn bearer_matches(headers: &HeaderMap, expected: &str) -> bool {
    let provided = match headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        Some(value) => value,
        None => return false,
    };
    let stripped = match provided.strip_prefix("Bearer ") {
        Some(s) => s,
        None => return false,
    };
    let a = stripped.as_bytes();
    let b = expected.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

pub async fn ensure_callback_token(
    store: &crate::relay_settings::RelaySettingsStore,
) -> Result<String> {
    let current = store.snapshot().callback_token.clone();
    if let Some(token) = current {
        if !token.trim().is_empty() {
            return Ok(token);
        }
    }
    let new_token = generate_callback_token();
    store
        .update(|s| {
            s.callback_token = Some(new_token.clone());
        })
        .map_err(|e| anyhow!("persisting callback token: {e}"))?;
    Ok(new_token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use std::sync::Mutex;
    use tower::ServiceExt;

    fn test_token(value: &str) -> Arc<RwLock<String>> {
        Arc::new(RwLock::new(value.to_string()))
    }

    struct MockSink {
        session_id: Option<String>,
        known_session: Option<String>,
        appended: Mutex<Vec<ChatMessagePayload>>,
        created_events: Mutex<Vec<ChatMessagePayload>>,
        errored_events: Mutex<Vec<Value>>,
        finalized: Mutex<Vec<String>>,
        reserved: Mutex<HashSet<String>>,
        unregistered: Mutex<HashSet<String>>,
        expected_session: Mutex<HashMap<String, String>>,
        armed_probes: Mutex<HashSet<String>>,
        resolved_probes: Mutex<Vec<ProbeOutcome>>,
        append_ok: Mutex<bool>,
    }

    impl MockSink {
        fn new(session_id: Option<&str>) -> Arc<Self> {
            Arc::new(Self {
                session_id: session_id.map(String::from),
                known_session: None,
                appended: Mutex::new(Vec::new()),
                created_events: Mutex::new(Vec::new()),
                errored_events: Mutex::new(Vec::new()),
                finalized: Mutex::new(Vec::new()),
                reserved: Mutex::new(HashSet::new()),
                unregistered: Mutex::new(HashSet::new()),
                expected_session: Mutex::new(HashMap::new()),
                armed_probes: Mutex::new(HashSet::new()),
                resolved_probes: Mutex::new(Vec::new()),
                append_ok: Mutex::new(true),
            })
        }

        fn new_stored(known_session: &str) -> Arc<Self> {
            Arc::new(Self {
                session_id: None,
                known_session: Some(known_session.to_string()),
                appended: Mutex::new(Vec::new()),
                created_events: Mutex::new(Vec::new()),
                errored_events: Mutex::new(Vec::new()),
                finalized: Mutex::new(Vec::new()),
                reserved: Mutex::new(HashSet::new()),
                unregistered: Mutex::new(HashSet::new()),
                expected_session: Mutex::new(HashMap::new()),
                armed_probes: Mutex::new(HashSet::new()),
                resolved_probes: Mutex::new(Vec::new()),
                append_ok: Mutex::new(true),
            })
        }

        fn arm_probe(&self, task_id: &str) {
            self.armed_probes
                .lock()
                .unwrap()
                .insert(task_id.to_string());
        }

        fn fail_appends(&self) {
            *self.append_ok.lock().unwrap() = false;
        }

        fn allow_appends(&self) {
            *self.append_ok.lock().unwrap() = true;
        }

        fn mark_unregistered(&self, task_id: &str) {
            self.unregistered
                .lock()
                .unwrap()
                .insert(task_id.to_string());
        }

        fn bind_session(&self, task_id: &str, session_id: &str) {
            self.expected_session
                .lock()
                .unwrap()
                .insert(task_id.to_string(), session_id.to_string());
        }
    }

    impl ReplySink for MockSink {
        fn current_session_id(&self) -> Option<String> {
            self.session_id.clone()
        }
        fn session_known(&self, session_id: &str) -> bool {
            self.known_session.as_deref() == Some(session_id)
        }
        fn append_message(&self, message: ChatMessagePayload) -> bool {
            if !*self.append_ok.lock().unwrap() {
                return false;
            }
            let mut appended = self.appended.lock().unwrap();
            if !appended.iter().any(|m| m.id == message.id) {
                appended.push(message);
            }
            true
        }
        fn emit_created(&self, payload: &ChatMessagePayload) {
            self.created_events.lock().unwrap().push(payload.clone());
        }
        fn emit_errored(&self, payload: &Value) {
            self.errored_events.lock().unwrap().push(payload.clone());
        }
        fn reserve_terminal(&self, task_id: &str, session_id: &str) -> ReserveResult {
            if self.finalized.lock().unwrap().iter().any(|t| t == task_id) {
                return ReserveResult::Duplicate;
            }
            if self.unregistered.lock().unwrap().contains(task_id) {
                return ReserveResult::Unregistered;
            }
            if let Some(expected) = self.expected_session.lock().unwrap().get(task_id) {
                if expected != session_id {
                    return ReserveResult::SessionMismatch;
                }
            }
            if self.reserved.lock().unwrap().insert(task_id.to_string()) {
                ReserveResult::Reserved
            } else {
                ReserveResult::Duplicate
            }
        }

        fn release_terminal(&self, task_id: &str) {
            self.reserved.lock().unwrap().remove(task_id);
        }

        fn complete_terminal(&self, task_id: &str) {
            self.reserved.lock().unwrap().remove(task_id);
            let mut finalized = self.finalized.lock().unwrap();
            if !finalized.iter().any(|t| t == task_id) {
                finalized.push(task_id.to_string());
            }
        }
        fn resolve_probe(&self, task_id: &str, outcome: ProbeOutcome) -> bool {
            if self.armed_probes.lock().unwrap().remove(task_id) {
                self.resolved_probes.lock().unwrap().push(outcome);
                true
            } else {
                false
            }
        }
    }

    const TEST_TASK_ID: &str = "11111111-2222-3333-4444-555555555555";

    fn make_request(token: Option<&str>, task_id: &str, body: &str) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri(format!("/tasks/{task_id}"))
            .header("content-type", "application/json");
        if let Some(t) = token {
            builder = builder.header("authorization", format!("Bearer {t}"));
        }
        builder.body(Body::from(body.to_string())).unwrap()
    }

    #[tokio::test]
    async fn token_is_32_bytes_base64url() {
        let token = generate_callback_token();
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&token)
            .expect("token must be valid base64url");
        assert_eq!(decoded.len(), 32);
    }

    #[tokio::test]
    async fn valid_reply_appends_and_emits() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-1",
            "user_message_id": "umsg-1",
            "status": "completed",
            "reply_text": "hello from assistant"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["ok"], json!(true));
        let appended = sink.appended.lock().unwrap();
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].content, "hello from assistant");
        assert_eq!(appended[0].id, TEST_TASK_ID);
        assert_eq!(appended[0].parent_message_id.as_deref(), Some("umsg-1"));
        assert_eq!(appended[0].role, "assistant");
        assert_eq!(sink.created_events.lock().unwrap().len(), 1);
        assert_eq!(sink.errored_events.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn message_status_resolves_probe() {
        let sink = MockSink::new(None);
        sink.arm_probe(TEST_TASK_ID);
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "probe-session",
            "status": "message",
            "reply_text": "ready when you are"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["probe"], json!(true));
        let probes = sink.resolved_probes.lock().unwrap();
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].status, "message");
    }

    #[tokio::test]
    async fn message_status_appends_during_session() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-1",
            "user_message_id": "umsg-1",
            "status": "message",
            "reply_text": "coaching cue"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let appended = sink.appended.lock().unwrap();
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].content, "coaching cue");
        assert_eq!(appended[0].role, "assistant");
        assert_eq!(appended[0].status, "completed");
        assert_eq!(appended[0].parent_message_id.as_deref(), Some("umsg-1"));
        assert_ne!(appended[0].id, TEST_TASK_ID);
        assert_eq!(sink.created_events.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn bad_bearer_rejected_401() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), test_token("real-token"));
        let body = json!({
            "session_id": "sess-1",
            "status": "completed",
            "reply_text": "hi"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("wrong-token"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(sink.appended.lock().unwrap().len(), 0);
        assert_eq!(sink.created_events.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn rotating_token_revokes_old_and_accepts_new() {
        let sink = MockSink::new(Some("sess-1"));
        let cell = test_token("old-token");
        let router = build_router(sink.clone(), cell.clone());
        let task_b = "66666666-7777-8888-9999-aaaaaaaaaaaa";
        let body_a = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-1",
            "status": "completed",
            "reply_text": "before"
        })
        .to_string();
        let body_b = json!({
            "task_id": task_b,
            "session_id": "sess-1",
            "status": "completed",
            "reply_text": "after"
        })
        .to_string();

        let accepted_old = router
            .clone()
            .oneshot(make_request(Some("old-token"), TEST_TASK_ID, &body_a))
            .await
            .unwrap();
        assert_eq!(accepted_old.status(), StatusCode::OK);

        *cell.write().unwrap() = "new-token".to_string();

        let revoked = router
            .clone()
            .oneshot(make_request(Some("old-token"), task_b, &body_b))
            .await
            .unwrap();
        assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);

        let accepted_new = router
            .oneshot(make_request(Some("new-token"), task_b, &body_b))
            .await
            .unwrap();
        assert_eq!(accepted_new.status(), StatusCode::OK);
        assert_eq!(sink.appended.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn missing_bearer_rejected_401() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), test_token("real-token"));
        let body = json!({
            "session_id": "sess-1",
            "status": "completed",
            "reply_text": "hi"
        })
        .to_string();
        let response = router
            .oneshot(make_request(None, TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn malformed_json_rejected() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), test_token("tok"));
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, "{not-valid-json"))
            .await
            .unwrap();
        assert!(
            response.status().is_client_error(),
            "expected 4xx, got {}",
            response.status()
        );
        assert_eq!(sink.appended.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn mismatched_session_ignored() {
        let sink = MockSink::new(Some("sess-current"));
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "session_id": "sess-other",
            "status": "completed",
            "reply_text": "stale reply"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["ignored"], json!(true));
        assert_eq!(v["reason"], json!("session_mismatch"));
        assert_eq!(sink.appended.lock().unwrap().len(), 0);
        assert_eq!(sink.created_events.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn callback_after_session_ended_dropped_with_explicit_reason() {
        let sink = MockSink::new(None);
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-ended",
            "user_message_id": "umsg-ended",
            "status": "completed",
            "reply_text": "reply for ended session"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["ignored"], json!(true));
        assert_eq!(v["reason"], json!("session_ended"));
        assert_eq!(sink.appended.lock().unwrap().len(), 0);
        assert_eq!(sink.created_events.lock().unwrap().len(), 0);
        assert_eq!(sink.errored_events.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn post_call_reply_for_stored_session_is_accepted_when_no_session_live() {
        let sink = MockSink::new_stored("sess-stored");
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-stored",
            "user_message_id": "notes-root",
            "status": "completed",
            "reply_text": "# Summary\n## Action Items\n- [ ] follow up"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let appended = sink.appended.lock().unwrap();
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].session_id, "sess-stored");
        assert_eq!(appended[0].status, "completed");
        assert_eq!(sink.created_events.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn reply_mentioning_pairing_is_stored_as_normal_completed() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), test_token("tok"));
        let reply = "Sure — to approve a teammate, run `hermes pairing approve stagewhisper ABCD1234` on the host.";
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-1",
            "user_message_id": "notes-root",
            "status": "completed",
            "reply_text": reply
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let appended = sink.appended.lock().unwrap();
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].status, "completed");
        assert_eq!(
            appended[0].content, reply,
            "content-path replies are relayed verbatim, never reclassified as pairing"
        );
        assert!(appended[0].error_code.is_none());
        assert_eq!(sink.errored_events.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn errored_status_appends_terminal_message_and_emits_error_event() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-1",
            "user_message_id": "umsg-1",
            "status": "errored",
            "error_code": "llm_timeout",
            "error_message": "model took too long"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let appended = sink.appended.lock().unwrap();
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].role, "assistant");
        assert_eq!(appended[0].status, "errored");
        assert_eq!(appended[0].id, TEST_TASK_ID);
        assert_eq!(appended[0].parent_message_id.as_deref(), Some("umsg-1"));
        assert_eq!(appended[0].error_code.as_deref(), Some("llm_timeout"));
        assert_eq!(
            appended[0].error_message.as_deref(),
            Some("model took too long")
        );
        assert_eq!(appended[0].content, "");
        assert!(appended[0].finalized_at.is_some());

        let created = sink.created_events.lock().unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].status, "errored");

        let errs = sink.errored_events.lock().unwrap();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0]["error_code"], json!("llm_timeout"));
        assert_eq!(errs[0]["session_id"], json!("sess-1"));
    }

    #[tokio::test]
    async fn silent_status_appends_cancelled_terminal_message() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-1",
            "user_message_id": "umsg-2",
            "status": "silent"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let appended = sink.appended.lock().unwrap();
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].role, "assistant");
        assert_eq!(appended[0].status, "cancelled");
        assert_eq!(appended[0].id, TEST_TASK_ID);
        assert_eq!(appended[0].parent_message_id.as_deref(), Some("umsg-2"));
        assert!(appended[0].finalized_at.is_some());

        let created = sink.created_events.lock().unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].status, "cancelled");

        assert_eq!(sink.errored_events.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn invalid_status_rejected() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "session_id": "sess-1",
            "status": "pending"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn task_id_mismatch_rejected() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "session_id": "sess-1",
            "status": "completed",
            "reply_text": "hi"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"], json!("task_id_mismatch"));
        assert_eq!(sink.appended.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn path_task_id_not_uuid_rejected() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "session_id": "sess-1",
            "status": "completed",
            "reply_text": "hi"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), "not-a-uuid", &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"], json!("invalid_task_id"));
        assert_eq!(sink.appended.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn probe_callback_short_circuits_before_session_gating() {
        let sink = MockSink::new(None);
        sink.arm_probe(TEST_TASK_ID);
        let router = build_router(sink.clone(), test_token("tok"));
        let reply = "Hi~ I don't recognize you yet! hermes pairing approve stagewhisper ABCD1234";
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "probe-session",
            "status": "completed",
            "reply_text": reply
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["probe"], json!(true));
        assert_eq!(sink.appended.lock().unwrap().len(), 0);
        assert_eq!(sink.created_events.lock().unwrap().len(), 0);
        let probes = sink.resolved_probes.lock().unwrap();
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].status, "completed");
        assert_eq!(probes[0].reply_text.as_deref(), Some(reply));
    }

    #[tokio::test]
    async fn non_probe_callback_falls_through_to_normal_flow() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-1",
            "user_message_id": "umsg-1",
            "status": "completed",
            "reply_text": "normal reply"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(sink.appended.lock().unwrap().len(), 1);
        assert_eq!(sink.resolved_probes.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn probe_registry_register_and_take_round_trip() {
        let registry = ProbeRegistry::default();
        let rx = registry.register(TEST_TASK_ID.to_string());
        let tx = registry.take(TEST_TASK_ID).expect("registered probe");
        tx.send(ProbeOutcome {
            status: "completed".to_string(),
            reply_text: Some("ok".to_string()),
            error_message: None,
        })
        .unwrap();
        let outcome = rx.await.unwrap();
        assert_eq!(outcome.status, "completed");
        assert_eq!(outcome.reply_text.as_deref(), Some("ok"));
        assert!(registry.take(TEST_TASK_ID).is_none());
    }

    #[test]
    fn reserve_then_complete_is_one_shot() {
        let pending = PendingReplies::default();
        pending.register(TEST_TASK_ID.to_string(), "sess-1".to_string());
        assert_eq!(pending.reserve(TEST_TASK_ID, "sess-1"), ReserveResult::Reserved);
        assert_eq!(pending.reserve(TEST_TASK_ID, "sess-1"), ReserveResult::Duplicate);
        pending.complete(TEST_TASK_ID);
        assert_eq!(pending.reserve(TEST_TASK_ID, "sess-1"), ReserveResult::Duplicate);
    }

    #[test]
    fn reserve_rejects_unregistered_task() {
        let pending = PendingReplies::default();
        assert_eq!(
            pending.reserve(TEST_TASK_ID, "sess-1"),
            ReserveResult::Unregistered
        );
        pending.register(TEST_TASK_ID.to_string(), "sess-1".to_string());
        assert_eq!(pending.reserve(TEST_TASK_ID, "sess-1"), ReserveResult::Reserved);
        pending.complete(TEST_TASK_ID);
        assert_eq!(pending.reserve(TEST_TASK_ID, "sess-1"), ReserveResult::Duplicate);
    }

    #[test]
    fn reserve_rejects_session_mismatch() {
        let pending = PendingReplies::default();
        pending.register(TEST_TASK_ID.to_string(), "sess-1".to_string());
        assert_eq!(
            pending.reserve(TEST_TASK_ID, "sess-2"),
            ReserveResult::SessionMismatch
        );
        assert_eq!(pending.reserve(TEST_TASK_ID, "sess-1"), ReserveResult::Reserved);
    }

    #[test]
    fn release_returns_task_to_pending_for_retry() {
        let pending = PendingReplies::default();
        pending.register(TEST_TASK_ID.to_string(), "sess-1".to_string());
        assert_eq!(pending.reserve(TEST_TASK_ID, "sess-1"), ReserveResult::Reserved);
        pending.release(TEST_TASK_ID);
        assert_eq!(pending.reserve(TEST_TASK_ID, "sess-1"), ReserveResult::Reserved);
        pending.complete(TEST_TASK_ID);
        assert_eq!(pending.reserve(TEST_TASK_ID, "sess-1"), ReserveResult::Duplicate);
    }

    #[test]
    fn durable_pending_survives_restart() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sw-pending-{}-{}.json",
            std::process::id(),
            unique
        ));
        let _ = fs::remove_file(&path);

        {
            let pending = PendingReplies::load(path.clone());
            pending.register(TEST_TASK_ID.to_string(), "sess-1".to_string());
        }

        let reloaded = PendingReplies::load(path.clone());
        assert_eq!(
            reloaded.reserve(TEST_TASK_ID, "sess-2"),
            ReserveResult::SessionMismatch
        );
        assert_eq!(reloaded.reserve(TEST_TASK_ID, "sess-1"), ReserveResult::Reserved);
        reloaded.complete(TEST_TASK_ID);
        assert_eq!(reloaded.reserve(TEST_TASK_ID, "sess-1"), ReserveResult::Duplicate);

        let after_complete = PendingReplies::load(path.clone());
        assert_eq!(
            after_complete.reserve(TEST_TASK_ID, "sess-1"),
            ReserveResult::Duplicate
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn reserved_task_is_pending_again_after_restart() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sw-pending-reserved-{}-{}.json",
            std::process::id(),
            unique
        ));
        let _ = fs::remove_file(&path);

        {
            let pending = PendingReplies::load(path.clone());
            pending.register(TEST_TASK_ID.to_string(), "sess-1".to_string());
            assert_eq!(pending.reserve(TEST_TASK_ID, "sess-1"), ReserveResult::Reserved);
        }

        let reloaded = PendingReplies::load(path.clone());
        assert_eq!(reloaded.reserve(TEST_TASK_ID, "sess-1"), ReserveResult::Reserved);

        let _ = fs::remove_file(&path);
    }

    #[tokio::test]
    async fn completed_reply_claims_terminal() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-1",
            "user_message_id": "umsg-1",
            "status": "completed",
            "reply_text": "done"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let finalized = sink.finalized.lock().unwrap();
        assert_eq!(finalized.as_slice(), [TEST_TASK_ID.to_string()]);
    }

    #[tokio::test]
    async fn terminal_callback_for_wrong_session_does_not_persist() {
        let sink = MockSink::new_stored("sess-other");
        sink.bind_session(TEST_TASK_ID, "sess-issued");
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-other",
            "user_message_id": "umsg-1",
            "status": "completed",
            "reply_text": "cross-session reply"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"], json!("session_mismatch"));
        assert_eq!(sink.appended.lock().unwrap().len(), 0);
        assert_eq!(sink.created_events.lock().unwrap().len(), 0);
        assert_eq!(sink.finalized.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn unregistered_terminal_callback_does_not_persist() {
        let sink = MockSink::new(Some("sess-1"));
        sink.mark_unregistered(TEST_TASK_ID);
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-1",
            "user_message_id": "umsg-1",
            "status": "completed",
            "reply_text": "forged reply"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GONE);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"], json!("unregistered_task"));
        assert_eq!(sink.appended.lock().unwrap().len(), 0);
        assert_eq!(sink.created_events.lock().unwrap().len(), 0);
        assert_eq!(sink.finalized.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn failed_persist_does_not_claim_and_allows_retry() {
        let sink = MockSink::new(Some("sess-1"));
        sink.fail_appends();
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-1",
            "user_message_id": "umsg-1",
            "status": "completed",
            "reply_text": "hello"
        })
        .to_string();

        let response = router
            .clone()
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(sink.finalized.lock().unwrap().len(), 0);
        assert_eq!(sink.created_events.lock().unwrap().len(), 0);

        sink.allow_appends();
        let retry = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(retry.status(), StatusCode::OK);
        assert_eq!(sink.appended.lock().unwrap().len(), 1);
        assert_eq!(sink.finalized.lock().unwrap().as_slice(), [TEST_TASK_ID.to_string()]);
        assert_eq!(sink.created_events.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn typing_status_does_not_claim_terminal() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-1",
            "user_message_id": "umsg-1",
            "status": "typing"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(sink.finalized.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn duplicate_terminal_callback_is_dropped() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-1",
            "user_message_id": "umsg-1",
            "status": "completed",
            "reply_text": "first reply wins"
        })
        .to_string();

        let first = router
            .clone()
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(sink.appended.lock().unwrap().len(), 1);

        let stale = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-1",
            "user_message_id": "umsg-1",
            "status": "completed",
            "reply_text": "stale duplicate"
        })
        .to_string();
        let second = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &stale))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let bytes = to_bytes(second.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["ignored"], json!(true));
        assert_eq!(v["reason"], json!("already_finalized"));

        let appended = sink.appended.lock().unwrap();
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].content, "first reply wins");
    }

    struct PendingBackedSink {
        session_id: String,
        pending: Arc<PendingReplies>,
        appended: Mutex<Vec<ChatMessagePayload>>,
    }

    impl PendingBackedSink {
        fn new(session_id: &str, pending: Arc<PendingReplies>) -> Arc<Self> {
            Arc::new(Self {
                session_id: session_id.to_string(),
                pending,
                appended: Mutex::new(Vec::new()),
            })
        }
    }

    impl ReplySink for PendingBackedSink {
        fn current_session_id(&self) -> Option<String> {
            Some(self.session_id.clone())
        }
        fn session_known(&self, session_id: &str) -> bool {
            session_id == self.session_id
        }
        fn append_message(&self, message: ChatMessagePayload) -> bool {
            self.appended.lock().unwrap().push(message);
            true
        }
        fn emit_created(&self, _payload: &ChatMessagePayload) {}
        fn emit_errored(&self, _payload: &Value) {}
        fn reserve_terminal(&self, task_id: &str, session_id: &str) -> ReserveResult {
            self.pending.reserve(task_id, session_id)
        }
        fn release_terminal(&self, task_id: &str) {
            self.pending.release(task_id);
        }
        fn complete_terminal(&self, task_id: &str) {
            self.pending.complete(task_id);
        }
    }

    #[tokio::test]
    async fn lost_pending_state_callback_is_rejected_not_silently_acked() {
        let pending = Arc::new(PendingReplies::default());
        let sink = PendingBackedSink::new("sess-1", pending.clone());
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-1",
            "user_message_id": "umsg-1",
            "status": "completed",
            "reply_text": "legit reply with no pending record"
        })
        .to_string();
        let response = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GONE);
        assert_eq!(sink.appended.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn registered_reply_persists_and_duplicate_is_acked() {
        let pending = Arc::new(PendingReplies::default());
        pending.register(TEST_TASK_ID.to_string(), "sess-1".to_string());
        let sink = PendingBackedSink::new("sess-1", pending.clone());
        let router = build_router(sink.clone(), test_token("tok"));
        let body = json!({
            "task_id": TEST_TASK_ID,
            "session_id": "sess-1",
            "user_message_id": "umsg-1",
            "status": "completed",
            "reply_text": "first wins"
        })
        .to_string();

        let first = router
            .clone()
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(sink.appended.lock().unwrap().len(), 1);

        let second = router
            .oneshot(make_request(Some("tok"), TEST_TASK_ID, &body))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let bytes = to_bytes(second.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["reason"], json!("already_finalized"));
        assert_eq!(sink.appended.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn callback_url_returns_base_only() {
        let addr: SocketAddr = "127.0.0.1:51234".parse().unwrap();
        let cl = ChatReplyListener {
            addr,
            advertised_url: None,
            token: test_token("tok"),
            shutdown: None,
        };
        assert_eq!(cl.callback_url(), "http://127.0.0.1:51234");
        assert!(!cl.callback_url().ends_with("/reply"));
        assert!(!cl.callback_url().ends_with('/'));
        assert!(!cl.callback_url().contains("/tasks"));
    }

    #[tokio::test]
    async fn callback_url_uses_advertised_override() {
        let addr: SocketAddr = "127.0.0.1:51234".parse().unwrap();
        let cl = ChatReplyListener {
            addr,
            advertised_url: Some("https://my-tunnel.ngrok.app".into()),
            token: test_token("tok"),
            shutdown: None,
        };
        assert_eq!(cl.callback_url(), "https://my-tunnel.ngrok.app");
    }
}
