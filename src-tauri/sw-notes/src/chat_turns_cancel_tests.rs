use super::*;
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

#[test]
fn cancel_removes_the_turn_and_further_registry_routed_calls_are_no_ops() {
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

    assert!(registry.cancel("turn-1"));
    assert!(!registry.contains("turn-1"));
    assert!(!registry.cancel("turn-1"));
    assert_eq!(
        chunk_kinds(&received.lock().unwrap()),
        vec!["error", "finish"]
    );
    received.lock().unwrap().clear();

    assert!(!registry.push_text_delta("turn-1", Some(BINDING), "late"));
    assert!(!registry.push_activity("turn-1", "still working"));
    assert!(received.lock().unwrap().is_empty());
}

#[test]
fn cancel_emits_the_terminal_sequence_exactly_once_and_deregisters() {
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

    assert!(registry.cancel("turn-1"));
    assert!(!registry.contains("turn-1"));

    let events = received.lock().unwrap();
    assert_eq!(chunk_kinds(&events), vec!["error", "finish"]);
    match &events[0] {
        UiChunk::Error { error_text } => assert_eq!(error_text, "This response was cancelled."),
        other => panic!("expected error chunk, got {other:?}"),
    }
    match &events[1] {
        UiChunk::Finish { finish_reason } => assert_eq!(finish_reason, "error"),
        other => panic!("expected finish chunk, got {other:?}"),
    }
}

#[test]
fn cancelling_an_already_cancelled_turn_emits_nothing_the_second_time() {
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

    assert!(registry.cancel("turn-1"));
    received.lock().unwrap().clear();

    assert!(!registry.cancel("turn-1"));
    assert!(received.lock().unwrap().is_empty());
}

#[test]
fn cancel_stops_emission_from_a_producer_still_holding_the_handle_directly() {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    let handle = registry
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

    assert!(registry.cancel("turn-1"));
    assert!(handle.is_cancelled());
    received.lock().unwrap().clear();

    handle.push_text_delta("chunk pushed after cancel");
    handle.push_activity("still working after cancel");
    assert!(received.lock().unwrap().is_empty());
}

#[test]
fn cancel_does_not_require_a_binding_and_ignores_the_one_captured_at_registration() {
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    let _ = registry
        .register("turn-1", "sess-1", "user-msg-1", "text-1", None, sink)
        .unwrap();
    received.lock().unwrap().clear();

    assert!(registry.cancel("turn-1"));
    assert!(!registry.contains("turn-1"));
}
