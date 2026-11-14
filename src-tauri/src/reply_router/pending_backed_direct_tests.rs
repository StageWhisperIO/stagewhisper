use super::*;
use serde_json::Value;
use std::sync::{Arc, Mutex};

const TASK_ID: &str = "11111111-2222-3333-4444-555555555555";

struct PendingBackedSink {
    session_id: String,
    pending: Arc<PendingReplies>,
    appended: Mutex<Vec<ChatMessagePayload>>,
}

impl PendingBackedSink {
    fn new(session_id: &str, pending: Arc<PendingReplies>) -> Self {
        Self {
            session_id: session_id.to_string(),
            pending,
            appended: Mutex::new(Vec::new()),
        }
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
    fn touch_pending(&self, task_id: &str) {
        self.pending.touch(task_id);
    }
    fn validate_task_session(&self, task_id: &str, session_id: &str) -> ReserveResult {
        self.pending.validate(task_id, session_id)
    }
}

fn typing_body() -> ReplyBody {
    ReplyBody {
        task_id: Some(TASK_ID.to_string()),
        session_id: "sess-1".to_string(),
        user_message_id: None,
        status: "typing".to_string(),
        message_id: None,
        reply_text: None,
        error_code: None,
        error_message: None,
        model: None,
        chunk: None,
    }
}

fn completed_body(text: &str) -> ReplyBody {
    ReplyBody {
        task_id: Some(TASK_ID.to_string()),
        session_id: "sess-1".to_string(),
        user_message_id: Some("umsg-1".to_string()),
        status: "completed".to_string(),
        message_id: None,
        reply_text: Some(text.to_string()),
        error_code: None,
        error_message: None,
        model: None,
        chunk: None,
    }
}

#[tokio::test]
async fn a_typing_reply_backed_by_real_pending_state_refreshes_the_last_activity_watchdog() {
    let pending = Arc::new(PendingReplies::default());
    pending.register(TASK_ID.to_string(), "sess-1".to_string());
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let elapsed_before_touch = pending.since_last_activity(TASK_ID).unwrap();
    assert!(elapsed_before_touch >= std::time::Duration::from_millis(20));

    let sink = PendingBackedSink::new("sess-1", pending.clone());
    let disposition = route_reply(&sink, TASK_ID, typing_body());
    assert_eq!(disposition, ReplyDisposition::Accepted);

    let elapsed_after_touch = pending.since_last_activity(TASK_ID).unwrap();
    assert!(elapsed_after_touch < elapsed_before_touch);
}

#[tokio::test]
async fn a_reply_backed_by_real_pending_state_with_no_registration_is_rejected_not_silently_acked()
{
    let pending = Arc::new(PendingReplies::default());
    let sink = PendingBackedSink::new("sess-1", pending.clone());

    let disposition = route_reply(
        &sink,
        TASK_ID,
        completed_body("legit reply with no pending record"),
    );

    assert_eq!(disposition, ReplyDisposition::UnregisteredTask);
    assert!(sink.appended.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_reply_backed_by_real_pending_state_persists_once_and_acks_the_duplicate() {
    let pending = Arc::new(PendingReplies::default());
    pending.register(TASK_ID.to_string(), "sess-1".to_string());
    let sink = PendingBackedSink::new("sess-1", pending.clone());

    let first = route_reply(&sink, TASK_ID, completed_body("first wins"));
    assert_eq!(first, ReplyDisposition::Accepted);
    assert_eq!(sink.appended.lock().unwrap().len(), 1);

    let second = route_reply(&sink, TASK_ID, completed_body("first wins"));
    assert_eq!(second, ReplyDisposition::AlreadyFinalized);
    assert_eq!(sink.appended.lock().unwrap().len(), 1);
}
