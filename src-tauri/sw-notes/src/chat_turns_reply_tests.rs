use super::*;
use crate::store::{ChatAppendOutcome, ChatMsg};
use std::sync::Mutex as StdMutex;

const BINDING: &str = "acct-1";

fn recording_sink() -> (
    impl Fn(UiChunk) + Send + Sync + 'static,
    Arc<StdMutex<Vec<UiChunk>>>,
) {
    let received: Arc<StdMutex<Vec<UiChunk>>> = Arc::new(StdMutex::new(Vec::new()));
    let recorder = received.clone();
    (
        move |chunk: UiChunk| recorder.lock().unwrap().push(chunk),
        received,
    )
}

fn deltas(events: &[UiChunk]) -> Vec<String> {
    events
        .iter()
        .filter_map(|chunk| match chunk {
            UiChunk::TextDelta { delta, .. } => Some(delta.clone()),
            _ => None,
        })
        .collect()
}

fn start_message_id(events: &[UiChunk]) -> String {
    events
        .iter()
        .find_map(|chunk| match chunk {
            UiChunk::Start { message_id } => Some(message_id.clone()),
            _ => None,
        })
        .expect("expected a start chunk")
}

fn persisted_reply_id(persistence: &RecordingPersistence) -> String {
    persistence
        .calls
        .lock()
        .unwrap()
        .last()
        .expect("expected a persisted message")
        .1
        .id
        .clone()
}

fn persisted_reply_content(persistence: &RecordingPersistence) -> String {
    persistence
        .calls
        .lock()
        .unwrap()
        .last()
        .expect("expected a persisted message")
        .1
        .content
        .clone()
}

fn chunk_kinds(events: &[UiChunk]) -> Vec<&'static str> {
    events
        .iter()
        .map(|chunk| match chunk {
            UiChunk::Start { .. } => "start",
            UiChunk::TextStart { .. } => "text-start",
            UiChunk::TextDelta { .. } => "text-delta",
            UiChunk::TextEnd { .. } => "text-end",
            UiChunk::Activity { .. } => "activity",
            UiChunk::Error { .. } => "error",
            UiChunk::Finish { .. } => "finish",
        })
        .collect()
}

struct RecordingPersistence {
    calls: StdMutex<Vec<(String, ChatMsg)>>,
    outcome: Result<ChatAppendOutcome, String>,
}

impl RecordingPersistence {
    fn ok() -> Self {
        Self {
            calls: StdMutex::new(Vec::new()),
            outcome: Ok(ChatAppendOutcome::Inserted),
        }
    }
}

impl TurnPersistence for RecordingPersistence {
    fn persist_reply(
        &self,
        session_id: &str,
        message: ChatMsg,
    ) -> Result<ChatAppendOutcome, String> {
        self.calls
            .lock()
            .unwrap()
            .push((session_id.to_string(), message));
        self.outcome.clone()
    }
}

fn terminated_snapshot(result: TerminalResult) -> TurnSnapshot {
    match result {
        TerminalResult::Terminated(TerminationOutcome::Replied(snapshot))
        | TerminalResult::Terminated(TerminationOutcome::Failed(snapshot)) => snapshot,
        other => panic!("expected a terminated outcome, got {other:?}"),
    }
}

#[test]
fn terminate_emits_no_extra_delta_when_the_buffer_already_equals_the_authoritative_content() {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    let _ = registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "text-1",
            Some(BINDING.to_string()),
            sink,
        )
        .unwrap();
    let handle = registry.get("turn-1").unwrap();
    handle.push_text_delta("already streamed");
    received.lock().unwrap().clear();

    let persistence = RecordingPersistence::ok();
    let result = registry.terminate(
        "turn-1",
        TurnOutcome::Reply("already streamed".to_string()),
        Some(BINDING),
        &persistence,
    );

    let events = received.lock().unwrap();
    assert!(deltas(&events).is_empty());
    assert_eq!(chunk_kinds(&events), vec!["text-end", "finish"]);
    let snapshot = terminated_snapshot(result);
    assert_eq!(snapshot.session_id, "sess-1");
    assert_eq!(snapshot.parent_message_id, "user-msg-1");
}

