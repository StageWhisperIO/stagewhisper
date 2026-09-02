use super::*;
use std::sync::Mutex as StdMutex;

use sw_notes::{ChatAppendOutcome, ChatMsg, TerminationOutcome, UiChunk};

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

struct FailingPersistence;

impl TurnPersistence for FailingPersistence {
    fn persist_reply(
        &self,
        _session_id: &str,
        _message: ChatMsg,
    ) -> Result<ChatAppendOutcome, String> {
        Err("disk full".to_string())
    }
}

#[test]
fn a_local_reply_whose_persistence_fails_emits_an_error_terminal_sequence_not_a_successful_finish()
{
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    registry
        .register(
            "turn-1",
            "sess-1",
            "user-1",
            "turn-1",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            sink,
        )
        .expect("fresh turn id registers");
    received.lock().unwrap().clear();

    let result = finalize_local_reply(
        &registry,
        &FailingPersistence,
        "turn-1",
        TurnOutcome::Reply("the local answer".to_string()),
    );

    assert!(matches!(
        result,
        TerminalResult::Terminated(TerminationOutcome::Failed(_))
    ));
    let events = received.lock().unwrap();
    assert_eq!(chunk_kinds(&events), vec!["error", "finish"]);
    match events.last() {
        Some(UiChunk::Finish { finish_reason }) => assert_eq!(finish_reason, "error"),
        other => panic!("expected a finish chunk, got {other:?}"),
    }
}

#[test]
fn a_turn_that_cannot_be_enqueued_ends_with_an_error_terminal_outcome_rather_than_hanging() {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    registry
        .register(
            "turn-1",
            "sess-1",
            "user-1",
            "turn-1",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            sink,
        )
        .expect("fresh turn id registers");
    received.lock().unwrap().clear();

    let result = finalize_local_reply(
        &registry,
        &UnavailablePersistence,
        "turn-1",
        TurnOutcome::Failure(LOCAL_TURN_QUEUE_UNAVAILABLE_ERROR_TEXT.to_string()),
    );

    assert!(matches!(
        result,
        TerminalResult::Terminated(TerminationOutcome::Failed(_))
    ));
    let events = received.lock().unwrap();
    assert_eq!(chunk_kinds(&events), vec!["error", "finish"]);
    match events.last() {
        Some(UiChunk::Finish { finish_reason }) => assert_eq!(finish_reason, "error"),
        other => panic!("expected a finish chunk, got {other:?}"),
    }
}
