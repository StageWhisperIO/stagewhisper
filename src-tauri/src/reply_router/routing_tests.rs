use super::*;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

const TASK_ID: &str = "11111111-2222-3333-4444-555555555555";

type ForwardCall = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

struct FakeSink {
    session_id: Option<String>,
    known_sessions: HashSet<String>,
    expected_sessions: Mutex<HashMap<String, String>>,
    reserved: Mutex<HashSet<String>>,
    finalized: Mutex<HashSet<String>>,
    appended: Mutex<Vec<ChatMessagePayload>>,
    created_events: Mutex<Vec<ChatMessagePayload>>,
    errored_events: Mutex<Vec<Value>>,
    forwarded_turns: Mutex<Vec<ForwardCall>>,
    touch_calls: Mutex<Vec<String>>,
    forward_result: bool,
    append_result: Mutex<bool>,
}

impl FakeSink {
    fn new(session_id: &str) -> Self {
        Self {
            session_id: Some(session_id.to_string()),
            known_sessions: HashSet::new(),
            expected_sessions: Mutex::new(HashMap::new()),
            reserved: Mutex::new(HashSet::new()),
            finalized: Mutex::new(HashSet::new()),
            appended: Mutex::new(Vec::new()),
            created_events: Mutex::new(Vec::new()),
            errored_events: Mutex::new(Vec::new()),
            forwarded_turns: Mutex::new(Vec::new()),
            touch_calls: Mutex::new(Vec::new()),
            forward_result: true,
            append_result: Mutex::new(true),
        }
    }

    fn without_current_session() -> Self {
        Self {
            session_id: None,
            ..Self::new("unused")
        }
    }

    fn stored(known_session: &str) -> Self {
        let mut known_sessions = HashSet::new();
        known_sessions.insert(known_session.to_string());
        Self {
            session_id: None,
            known_sessions,
            ..Self::new("unused")
        }
    }

    fn without_forwarding(mut self) -> Self {
        self.forward_result = false;
        self
    }

    fn fail_appends(&self) {
        *self.append_result.lock().unwrap() = false;
    }

    fn allow_appends(&self) {
        *self.append_result.lock().unwrap() = true;
    }

    fn bind_expected_session(&self, task_id: &str, session_id: &str) {
        self.expected_sessions
            .lock()
            .unwrap()
            .insert(task_id.to_string(), session_id.to_string());
    }
}