#[test]
fn terminate_emits_only_the_missing_suffix_when_the_buffer_is_a_strict_prefix_of_the_authoritative_content(
) {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    let _ = registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "text-1",
            Some(BINDING.to_string()),
            sink,
        )
        .unwrap();
    let handle = registry.get("turn-1").unwrap();
    handle.push_text_delta("the whole ");
    received.lock().unwrap().clear();

    let persistence = RecordingPersistence::ok();
    registry.terminate(
        "turn-1",
        TurnOutcome::Reply("the whole reply".to_string()),
        Some(BINDING),
        &persistence,
    );

    let events = received.lock().unwrap();
    assert_eq!(deltas(&events), vec!["reply"]);
}

#[test]
fn terminate_sends_the_full_content_as_one_delta_when_nothing_streamed_yet() {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    let _ = registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "text-1",
            Some(BINDING.to_string()),
            sink,
        )
        .unwrap();
    received.lock().unwrap().clear();

    let persistence = RecordingPersistence::ok();
    registry.terminate(
        "turn-1",
        TurnOutcome::Reply("the whole reply".to_string()),
        Some(BINDING),
        &persistence,
    );

    let events = received.lock().unwrap();
    assert_eq!(deltas(&events), vec!["the whole reply"]);
}

#[test]
fn terminate_emits_nothing_extra_and_keeps_streamed_text_when_the_buffer_diverges_from_the_authoritative_content(
) {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    let _ = registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "text-1",
            Some(BINDING.to_string()),
            sink,
        )
        .unwrap();
    let handle = registry.get("turn-1").unwrap();
    handle.push_text_delta("a completely different draft");
    received.lock().unwrap().clear();

    let persistence = RecordingPersistence::ok();
    registry.terminate(
        "turn-1",
        TurnOutcome::Reply("the final authoritative reply".to_string()),
        Some(BINDING),
        &persistence,
    );

    let events = received.lock().unwrap();
    assert!(deltas(&events).is_empty());
    assert_eq!(chunk_kinds(&events), vec!["text-end", "finish"]);
    assert_eq!(
        handle.buffered_text(),
        "a completely different draft".to_string()
    );
}

#[test]
fn terminate_removes_the_turn_and_returns_already_terminated_when_called_twice() {
    let registry = TurnRegistry::new();
    let (sink, _received) = recording_sink();
    let _ = registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "text-1",
            Some(BINDING.to_string()),
            sink,
        )
        .unwrap();

    let persistence = RecordingPersistence::ok();
    let first = registry.terminate(
        "turn-1",
        TurnOutcome::Reply("reply".to_string()),
        Some(BINDING),
        &persistence,
    );
    assert!(matches!(
        first,
        TerminalResult::Terminated(TerminationOutcome::Replied(_))
    ));
    assert!(!registry.contains("turn-1"));

    let second_persistence = RecordingPersistence::ok();
    let second = registry.terminate(
        "turn-1",
        TurnOutcome::Reply("reply".to_string()),
        Some(BINDING),
        &second_persistence,
    );
    assert_eq!(second, TerminalResult::AlreadyTerminated);
    assert!(second_persistence.calls.lock().unwrap().is_empty());
}

#[test]
fn terminate_with_failure_emits_error_then_finish_with_error_reason() {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    let _ = registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "text-1",
            Some(BINDING.to_string()),
            sink,
        )
        .unwrap();
    received.lock().unwrap().clear();

    let persistence = RecordingPersistence::ok();
    let result = registry.terminate(
        "turn-1",
        TurnOutcome::Failure("boom".to_string()),
        Some(BINDING),
        &persistence,
    );

    let events = received.lock().unwrap();
    assert_eq!(chunk_kinds(&events), vec!["error", "finish"]);
    match &events[0] {
        UiChunk::Error { error_text } => assert_eq!(error_text, "boom"),
        other => panic!("expected error chunk, got {other:?}"),
    }
    match &events[1] {
        UiChunk::Finish { finish_reason } => assert_eq!(finish_reason, "error"),
        other => panic!("expected finish chunk, got {other:?}"),
    }
    let snapshot = terminated_snapshot(result);
    assert_eq!(snapshot.session_id, "sess-1");
    assert_eq!(snapshot.parent_message_id, "user-msg-1");
}

