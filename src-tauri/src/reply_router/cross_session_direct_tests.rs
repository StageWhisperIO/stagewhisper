use super::*;
use serde_json::Value;
use std::fs;
use std::sync::{Arc, Mutex};

fn temp_registry_backed_store(tag: &str) -> Arc<sw_notes::SessionStore> {
    let dir =
        std::env::temp_dir().join(format!("sw_free_reply_router_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    Arc::new(sw_notes::SessionStore::new(dir, [0x77; 32]).expect("temp session store"))
}

fn sample_registry_record(session_id: &str) -> sw_notes::SessionRecord {
    sw_notes::SessionRecord {
        session_id: session_id.to_string(),
        relay_session_id: uuid::Uuid::new_v4().to_string(),
        backend_session_id: None,
        started_at: "2026-08-17T10:00:00Z".to_string(),
        ended_at: "2026-08-17T10:30:00Z".to_string(),
        title: None,
        title_is_auto: false,
        segments: vec![],
        insights: vec![],
        blocks: vec![],
        notes_markdown: None,
        notes_status: None,
        notes_error: None,
        notes_root_message_id: None,
        notes_pending_root_message_id: None,
        chat: vec![],
        attendees: vec![],
        calendar_event_id: None,
        playbook: None,
    }
}

struct RegistryBackedSink {
    live_session_id: String,
    store: Arc<sw_notes::SessionStore>,
    registry: Arc<sw_notes::TurnRegistry>,
    pending: Arc<PendingReplies>,
}

impl RegistryBackedSink {
    fn new(
        live_session_id: &str,
        store: Arc<sw_notes::SessionStore>,
        registry: Arc<sw_notes::TurnRegistry>,
        pending: Arc<PendingReplies>,
    ) -> Self {
        Self {
            live_session_id: live_session_id.to_string(),
            store,
            registry,
            pending,
        }
    }
}

impl ReplySink for RegistryBackedSink {
    fn current_session_id(&self) -> Option<String> {
        Some(self.live_session_id.clone())
    }
    fn session_known(&self, session_id: &str) -> bool {
        self.store.load(session_id).ok().flatten().is_some()
    }
    fn append_message(&self, message: ChatMessagePayload) -> bool {
        self.store
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
            .is_ok()
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
    fn touch_pending(&self, task_id: &str) {
        self.pending.touch(task_id);
    }
    fn validate_task_session(&self, task_id: &str, session_id: &str) -> ReserveResult {
        self.pending.validate(task_id, session_id)
    }
    fn task_session(&self, task_id: &str) -> Option<String> {
        self.pending.session_for(task_id)
    }
    fn route_turn_part(&self, task_id: &str, text: &str) -> bool {
        self.registry.push_message_part(
            task_id,
            Some(crate::state::session_chat::LOCAL_IDENTITY_BINDING),
            text,
        )
    }
    fn route_turn_text_delta(&self, task_id: &str, delta: &str) -> bool {
        self.registry.push_text_delta(
            task_id,
            Some(crate::state::session_chat::LOCAL_IDENTITY_BINDING),
            delta,
        )
    }
    fn turn_is_retired(&self, task_id: &str) -> bool {
        self.registry.is_retired(task_id)
    }
    fn forward_to_turn(
        &self,
        task_id: &str,
        status: &str,
        reply_text: Option<&str>,
        error_code: Option<&str>,
        error_message: Option<&str>,
    ) -> bool {
        crate::state::session_chat::apply_relay_turn_outcome(
            &self.registry,
            &self.store,
            task_id,
            status,
            reply_text,
            error_code,
            error_message,
        )
    }
}

fn recording_turn_sink() -> (
    impl Fn(sw_notes::UiChunk) + Send + Sync + 'static,
    Arc<Mutex<Vec<sw_notes::UiChunk>>>,
) {
    let received: Arc<Mutex<Vec<sw_notes::UiChunk>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = received.clone();
    (
        move |chunk: sw_notes::UiChunk| recorder.lock().unwrap().push(chunk),
        received,
    )
}

fn turn_chunk_kinds(events: &[sw_notes::UiChunk]) -> Vec<&'static str> {
    events
        .iter()
        .map(|chunk| match chunk {
            sw_notes::UiChunk::Start { .. } => "start",
            sw_notes::UiChunk::TextStart { .. } => "text-start",
            sw_notes::UiChunk::TextDelta { .. } => "text-delta",
            sw_notes::UiChunk::TextEnd { .. } => "text-end",
            sw_notes::UiChunk::Activity { .. } => "activity",
            sw_notes::UiChunk::Error { .. } => "error",
            sw_notes::UiChunk::Finish { .. } => "finish",
        })
        .collect()
}

fn register_live_relay_turn(
    registry: &sw_notes::TurnRegistry,
    pending: &PendingReplies,
    task_id: &str,
    session_id: &str,
) -> Arc<Mutex<Vec<sw_notes::UiChunk>>> {
    let (sink, captured) = recording_turn_sink();
    registry
        .register(
            task_id,
            session_id,
            "umsg-1",
            task_id,
            Some(crate::state::session_chat::LOCAL_IDENTITY_BINDING.to_string()),
            sink,
        )
        .expect("fresh turn id registers");
    pending.register(task_id.to_string(), session_id.to_string());
    captured.lock().unwrap().clear();
    captured
}

const LIVE_TASK_ID_MESSAGE: &str = "aaaaaaaa-1111-2222-3333-444444444444";
const LIVE_TASK_ID_ERRORED: &str = "bbbbbbbb-1111-2222-3333-444444444444";
const LIVE_TASK_ID_TYPING: &str = "cccccccc-1111-2222-3333-444444444444";
const LIVE_TASK_ID_CORRECT: &str = "dddddddd-1111-2222-3333-444444444444";
const LIVE_TASK_ID_STREAM: &str = "eeeeeeee-1111-2222-3333-444444444444";
const LIVE_TASK_ID_STREAM_WRONG: &str = "ffffffff-1111-2222-3333-444444444444";

fn reply_body(task_id: &str, session_id: &str, status: &str) -> ReplyBody {
    ReplyBody {
        task_id: Some(task_id.to_string()),
        session_id: session_id.to_string(),
        user_message_id: Some("umsg-1".to_string()),
        status: status.to_string(),
        message_id: None,
        reply_text: None,
        error_code: None,
        error_message: None,
        model: None,
        chunk: None,
    }
}

#[tokio::test]
async fn a_byo_message_reply_carrying_a_known_but_wrong_session_id_for_a_live_task_is_rejected_and_persists_and_emits_nothing(
) {
    let store = temp_registry_backed_store("message_wrong_session");
    store.save(&sample_registry_record("sess-live")).unwrap();
    store.save(&sample_registry_record("sess-other")).unwrap();
    let registry = Arc::new(sw_notes::TurnRegistry::new());
    let pending = Arc::new(PendingReplies::default());
    let captured = register_live_relay_turn(&registry, &pending, LIVE_TASK_ID_MESSAGE, "sess-live");

    let sink = RegistryBackedSink::new(
        "sess-live",
        store.clone(),
        registry.clone(),
        pending.clone(),
    );
    let mut body = reply_body(LIVE_TASK_ID_MESSAGE, "sess-other", "message");
    body.reply_text = Some("malicious injected reply".to_string());

    let disposition = route_reply(&sink, LIVE_TASK_ID_MESSAGE, body);

    assert_eq!(disposition, ReplyDisposition::SessionMismatch);
    assert!(store.load("sess-live").unwrap().unwrap().chat.is_empty());
    assert!(store.load("sess-other").unwrap().unwrap().chat.is_empty());
    assert!(registry.contains(LIVE_TASK_ID_MESSAGE));
    assert!(captured.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_byo_errored_reply_carrying_a_known_but_wrong_session_id_for_a_live_task_is_rejected_and_persists_and_emits_nothing(
) {
    let store = temp_registry_backed_store("errored_wrong_session");
    store.save(&sample_registry_record("sess-live")).unwrap();
    store.save(&sample_registry_record("sess-other")).unwrap();
    let registry = Arc::new(sw_notes::TurnRegistry::new());
    let pending = Arc::new(PendingReplies::default());
    let captured = register_live_relay_turn(&registry, &pending, LIVE_TASK_ID_ERRORED, "sess-live");

    let sink = RegistryBackedSink::new(
        "sess-live",
        store.clone(),
        registry.clone(),
        pending.clone(),
    );
    let mut body = reply_body(LIVE_TASK_ID_ERRORED, "sess-other", "errored");
    body.error_code = Some("forged".to_string());
    body.error_message = Some("forged error for the wrong session".to_string());

    let disposition = route_reply(&sink, LIVE_TASK_ID_ERRORED, body);

    assert_eq!(disposition, ReplyDisposition::SessionMismatch);
    assert!(store.load("sess-live").unwrap().unwrap().chat.is_empty());
    assert!(store.load("sess-other").unwrap().unwrap().chat.is_empty());
    assert!(registry.contains(LIVE_TASK_ID_ERRORED));
    assert!(captured.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_byo_typing_reply_carrying_a_known_but_wrong_session_id_for_a_live_task_is_rejected_and_does_not_refresh_the_watchdog(
) {
    let store = temp_registry_backed_store("typing_wrong_session");
    store.save(&sample_registry_record("sess-live")).unwrap();
    store.save(&sample_registry_record("sess-other")).unwrap();
    let registry = Arc::new(sw_notes::TurnRegistry::new());
    let pending = Arc::new(PendingReplies::default());
    let captured = register_live_relay_turn(&registry, &pending, LIVE_TASK_ID_TYPING, "sess-live");

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let elapsed_before = pending.since_last_activity(LIVE_TASK_ID_TYPING).unwrap();

    let sink = RegistryBackedSink::new(
        "sess-live",
        store.clone(),
        registry.clone(),
        pending.clone(),
    );
    let body = reply_body(LIVE_TASK_ID_TYPING, "sess-other", "typing");

    let disposition = route_reply(&sink, LIVE_TASK_ID_TYPING, body);

    assert_eq!(disposition, ReplyDisposition::SessionMismatch);
    let elapsed_after = pending.since_last_activity(LIVE_TASK_ID_TYPING).unwrap();
    assert!(
        elapsed_after >= elapsed_before,
        "a session-mismatched typing reply must not refresh the watchdog"
    );
    assert!(store.load("sess-live").unwrap().unwrap().chat.is_empty());
    assert!(registry.contains(LIVE_TASK_ID_TYPING));
    assert!(captured.lock().unwrap().is_empty());
}

#[tokio::test]
async fn every_message_part_reaches_the_live_turn_and_only_the_terminal_closes_it() {
    let store = temp_registry_backed_store("message_correct_session");
    store.save(&sample_registry_record("sess-live")).unwrap();
    let registry = Arc::new(sw_notes::TurnRegistry::new());
    let pending = Arc::new(PendingReplies::default());
    let captured = register_live_relay_turn(&registry, &pending, LIVE_TASK_ID_CORRECT, "sess-live");

    let sink = RegistryBackedSink::new(
        "sess-live",
        store.clone(),
        registry.clone(),
        pending.clone(),
    );

    for part in ["the first part", "the second part"] {
        let mut body = reply_body(LIVE_TASK_ID_CORRECT, "sess-live", "message");
        body.reply_text = Some(part.to_string());
        assert_eq!(
            route_reply(&sink, LIVE_TASK_ID_CORRECT, body),
            ReplyDisposition::Accepted
        );
    }

    assert!(store.load("sess-live").unwrap().unwrap().chat.is_empty());
    assert!(registry.contains(LIVE_TASK_ID_CORRECT));
    assert_eq!(
        turn_chunk_kinds(&captured.lock().unwrap()),
        vec!["text-delta", "text-delta"]
    );

    let terminal = reply_body(LIVE_TASK_ID_CORRECT, "sess-live", "completed");
    assert_eq!(
        route_reply(&sink, LIVE_TASK_ID_CORRECT, terminal),
        ReplyDisposition::Accepted
    );

    assert!(!registry.contains(LIVE_TASK_ID_CORRECT));
    let record = store.load("sess-live").unwrap().unwrap();
    assert_eq!(record.chat.len(), 1);
    assert_eq!(record.chat[0].content, "the first part\n\nthe second part");
    {
        let events = captured.lock().unwrap();
        assert_eq!(
            turn_chunk_kinds(&events),
            vec!["text-delta", "text-delta", "text-end", "finish"]
        );
        match events.last() {
            Some(sw_notes::UiChunk::Finish { finish_reason }) => {
                assert_eq!(finish_reason, "stop")
            }
            other => panic!("expected a finish chunk, got {other:?}"),
        }
    }

    let mut late_part = reply_body(LIVE_TASK_ID_CORRECT, "sess-live", "message");
    late_part.reply_text = Some("arrived too late".to_string());
    assert_eq!(
        route_reply(&sink, LIVE_TASK_ID_CORRECT, late_part),
        ReplyDisposition::AlreadyFinalized
    );
    assert_eq!(store.load("sess-live").unwrap().unwrap().chat.len(), 1);
    assert_eq!(captured.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn a_stream_chunk_reaches_the_live_turn_without_closing_it_and_the_terminal_keeps_its_text() {
    let store = temp_registry_backed_store("stream_correct_session");
    store.save(&sample_registry_record("sess-live")).unwrap();
    let registry = Arc::new(sw_notes::TurnRegistry::new());
    let pending = Arc::new(PendingReplies::default());
    let captured = register_live_relay_turn(&registry, &pending, LIVE_TASK_ID_STREAM, "sess-live");

    let sink = RegistryBackedSink::new(
        "sess-live",
        store.clone(),
        registry.clone(),
        pending.clone(),
    );

    for delta in ["Hel", "lo"] {
        let mut body = reply_body(LIVE_TASK_ID_STREAM, "sess-live", "stream");
        body.chunk = Some(serde_json::json!({
            "type": "text-delta",
            "id": "t1",
            "delta": delta,
        }));
        assert_eq!(
            route_reply(&sink, LIVE_TASK_ID_STREAM, body),
            ReplyDisposition::Accepted
        );
    }

    assert!(registry.contains(LIVE_TASK_ID_STREAM));
    assert_eq!(
        turn_chunk_kinds(&captured.lock().unwrap()),
        vec!["text-delta", "text-delta"]
    );

    let terminal = reply_body(LIVE_TASK_ID_STREAM, "sess-live", "completed");
    assert_eq!(
        route_reply(&sink, LIVE_TASK_ID_STREAM, terminal),
        ReplyDisposition::Accepted
    );

    let record = store.load("sess-live").unwrap().unwrap();
    assert_eq!(record.chat.len(), 1);
    assert_eq!(record.chat[0].content, "Hello");
}

#[tokio::test]
async fn a_stream_chunk_carrying_a_wrong_session_id_never_reaches_the_live_turn() {
    let store = temp_registry_backed_store("stream_wrong_session");
    store.save(&sample_registry_record("sess-live")).unwrap();
    store.save(&sample_registry_record("sess-other")).unwrap();
    let registry = Arc::new(sw_notes::TurnRegistry::new());
    let pending = Arc::new(PendingReplies::default());
    let captured =
        register_live_relay_turn(&registry, &pending, LIVE_TASK_ID_STREAM_WRONG, "sess-live");

    let sink = RegistryBackedSink::new(
        "sess-live",
        store.clone(),
        registry.clone(),
        pending.clone(),
    );
    let mut body = reply_body(LIVE_TASK_ID_STREAM_WRONG, "sess-other", "stream");
    body.chunk = Some(serde_json::json!({
        "type": "text-delta",
        "id": "t1",
        "delta": "leaked",
    }));

    assert_eq!(
        route_reply(&sink, LIVE_TASK_ID_STREAM_WRONG, body),
        ReplyDisposition::SessionMismatch
    );
    assert!(registry.contains(LIVE_TASK_ID_STREAM_WRONG));
    assert!(captured.lock().unwrap().is_empty());
}
