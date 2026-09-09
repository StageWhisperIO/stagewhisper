use super::*;
use crate::chat_stream::ActivityData;
use std::sync::Mutex as StdMutex;
use std::thread;

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

fn joined_deltas(events: &[UiChunk]) -> String {
    deltas(events).concat()
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
fn registering_a_turn_emits_start_and_text_start() {
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

    let events = received.lock().unwrap();
    assert_eq!(chunk_kinds(&events), vec!["start", "text-start"]);
}

#[test]
fn resume_replays_the_buffer_exactly_once_under_a_single_lock() {
    let registry = TurnRegistry::new();
    let (initial_sink, _initial_received) = recording_sink();
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
    let handle = registry.get("turn-1").unwrap();
    handle.push_text_delta("hel");
    handle.push_text_delta("lo");

    let (resume_sink, resume_received) = recording_sink();
    let resumed_turn_id = registry
        .resume("sess-1", Some(BINDING), resume_sink)
        .unwrap();
    assert_eq!(resumed_turn_id, "turn-1");

    assert_eq!(deltas(&resume_received.lock().unwrap()), vec!["hello"]);

    let (second_resume_sink, second_resume_received) = recording_sink();
    registry
        .resume("sess-1", Some(BINDING), second_resume_sink)
        .unwrap();
    assert_eq!(
        deltas(&second_resume_received.lock().unwrap()),
        vec!["hello"]
    );
}

#[test]
fn resume_does_not_replay_an_empty_buffer_as_a_delta() {
    let registry = TurnRegistry::new();
    let (initial_sink, _initial_received) = recording_sink();
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

    let (resume_sink, resume_received) = recording_sink();
    registry
        .resume("sess-1", Some(BINDING), resume_sink)
        .unwrap();

    let events = resume_received.lock().unwrap();
    assert_eq!(chunk_kinds(&events), vec!["start", "text-start"]);
}

#[test]
fn resume_returns_none_for_a_session_with_no_turns() {
    let registry = TurnRegistry::new();
    let (sink, _received) = recording_sink();
    assert!(registry
        .resume("missing-session", Some(BINDING), sink)
        .is_none());
}

#[test]
fn resume_picks_the_most_recently_registered_turn_when_a_session_has_several_in_flight() {
    let registry = TurnRegistry::new();
    let (first_sink, _first_received) = recording_sink();
    let _ = registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "text-1",
            Some(BINDING.to_string()),
            first_sink,
        )
        .unwrap();
    let (second_sink, _second_received) = recording_sink();
    let _ = registry
        .register(
            "turn-2",
            "sess-1",
            "user-msg-2",
            "text-2",
            Some(BINDING.to_string()),
            second_sink,
        )
        .unwrap();

    let (resume_sink, _resume_received) = recording_sink();
    let resumed_turn_id = registry
        .resume("sess-1", Some(BINDING), resume_sink)
        .unwrap();

    assert_eq!(resumed_turn_id, "turn-2");
}

#[test]
fn two_concurrent_turns_on_the_same_session_never_cross_deliver_chunks() {
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

    assert!(registry.push_text_delta("turn-a", Some(BINDING), "for a"));
    assert!(registry.push_text_delta("turn-b", Some(BINDING), "for b"));
    assert!(registry.push_activity("turn-a", "a working"));

    assert_eq!(deltas(&received_a.lock().unwrap()), vec!["for a"]);
    assert_eq!(deltas(&received_b.lock().unwrap()), vec!["for b"]);
}

#[test]
fn concurrent_pushes_during_resume_produce_no_duplicated_or_missing_text() {
    let registry = TurnRegistry::new();
    let (initial_sink, _initial_received) = recording_sink();
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
    let handle = registry.get("turn-1").unwrap();

    let expected_total: Arc<StdMutex<String>> = Arc::new(StdMutex::new(String::new()));
    let pusher_handle = handle.clone();
    let pusher_expected = expected_total.clone();
    let pusher = thread::spawn(move || {
        for i in 0..500 {
            let chunk = format!("[{i}]");
            pusher_handle.push_text_delta(&chunk);
            pusher_expected.lock().unwrap().push_str(&chunk);
        }
    });

    let (resume_sink, resume_received) = recording_sink();
    let resumed_turn_id = registry
        .resume("sess-1", Some(BINDING), resume_sink)
        .unwrap();
    assert_eq!(resumed_turn_id, "turn-1");

    pusher.join().unwrap();

    handle.push_text_delta("[final]");
    expected_total.lock().unwrap().push_str("[final]");

    let received_text = joined_deltas(&resume_received.lock().unwrap());
    assert_eq!(received_text, *expected_total.lock().unwrap());
}

#[test]
fn push_text_delta_and_push_activity_via_registry_are_no_ops_for_an_unknown_turn() {
    let registry = TurnRegistry::new();
    assert!(!registry.push_text_delta("missing-turn", None, "hi"));
    assert!(!registry.push_activity("missing-turn", "thinking"));
}

#[test]
fn push_text_delta_via_registry_is_rejected_when_the_current_binding_no_longer_matches_the_captured_binding(
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
    received.lock().unwrap().clear();

    assert!(!registry.push_text_delta("turn-1", Some("acct-2"), "a stale fragment"));
    assert!(!registry.push_text_delta("turn-1", None, "another stale fragment"));
    assert!(deltas(&received.lock().unwrap()).is_empty());
    assert!(registry.contains("turn-1"));
}

#[test]
fn handle_emit_delivers_an_arbitrary_chunk_without_ending_the_turn() {
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

    handle.emit(UiChunk::Activity {
        data: ActivityData {
            label: "Waiting for a reply...".to_string(),
        },
        transient: true,
    });

    assert_eq!(chunk_kinds(&received.lock().unwrap()), vec!["activity"]);
    assert!(registry.contains("turn-1"));
}
