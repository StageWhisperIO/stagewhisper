use super::*;
use crate::store::{ChatAppendOutcome, ChatMsg};
use std::sync::{Barrier, Mutex as StdMutex};
use std::thread;

const BINDING: &str = "acct-1";
const OTHER_BINDING: &str = "acct-2";

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

    fn failing() -> Self {
        Self {
            calls: StdMutex::new(Vec::new()),
            outcome: Err("disk full".to_string()),
        }
    }

    fn reporting(outcome: ChatAppendOutcome) -> Self {
        Self {
            calls: StdMutex::new(Vec::new()),
            outcome: Ok(outcome),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
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

fn register_bound(registry: &TurnRegistry) -> Arc<StdMutex<Vec<UiChunk>>> {
    let (sink, received) = recording_sink();
    let _ = registry
        .register(
            "turn-1",
            "sess-1",
            "user-1",
            "text-1",
            Some(BINDING.to_string()),
            sink,
        )
        .unwrap();
    received.lock().unwrap().clear();
    received
}

#[test]
fn two_threads_terminating_the_same_turn_concurrently_produce_exactly_one_persistence_and_terminal_sequence(
) {
    let registry = Arc::new(TurnRegistry::new());
    let received = register_bound(&registry);

    let persistence = Arc::new(RecordingPersistence::ok());
    let barrier = Arc::new(Barrier::new(2));

    let workers: Vec<_> = (0..2)
        .map(|i| {
            let registry = registry.clone();
            let persistence = persistence.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                registry.terminate(
                    "turn-1",
                    TurnOutcome::Reply(format!("reply from thread {i}")),
                    Some(BINDING),
                    persistence.as_ref(),
                )
            })
        })
        .collect();

    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();

    let terminated = results
        .iter()
        .filter(|r| {
            matches!(
                r,
                TerminalResult::Terminated(TerminationOutcome::Replied(_))
            )
        })
        .count();
    let already_terminated = results
        .iter()
        .filter(|r| matches!(r, TerminalResult::AlreadyTerminated))
        .count();
    assert_eq!(terminated, 1);
    assert_eq!(already_terminated, 1);

    assert_eq!(persistence.call_count(), 1);
    assert_eq!(
        chunk_kinds(&received.lock().unwrap()),
        vec!["text-delta", "text-end", "finish"]
    );
}

#[test]
fn a_persistence_failure_on_a_reply_outcome_emits_error_and_finish_error_and_never_finish_stop() {
    let registry = TurnRegistry::new();
    let received = register_bound(&registry);

    let persistence = RecordingPersistence::failing();
    let result = registry.terminate(
        "turn-1",
        TurnOutcome::Reply("the reply".to_string()),
        Some(BINDING),
        &persistence,
    );

    assert_eq!(persistence.call_count(), 1);
    assert!(matches!(
        result,
        TerminalResult::Terminated(TerminationOutcome::Failed(_))
    ));

    let events = received.lock().unwrap();
    assert_eq!(chunk_kinds(&events), vec!["error", "finish"]);
    match &events[1] {
        UiChunk::Finish { finish_reason } => assert_eq!(finish_reason, "error"),
        other => panic!("expected finish chunk, got {other:?}"),
    }
}

#[test]
fn a_persistence_failure_on_a_failure_outcome_still_emits_the_error_terminal_sequence() {
    let registry = TurnRegistry::new();
    let received = register_bound(&registry);

    let persistence = RecordingPersistence::failing();
    let result = registry.terminate(
        "turn-1",
        TurnOutcome::Failure("boom".to_string()),
        Some(BINDING),
        &persistence,
    );

    assert_eq!(persistence.call_count(), 1);
    assert!(matches!(
        result,
        TerminalResult::Terminated(TerminationOutcome::Failed(_))
    ));

    let events = received.lock().unwrap();
    assert_eq!(chunk_kinds(&events), vec!["error", "finish"]);
    match &events[1] {
        UiChunk::Finish { finish_reason } => assert_eq!(finish_reason, "error"),
        other => panic!("expected finish chunk, got {other:?}"),
    }
}

#[test]
fn terminate_with_a_mismatched_binding_is_refused_persists_nothing_and_leaves_the_turn_intact() {
    let registry = TurnRegistry::new();
    let received = register_bound(&registry);

    let persistence = RecordingPersistence::ok();
    let result = registry.terminate(
        "turn-1",
        TurnOutcome::Reply("the reply".to_string()),
        Some(OTHER_BINDING),
        &persistence,
    );

    assert_eq!(result, TerminalResult::Unauthorized);
    assert_eq!(persistence.call_count(), 0);
    assert!(received.lock().unwrap().is_empty());
    assert!(registry.contains("turn-1"));

    let legitimate_persistence = RecordingPersistence::ok();
    let legitimate_result = registry.terminate(
        "turn-1",
        TurnOutcome::Reply("the reply".to_string()),
        Some(BINDING),
        &legitimate_persistence,
    );
    assert!(matches!(
        legitimate_result,
        TerminalResult::Terminated(TerminationOutcome::Replied(_))
    ));
    assert_eq!(legitimate_persistence.call_count(), 1);
}