impl ReplySink for FakeSink {
    fn current_session_id(&self) -> Option<String> {
        self.session_id.clone()
    }
    fn session_known(&self, session_id: &str) -> bool {
        self.known_sessions.contains(session_id)
    }
    fn append_message(&self, message: ChatMessagePayload) -> bool {
        if !*self.append_result.lock().unwrap() {
            return false;
        }
        self.appended.lock().unwrap().push(message);
        true
    }
    fn emit_created(&self, payload: &ChatMessagePayload) {
        self.created_events.lock().unwrap().push(payload.clone());
    }
    fn emit_errored(&self, payload: &Value) {
        self.errored_events.lock().unwrap().push(payload.clone());
    }
    fn reserve_terminal(&self, task_id: &str, session_id: &str) -> ReserveResult {
        if self.finalized.lock().unwrap().contains(task_id) {
            return ReserveResult::Duplicate;
        }
        if let Some(expected) = self.expected_sessions.lock().unwrap().get(task_id) {
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
        self.finalized.lock().unwrap().insert(task_id.to_string());
    }
    fn touch_pending(&self, task_id: &str) {
        self.touch_calls.lock().unwrap().push(task_id.to_string());
    }
    fn forward_to_turn(
        &self,
        task_id: &str,
        status: &str,
        reply_text: Option<&str>,
        error_code: Option<&str>,
        error_message: Option<&str>,
    ) -> bool {
        self.forwarded_turns.lock().unwrap().push((
            task_id.to_string(),
            status.to_string(),
            reply_text.map(str::to_string),
            error_code.map(str::to_string),
            error_message.map(str::to_string),
        ));
        self.forward_result
    }
}

fn reply_body(session_id: &str, status: &str) -> ReplyBody {
    ReplyBody {
        task_id: Some(TASK_ID.to_string()),
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

#[test]
fn a_completed_reply_completes_the_relay_turn() {
    let sink = FakeSink::new("sess-1");
    let mut body = reply_body("sess-1", "completed");
    body.reply_text = Some("hello from assistant".to_string());

    let disposition = route_reply(&sink, TASK_ID, body);

    assert_eq!(disposition, ReplyDisposition::Accepted);
    assert_eq!(
        sink.forwarded_turns.lock().unwrap().as_slice(),
        [(
            TASK_ID.to_string(),
            "completed".to_string(),
            Some("hello from assistant".to_string()),
            None,
            None,
        )]
    );
    assert!(sink.finalized.lock().unwrap().contains(TASK_ID));
    assert!(sink.appended.lock().unwrap().is_empty());
}

#[test]
fn a_duplicate_task_id_is_rejected_as_a_duplicate() {
    let sink = FakeSink::new("sess-1");
    let first = reply_body("sess-1", "completed");
    let first_disposition = route_reply(&sink, TASK_ID, first);
    assert_eq!(first_disposition, ReplyDisposition::Accepted);

    let second = reply_body("sess-1", "completed");
    let second_disposition = route_reply(&sink, TASK_ID, second);
    assert_eq!(second_disposition, ReplyDisposition::AlreadyFinalized);
    assert_eq!(sink.forwarded_turns.lock().unwrap().len(), 1);
}

#[test]
fn a_reply_whose_session_does_not_match_the_registered_session_is_rejected() {
    let sink = FakeSink::new("sess-current");
    sink.bind_expected_session(TASK_ID, "sess-registered");
    let body = reply_body("sess-current", "completed");

    let disposition = route_reply(&sink, TASK_ID, body);

    assert_eq!(disposition, ReplyDisposition::SessionMismatch);
    assert!(sink.forwarded_turns.lock().unwrap().is_empty());
    assert!(sink.finalized.lock().unwrap().is_empty());
    assert!(sink.appended.lock().unwrap().is_empty());
}

#[test]
fn an_errored_status_surfaces_the_error_rather_than_a_message() {
    let sink = FakeSink::new("sess-1");
    let mut body = reply_body("sess-1", "errored");
    body.error_code = Some("llm_timeout".to_string());
    body.error_message = Some("model took too long".to_string());

    let disposition = route_reply(&sink, TASK_ID, body);

    assert_eq!(disposition, ReplyDisposition::Accepted);
    let forwarded = sink.forwarded_turns.lock().unwrap();
    assert_eq!(forwarded.len(), 1);
    assert_eq!(forwarded[0].1, "errored");
    assert_eq!(forwarded[0].2, None);
    assert_eq!(forwarded[0].3.as_deref(), Some("llm_timeout"));
    assert_eq!(forwarded[0].4.as_deref(), Some("model took too long"));
}

#[test]
fn an_errored_status_that_is_not_forwarded_persists_and_emits_an_error_event_instead_of_a_created_message(
) {
    let sink = FakeSink::new("sess-1").without_forwarding();
    let mut body = reply_body("sess-1", "errored");
    body.error_code = Some("llm_timeout".to_string());
    body.error_message = Some("model took too long".to_string());

    let disposition = route_reply(&sink, TASK_ID, body);

    assert_eq!(disposition, ReplyDisposition::Accepted);
    let appended = sink.appended.lock().unwrap();
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0].status, "errored");
    assert_eq!(appended[0].content, "");
    let errored = sink.errored_events.lock().unwrap();
    assert_eq!(errored.len(), 1);
    assert_eq!(errored[0]["error_code"], json!("llm_timeout"));
}

#[test]
fn a_non_terminal_typing_status_does_not_complete_the_turn() {
    let sink = FakeSink::new("sess-1");
    let body = reply_body("sess-1", "typing");

    let disposition = route_reply(&sink, TASK_ID, body);

    assert_eq!(disposition, ReplyDisposition::Accepted);
    assert!(sink.finalized.lock().unwrap().is_empty());
    assert!(sink.forwarded_turns.lock().unwrap().is_empty());
    assert_eq!(sink.touch_calls.lock().unwrap().as_slice(), [TASK_ID]);
}

#[path = "routing_tests_dispositions.rs"]
mod dispositions;