#[test]
fn terminate_with_failure_removes_the_turn_and_returns_already_terminated_when_called_twice() {
    let registry = TurnRegistry::new();
    let (sink, _received) = recording_sink();
    let _ = registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "text-1",
            Some(BINDING.to_string()),
            sink,
        )
        .unwrap();

    let persistence = RecordingPersistence::ok();
    assert!(matches!(
        registry.terminate(
            "turn-1",
            TurnOutcome::Failure("boom".to_string()),
            Some(BINDING),
            &persistence,
        ),
        TerminalResult::Terminated(TerminationOutcome::Failed(_))
    ));
    assert!(!registry.contains("turn-1"));

    let second_persistence = RecordingPersistence::ok();
    assert_eq!(
        registry.terminate(
            "turn-1",
            TurnOutcome::Failure("boom".to_string()),
            Some(BINDING),
            &second_persistence,
        ),
        TerminalResult::AlreadyTerminated
    );
}

#[test]
fn terminating_one_turn_does_not_affect_another_turn_on_the_same_session() {
    let registry = TurnRegistry::new();
    let (sink_a, received_a) = recording_sink();
    let _ = registry
        .register(
            "turn-a",
            "sess-shared",
            "user-msg-a",
            "text-a",
            Some(BINDING.to_string()),
            sink_a,
        )
        .unwrap();
    let (sink_b, received_b) = recording_sink();
    let _ = registry
        .register(
            "turn-b",
            "sess-shared",
            "user-msg-b",
            "text-b",
            Some(BINDING.to_string()),
            sink_b,
        )
        .unwrap();
    received_a.lock().unwrap().clear();
    received_b.lock().unwrap().clear();

    let persistence = RecordingPersistence::ok();
    registry.terminate(
        "turn-a",
        TurnOutcome::Reply("reply a".to_string()),
        Some(BINDING),
        &persistence,
    );

    assert!(!registry.contains("turn-a"));
    assert!(registry.contains("turn-b"));
    assert!(!deltas(&received_a.lock().unwrap()).is_empty());
    assert!(received_b.lock().unwrap().is_empty());
}

#[test]
fn the_streamed_assistant_message_id_equals_the_id_the_reply_is_persisted_under() {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    let _ = registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "text-1",
            Some(BINDING.to_string()),
            sink,
        )
        .unwrap();
    let streamed_message_id = start_message_id(&received.lock().unwrap());

    let persistence = RecordingPersistence::ok();
    registry.terminate(
        "turn-1",
        TurnOutcome::Reply("the answer".to_string()),
        Some(BINDING),
        &persistence,
    );

    assert_eq!(streamed_message_id, persisted_reply_id(&persistence));
}

#[test]
fn a_turn_resumed_before_its_reply_is_persisted_keeps_the_same_message_id_as_the_eventual_persisted_reply(
) {
    let registry = TurnRegistry::new();
    let (initial_sink, initial_received) = recording_sink();
    let _ = registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "text-1",
            Some(BINDING.to_string()),
            initial_sink,
        )
        .unwrap();
    let initial_message_id = start_message_id(&initial_received.lock().unwrap());

    let (resume_sink, resume_received) = recording_sink();
    let resumed_turn_id = registry
        .resume("sess-1", Some(BINDING), resume_sink)
        .unwrap();
    assert_eq!(resumed_turn_id, "turn-1");
    let resumed_message_id = start_message_id(&resume_received.lock().unwrap());
    assert_eq!(resumed_message_id, initial_message_id);

    let persistence = RecordingPersistence::ok();
    registry.terminate(
        "turn-1",
        TurnOutcome::Reply("the answer".to_string()),
        Some(BINDING),
        &persistence,
    );

    assert_eq!(persisted_reply_id(&persistence), initial_message_id);
}

#[test]
fn streaming_a_part_and_terminating_a_bound_turn_demand_the_same_binding() {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "text-1",
            Some(BINDING.to_string()),
            sink,
        )
        .unwrap();

    assert!(!registry.push_text_delta("turn-1", None, "unbound part"));
    assert_eq!(
        registry.terminate(
            "turn-1",
            TurnOutcome::Reply("unbound".to_string()),
            None,
            &RecordingPersistence::ok(),
        ),
        TerminalResult::Unauthorized
    );

    assert!(registry.push_text_delta("turn-1", Some(BINDING), "bound part"));
    assert_eq!(deltas(&received.lock().unwrap()), vec!["bound part"]);
}

