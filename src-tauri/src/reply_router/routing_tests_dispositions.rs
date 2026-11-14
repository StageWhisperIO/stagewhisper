use super::*;

struct ProbeArmedSink {
    armed: Mutex<HashSet<String>>,
    resolved: Mutex<Vec<ProbeOutcome>>,
}

impl ProbeArmedSink {
    fn new() -> Self {
        Self {
            armed: Mutex::new(HashSet::new()),
            resolved: Mutex::new(Vec::new()),
        }
    }

    fn arm(&self, task_id: &str) {
        self.armed.lock().unwrap().insert(task_id.to_string());
    }
}

impl ReplySink for ProbeArmedSink {
    fn current_session_id(&self) -> Option<String> {
        None
    }
    fn session_known(&self, _session_id: &str) -> bool {
        false
    }
    fn append_message(&self, _message: ChatMessagePayload) -> bool {
        true
    }
    fn emit_created(&self, _payload: &ChatMessagePayload) {}
    fn emit_errored(&self, _payload: &Value) {}
    fn reserve_terminal(&self, _task_id: &str, _session_id: &str) -> ReserveResult {
        ReserveResult::Reserved
    }
    fn release_terminal(&self, _task_id: &str) {}
    fn complete_terminal(&self, _task_id: &str) {}
    fn resolve_probe(&self, task_id: &str, outcome: ProbeOutcome) -> bool {
        if self.armed.lock().unwrap().remove(task_id) {
            self.resolved.lock().unwrap().push(outcome);
            true
        } else {
            false
        }
    }
}

fn completed_body(session_id: &str, text: &str) -> ReplyBody {
    let mut body = reply_body(session_id, "completed");
    body.reply_text = Some(text.to_string());
    body
}

#[test]
fn an_unknown_status_string_is_rejected_as_invalid() {
    let sink = FakeSink::new("sess-1");
    let body = reply_body("sess-1", "pending");

    let disposition = route_reply(&sink, TASK_ID, body);

    assert_eq!(disposition, ReplyDisposition::InvalidStatus);
}

#[test]
fn a_reply_body_task_id_that_disagrees_with_the_route_task_id_is_rejected() {
    let sink = FakeSink::new("sess-1");
    let mut body = reply_body("sess-1", "completed");
    body.task_id = Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string());

    let disposition = route_reply(&sink, TASK_ID, body);

    assert_eq!(disposition, ReplyDisposition::TaskIdMismatch);
    assert!(sink.appended.lock().unwrap().is_empty());
}

#[test]
fn a_non_uuid_route_task_id_is_rejected_before_any_other_validation() {
    let sink = FakeSink::new("sess-1");
    let body = reply_body("sess-1", "completed");

    let disposition = route_reply(&sink, "not-a-uuid", body);

    assert_eq!(disposition, ReplyDisposition::InvalidTaskId);
}

#[test]
fn a_message_status_reply_resolves_an_armed_probe_before_any_session_gating() {
    let sink = ProbeArmedSink::new();
    sink.arm(TASK_ID);
    let mut body = reply_body("probe-session-not-known-or-current", "message");
    body.reply_text = Some("ready when you are".to_string());

    let disposition = route_reply(&sink, TASK_ID, body);

    assert_eq!(disposition, ReplyDisposition::ProbeResolved);
    let resolved = sink.resolved.lock().unwrap();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].status, "message");
    assert_eq!(
        resolved[0].reply_text.as_deref(),
        Some("ready when you are")
    );
}

#[test]
fn a_completed_status_reply_also_resolves_an_armed_probe_before_any_session_gating() {
    let sink = ProbeArmedSink::new();
    sink.arm(TASK_ID);
    let body = completed_body(
        "probe-session-not-known-or-current",
        "hi, I don't recognize you yet",
    );

    let disposition = route_reply(&sink, TASK_ID, body);

    assert_eq!(disposition, ReplyDisposition::ProbeResolved);
}

#[test]
fn a_message_status_reply_that_is_not_forwarded_is_appended_as_a_hashed_chat_message() {
    let sink = FakeSink::new("sess-1").without_forwarding();
    let mut body = reply_body("sess-1", "message");
    body.reply_text = Some("coaching cue".to_string());

    let disposition = route_reply(&sink, TASK_ID, body);

    assert_eq!(disposition, ReplyDisposition::Accepted);
    let appended = sink.appended.lock().unwrap();
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0].content, "coaching cue");
    assert_eq!(appended[0].role, "assistant");
    assert_eq!(appended[0].status, "completed");
    assert_ne!(appended[0].id, TASK_ID);
    assert_eq!(sink.created_events.lock().unwrap().len(), 1);
}