#[test]
fn terminate_with_no_binding_captured_is_refused() {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    let _ = registry
        .register("turn-1", "sess-1", "user-1", "text-1", None, sink)
        .unwrap();
    received.lock().unwrap().clear();

    let persistence = RecordingPersistence::ok();
    let result = registry.terminate(
        "turn-1",
        TurnOutcome::Reply("the reply".to_string()),
        Some(BINDING),
        &persistence,
    );

    assert_eq!(result, TerminalResult::Unauthorized);
    assert_eq!(persistence.call_count(), 0);
    assert!(received.lock().unwrap().is_empty());
    assert!(registry.contains("turn-1"));
}

#[test]
fn terminate_with_no_binding_captured_and_no_current_binding_is_still_refused() {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    let _ = registry
        .register("turn-1", "sess-1", "user-1", "text-1", None, sink)
        .unwrap();
    received.lock().unwrap().clear();

    let persistence = RecordingPersistence::ok();
    let result = registry.terminate(
        "turn-1",
        TurnOutcome::Reply("the reply".to_string()),
        None,
        &persistence,
    );

    assert_eq!(result, TerminalResult::Unauthorized);
    assert_eq!(persistence.call_count(), 0);
    assert!(registry.contains("turn-1"));
}

#[test]
fn terminate_on_an_unregistered_turn_id_reports_unknown() {
    let registry = TurnRegistry::new();
    let persistence = RecordingPersistence::ok();

    let result = registry.terminate(
        "never-registered",
        TurnOutcome::Reply("x".to_string()),
        Some(BINDING),
        &persistence,
    );

    assert_eq!(result, TerminalResult::Unknown);
    assert_eq!(persistence.call_count(), 0);
}

#[test]
fn resume_with_a_mismatched_binding_is_refused() {
    let registry = TurnRegistry::new();
    let _received = register_bound(&registry);

    let (resume_sink, resume_received) = recording_sink();
    let resumed = registry.resume("sess-1", Some(OTHER_BINDING), resume_sink);

    assert!(resumed.is_none());
    assert!(resume_received.lock().unwrap().is_empty());
    assert!(registry.contains("turn-1"));
}

#[test]
fn resume_with_a_matching_binding_succeeds() {
    let registry = TurnRegistry::new();
    let _received = register_bound(&registry);

    let (resume_sink, resume_received) = recording_sink();
    let resumed = registry.resume("sess-1", Some(BINDING), resume_sink);

    assert_eq!(resumed, Some("turn-1".to_string()));
    assert_eq!(
        chunk_kinds(&resume_received.lock().unwrap()),
        vec!["start", "text-start"]
    );
}

#[test]
fn terminate_emits_the_error_terminal_sequence_not_finish_stop_when_persistence_reports_a_conflicting_duplicate(
) {
    let registry = TurnRegistry::new();
    let received = register_bound(&registry);

    let persistence = RecordingPersistence::reporting(ChatAppendOutcome::ConflictingDuplicate);
    let result = registry.terminate(
        "turn-1",
        TurnOutcome::Reply("the reply".to_string()),
        Some(BINDING),
        &persistence,
    );

    assert_eq!(persistence.call_count(), 1);
    assert!(matches!(
        result,
        TerminalResult::Terminated(TerminationOutcome::Failed(_))
    ));

    let events = received.lock().unwrap();
    assert_eq!(chunk_kinds(&events), vec!["error", "finish"]);
    match &events[1] {
        UiChunk::Finish { finish_reason } => assert_eq!(finish_reason, "error"),
        other => panic!("expected finish chunk, got {other:?}"),
    }
}

#[test]
fn terminate_emits_the_error_terminal_sequence_when_persistence_reports_a_missing_session() {
    let registry = TurnRegistry::new();
    let received = register_bound(&registry);

    let persistence = RecordingPersistence::reporting(ChatAppendOutcome::MissingSession);
    let result = registry.terminate(
        "turn-1",
        TurnOutcome::Reply("the reply".to_string()),
        Some(BINDING),
        &persistence,
    );

    assert_eq!(persistence.call_count(), 1);
    assert!(matches!(
        result,
        TerminalResult::Terminated(TerminationOutcome::Failed(_))
    ));

    let events = received.lock().unwrap();
    assert_eq!(chunk_kinds(&events), vec!["error", "finish"]);
    match &events[1] {
        UiChunk::Finish { finish_reason } => assert_eq!(finish_reason, "error"),
        other => panic!("expected finish chunk, got {other:?}"),
    }
}

#[test]
fn terminate_emits_the_success_terminal_sequence_when_persistence_reports_an_identical_duplicate() {
    let registry = TurnRegistry::new();
    let received = register_bound(&registry);

    let persistence = RecordingPersistence::reporting(ChatAppendOutcome::IdenticalDuplicate);
    let result = registry.terminate(
        "turn-1",
        TurnOutcome::Reply("the reply".to_string()),
        Some(BINDING),
        &persistence,
    );

    assert_eq!(persistence.call_count(), 1);
    assert!(matches!(
        result,
        TerminalResult::Terminated(TerminationOutcome::Replied(_))
    ));

    let events = received.lock().unwrap();
    assert_eq!(
        chunk_kinds(&events),
        vec!["text-delta", "text-end", "finish"]
    );
    match events.last() {
        Some(UiChunk::Finish { finish_reason }) => assert_eq!(finish_reason, "stop"),
        other => panic!("expected finish chunk, got {other:?}"),
    }
}