#[test]
fn a_terminal_carrying_no_text_persists_the_parts_that_were_streamed() {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "text-1",
            Some(BINDING.to_string()),
            sink,
        )
        .unwrap();

    assert!(registry.push_text_delta("turn-1", Some(BINDING), "the whole answer"));

    let persistence = RecordingPersistence::ok();
    registry.terminate(
        "turn-1",
        TurnOutcome::Reply(String::new()),
        Some(BINDING),
        &persistence,
    );

    assert_eq!(persisted_reply_content(&persistence), "the whole answer");
    assert_eq!(deltas(&received.lock().unwrap()), vec!["the whole answer"]);
}

#[test]
fn separate_message_parts_keep_their_boundaries_when_the_terminal_carries_no_text() {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "text-1",
            Some(BINDING.to_string()),
            sink,
        )
        .unwrap();

    assert!(registry.push_message_part("turn-1", Some(BINDING), "let me check"));
    assert!(registry.push_message_part("turn-1", Some(BINDING), "here is the answer"));

    let persistence = RecordingPersistence::ok();
    registry.terminate(
        "turn-1",
        TurnOutcome::Reply(String::new()),
        Some(BINDING),
        &persistence,
    );

    assert_eq!(
        persisted_reply_content(&persistence),
        "let me check\n\nhere is the answer"
    );
    assert_eq!(
        deltas(&received.lock().unwrap()),
        vec!["let me check", "\n\nhere is the answer"]
    );
}

#[test]
fn a_message_part_that_restates_the_streamed_buffer_is_not_appended_twice() {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "text-1",
            Some(BINDING.to_string()),
            sink,
        )
        .unwrap();

    assert!(registry.push_text_delta("turn-1", Some(BINDING), "Hello"));
    assert!(registry.push_text_delta("turn-1", Some(BINDING), " world"));
    registry.push_message_part("turn-1", Some(BINDING), "Hello world");

    let persistence = RecordingPersistence::ok();
    registry.terminate(
        "turn-1",
        TurnOutcome::Reply("Hello world".to_string()),
        Some(BINDING),
        &persistence,
    );

    assert_eq!(persisted_reply_content(&persistence), "Hello world");
    assert_eq!(deltas(&received.lock().unwrap()), vec!["Hello", " world"]);
}

#[test]
fn a_message_part_that_extends_the_streamed_buffer_only_adds_the_new_suffix() {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "text-1",
            Some(BINDING.to_string()),
            sink,
        )
        .unwrap();

    assert!(registry.push_text_delta("turn-1", Some(BINDING), "Hello"));
    registry.push_message_part("turn-1", Some(BINDING), "Hello world");

    let persistence = RecordingPersistence::ok();
    registry.terminate(
        "turn-1",
        TurnOutcome::Reply("Hello world".to_string()),
        Some(BINDING),
        &persistence,
    );

    assert_eq!(persisted_reply_content(&persistence), "Hello world");
    assert_eq!(deltas(&received.lock().unwrap()), vec!["Hello", " world"]);
}

#[test]
fn a_final_message_part_that_only_trimmed_the_leading_blank_lines_is_not_appended_a_second_time() {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "text-1",
            Some(BINDING.to_string()),
            sink,
        )
        .unwrap();

    assert!(registry.push_text_delta("turn-1", Some(BINDING), "\n\nYes, absolutely"));
    assert!(registry.push_text_delta("turn-1", Some(BINDING), " and here is the answer"));
    registry.push_message_part(
        "turn-1",
        Some(BINDING),
        "Yes, absolutely and here is the answer",
    );

    let persistence = RecordingPersistence::ok();
    registry.terminate(
        "turn-1",
        TurnOutcome::Reply(String::new()),
        Some(BINDING),
        &persistence,
    );

    assert_eq!(
        persisted_reply_content(&persistence),
        "\n\nYes, absolutely and here is the answer"
    );
    assert_eq!(
        deltas(&received.lock().unwrap()),
        vec!["\n\nYes, absolutely", " and here is the answer"]
    );
}