#[test]
fn a_reply_for_a_different_unknown_session_while_a_session_is_current_is_dropped_as_a_session_mismatch(
) {
    let sink = FakeSink::new("sess-current");
    let body = completed_body("sess-other", "stale reply");

    let disposition = route_reply(&sink, TASK_ID, body);

    assert_eq!(
        disposition,
        ReplyDisposition::Dropped(DropReason::SessionMismatch)
    );
    assert!(sink.appended.lock().unwrap().is_empty());
    assert!(sink.created_events.lock().unwrap().is_empty());
}

#[test]
fn a_reply_for_an_unknown_session_with_no_current_session_is_dropped_as_session_ended() {
    let sink = FakeSink::without_current_session();
    let body = completed_body("sess-ended", "reply for ended session");

    let disposition = route_reply(&sink, TASK_ID, body);

    assert_eq!(
        disposition,
        ReplyDisposition::Dropped(DropReason::SessionEnded)
    );
    assert!(sink.appended.lock().unwrap().is_empty());
}

#[test]
fn a_completed_reply_for_a_known_stored_session_with_no_live_session_persists_normally() {
    let sink = FakeSink::stored("sess-stored").without_forwarding();
    let body = completed_body("sess-stored", "# Summary\n## Action Items\n- [ ] follow up");

    let disposition = route_reply(&sink, TASK_ID, body);

    assert_eq!(disposition, ReplyDisposition::Accepted);
    let appended = sink.appended.lock().unwrap();
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0].session_id, "sess-stored");
    assert_eq!(appended[0].status, "completed");
    assert_eq!(sink.created_events.lock().unwrap().len(), 1);
}

#[test]
fn a_completed_reply_that_mentions_pairing_language_is_stored_verbatim_not_reclassified() {
    let sink = FakeSink::new("sess-1").without_forwarding();
    let reply =
        "Sure, to approve a teammate, run `hermes pairing approve stagewhisper ABCD1234` on the host.";
    let body = completed_body("sess-1", reply);

    let disposition = route_reply(&sink, TASK_ID, body);

    assert_eq!(disposition, ReplyDisposition::Accepted);
    let appended = sink.appended.lock().unwrap();
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0].status, "completed");
    assert_eq!(
        appended[0].content, reply,
        "content-path replies are relayed verbatim, never reclassified as pairing"
    );
    assert!(appended[0].error_code.is_none());
    assert!(sink.errored_events.lock().unwrap().is_empty());
}

#[test]
fn a_silent_status_with_a_user_message_id_persists_as_a_cancelled_terminal_message() {
    let sink = FakeSink::new("sess-1").without_forwarding();
    let body = reply_body("sess-1", "silent");

    let disposition = route_reply(&sink, TASK_ID, body);

    assert_eq!(disposition, ReplyDisposition::Accepted);
    let appended = sink.appended.lock().unwrap();
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0].status, "cancelled");
    assert_eq!(appended[0].role, "assistant");
    assert!(appended[0].finalized_at.is_some());
    assert_eq!(sink.created_events.lock().unwrap().len(), 1);
    assert!(sink.errored_events.lock().unwrap().is_empty());
}

#[test]
fn a_failed_persist_does_not_claim_the_terminal_slot_and_allows_a_successful_retry() {
    let sink = FakeSink::new("sess-1").without_forwarding();
    sink.fail_appends();

    let first = route_reply(&sink, TASK_ID, completed_body("sess-1", "hello"));
    assert_eq!(first, ReplyDisposition::PersistFailed);
    assert!(sink.finalized.lock().unwrap().is_empty());
    assert!(sink.created_events.lock().unwrap().is_empty());

    sink.allow_appends();
    let second = route_reply(&sink, TASK_ID, completed_body("sess-1", "hello"));
    assert_eq!(second, ReplyDisposition::Accepted);
    assert_eq!(sink.appended.lock().unwrap().len(), 1);
    assert_eq!(sink.finalized.lock().unwrap().len(), 1);
    assert_eq!(sink.created_events.lock().unwrap().len(), 1);
}
