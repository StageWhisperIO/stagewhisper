use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;

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

const FINALIZED_CAPACITY: usize = 4096;

#[derive(Default)]
struct PendingInner {
    pending: HashSet<String>,
    finalized: HashSet<String>,
    finalized_order: VecDeque<String>,
}

#[derive(Default)]
pub struct PendingReplies {
    inner: std::sync::Mutex<PendingInner>,
}

impl PendingReplies {
    pub fn register(&self, task_id: String) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.pending.insert(task_id);
        }
    }

    pub fn claim_terminal(&self, task_id: &str) -> bool {
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        if guard.finalized.contains(task_id) {
            return false;
        }
        guard.pending.remove(task_id);
        guard.finalized.insert(task_id.to_string());
        guard.finalized_order.push_back(task_id.to_string());
        while guard.finalized_order.len() > FINALIZED_CAPACITY {
            if let Some(old) = guard.finalized_order.pop_front() {
                guard.finalized.remove(&old);
            }
        }
        true
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
    fn append_message(&self, message: ChatMessagePayload);
    fn emit_created(&self, payload: &ChatMessagePayload);
    fn emit_errored(&self, payload: &Value);
    fn claim_terminal(&self, task_id: &str) -> bool;
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

    fn append_message(&self, message: ChatMessagePayload) {
        if let Some(store) = self
            .app
            .try_state::<Arc<SessionStore>>()
            .map(|s| s.inner().clone())
        {
            let _ = store.record_reply(
                &message.session_id,
                message.parent_message_id.as_deref(),
                &message.id,
                &message.content,
                &message.status,
                message.error_code.as_deref(),
                message.error_message.as_deref(),
                &message.created_at,
            );
        }
    }

    fn emit_created(&self, payload: &ChatMessagePayload) {
        let _ = self.app.emit("chat-message-created", payload);
    }

    fn emit_errored(&self, payload: &Value) {
        let _ = self.app.emit("chat-message-errored", payload);
    }

    fn claim_terminal(&self, task_id: &str) -> bool {
        match self.app.try_state::<Arc<PendingReplies>>() {
            Some(pending) => pending.claim_terminal(task_id),
            None => true,
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
    token: String,
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
    #[allow(dead_code)]
    token: String,
    shutdown: Option<oneshot::Sender<()>>,
}

impl ChatReplyListener {
    pub async fn start(app: AppHandle, token: String) -> Result<Self> {
        let port: u16 = std::env::var("STAGEWHISPER_CALLBACK_PORT")
            .ok()
            .and_then(|value| value.trim().parse().ok())
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

        let advertised_url = std::env::var("STAGEWHISPER_CALLBACK_URL")
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty());

        let sink: Arc<dyn ReplySink> = Arc::new(TauriReplySink { app });
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

fn build_router(sink: Arc<dyn ReplySink>, token: String) -> Router {
    Router::new()
        .route("/tasks/{task_id}", post(handle_reply))
        .with_state(ListenerState { sink, token })
}

async fn handle_reply(
    State(state): State<ListenerState>,
    Path(path_task_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ReplyBody>,
) -> impl IntoResponse {
    if !bearer_matches(&headers, &state.token) {
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
    let known_statuses = ["completed", "errored", "silent", "typing"];
    if !known_statuses.contains(&body.status.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"ok": false, "error": "invalid status"})),
        )
            .into_response();
    }

    let effective_task_id = body.task_id.clone().unwrap_or_else(|| path_task_id.clone());

    if matches!(body.status.as_str(), "completed" | "errored") {
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

    if matches!(body.status.as_str(), "completed" | "errored" | "silent")
        && !state.sink.claim_terminal(&effective_task_id)
    {
        eprintln!(
            "[chat-reply-listener] dropping late/duplicate terminal callback task_id={effective_task_id} status={}",
            body.status,
        );
        return (
            StatusCode::OK,
            Json(json!({"ok": true, "ignored": true, "reason": "already_finalized"})),
        )
            .into_response();
    }

    let now = chrono::Utc::now().to_rfc3339();

    match body.status.as_str() {
        "completed" => {
            let reply_text = body.reply_text.clone().unwrap_or_default();
            let payload = ChatMessagePayload {
                id: effective_task_id,
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
                finalized_at: Some(now),
            };
            state.sink.append_message(payload.clone());
            state.sink.emit_created(&payload);
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
                finalized_at: Some(now),
            };
            state.sink.append_message(placeholder.clone());
            state.sink.emit_created(&placeholder);
            let event = json!({
                "task_id": effective_task_id,
                "session_id": body.session_id,
                "user_message_id": body.user_message_id,
                "error_code": body.error_code,
                "error_message": body.error_message,
            });
            state.sink.emit_errored(&event);
        }
        "silent" if body.user_message_id.is_some() => {
            let placeholder = ChatMessagePayload {
                id: effective_task_id,
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
            state.sink.append_message(placeholder.clone());
            state.sink.emit_created(&placeholder);
        }
        _ => {}
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

    struct MockSink {
        session_id: Option<String>,
        known_session: Option<String>,
        appended: Mutex<Vec<ChatMessagePayload>>,
        created_events: Mutex<Vec<ChatMessagePayload>>,
        errored_events: Mutex<Vec<Value>>,
        finalized: Mutex<Vec<String>>,
        armed_probes: Mutex<HashSet<String>>,
        resolved_probes: Mutex<Vec<ProbeOutcome>>,
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
                armed_probes: Mutex::new(HashSet::new()),
                resolved_probes: Mutex::new(Vec::new()),
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
                armed_probes: Mutex::new(HashSet::new()),
                resolved_probes: Mutex::new(Vec::new()),
            })
        }

        fn arm_probe(&self, task_id: &str) {
            self.armed_probes
                .lock()
                .unwrap()
                .insert(task_id.to_string());
        }
    }

    impl ReplySink for MockSink {
        fn current_session_id(&self) -> Option<String> {
            self.session_id.clone()
        }
        fn session_known(&self, session_id: &str) -> bool {
            self.known_session.as_deref() == Some(session_id)
        }
        fn append_message(&self, message: ChatMessagePayload) {
            self.appended.lock().unwrap().push(message);
        }
        fn emit_created(&self, payload: &ChatMessagePayload) {
            self.created_events.lock().unwrap().push(payload.clone());
        }
        fn emit_errored(&self, payload: &Value) {
            self.errored_events.lock().unwrap().push(payload.clone());
        }
        fn claim_terminal(&self, task_id: &str) -> bool {
            let mut finalized = self.finalized.lock().unwrap();
            if finalized.iter().any(|t| t == task_id) {
                return false;
            }
            finalized.push(task_id.to_string());
            true
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
        let router = build_router(sink.clone(), "tok".into());
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
    async fn bad_bearer_rejected_401() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), "real-token".into());
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
    async fn missing_bearer_rejected_401() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), "real-token".into());
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
        let router = build_router(sink.clone(), "tok".into());
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
        let router = build_router(sink.clone(), "tok".into());
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
        let router = build_router(sink.clone(), "tok".into());
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
        let router = build_router(sink.clone(), "tok".into());
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
        let router = build_router(sink.clone(), "tok".into());
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
        let router = build_router(sink.clone(), "tok".into());
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
        let router = build_router(sink.clone(), "tok".into());
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
        let router = build_router(sink.clone(), "tok".into());
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
        let router = build_router(sink.clone(), "tok".into());
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
        let router = build_router(sink.clone(), "tok".into());
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
        let router = build_router(sink.clone(), "tok".into());
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
        let router = build_router(sink.clone(), "tok".into());
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
    fn claim_terminal_is_one_shot() {
        let pending = PendingReplies::default();
        pending.register(TEST_TASK_ID.to_string());
        assert!(pending.claim_terminal(TEST_TASK_ID));
        assert!(!pending.claim_terminal(TEST_TASK_ID));
    }

    #[test]
    fn claim_terminal_first_wins_even_without_register() {
        let pending = PendingReplies::default();
        assert!(pending.claim_terminal(TEST_TASK_ID));
        assert!(!pending.claim_terminal(TEST_TASK_ID));
    }

    #[tokio::test]
    async fn completed_reply_claims_terminal() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), "tok".into());
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
    async fn typing_status_does_not_claim_terminal() {
        let sink = MockSink::new(Some("sess-1"));
        let router = build_router(sink.clone(), "tok".into());
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
        let router = build_router(sink.clone(), "tok".into());
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

    #[tokio::test]
    async fn callback_url_returns_base_only() {
        let addr: SocketAddr = "127.0.0.1:51234".parse().unwrap();
        let cl = ChatReplyListener {
            addr,
            advertised_url: None,
            token: "tok".into(),
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
            token: "tok".into(),
            shutdown: None,
        };
        assert_eq!(cl.callback_url(), "https://my-tunnel.ngrok.app");
    }
}
