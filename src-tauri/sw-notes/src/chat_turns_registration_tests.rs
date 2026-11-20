use super::*;
use crate::store::{ChatAppendOutcome, ChatMsg};
use std::sync::{Barrier, Mutex as StdMutex};
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

struct NoopPersistence;

impl TurnPersistence for NoopPersistence {
    fn persist_reply(
        &self,
        _session_id: &str,
        _message: ChatMsg,
    ) -> Result<ChatAppendOutcome, String> {
        Ok(ChatAppendOutcome::Inserted)
    }
}

#[test]
fn registering_a_turn_id_that_is_already_live_is_rejected_and_the_original_keeps_routing() {
    let registry = TurnRegistry::new();
    let (sink_first, received_first) = recording_sink();
    let _ = registry
        .register(
            "turn-1",
            "sess-first",
            "user-first",
            "text-first",
            Some(BINDING.to_string()),
            sink_first,
        )
        .unwrap();

    let (sink_second, received_second) = recording_sink();
    let rejected = registry.register(
        "turn-1",
        "sess-second",
        "user-second",
        "text-second",
        Some(BINDING.to_string()),
        sink_second,
    );
    assert!(rejected.is_none());

    received_first.lock().unwrap().clear();
    received_second.lock().unwrap().clear();

    assert!(registry.push_text_delta("turn-1", Some(BINDING), "hello"));
    assert_eq!(deltas(&received_first.lock().unwrap()), vec!["hello"]);
    assert!(received_second.lock().unwrap().is_empty());

    let persistence = NoopPersistence;
    let result = registry.terminate(
        "turn-1",
        TurnOutcome::Reply("hello".to_string()),
        Some(BINDING),
        &persistence,
    );
    let snapshot = match result {
        TerminalResult::Terminated(TerminationOutcome::Replied(snapshot)) => snapshot,
        other => panic!("expected a replied outcome, got {other:?}"),
    };
    assert_eq!(snapshot.session_id, "sess-first");
    assert_eq!(snapshot.parent_message_id, "user-first");
    assert!(received_second.lock().unwrap().is_empty());
}

#[test]
fn a_retired_turn_id_cannot_be_immediately_re_registered() {
    let registry = TurnRegistry::new();
    let (sink, _received) = recording_sink();
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

    let persistence = NoopPersistence;
    assert!(matches!(
        registry.terminate(
            "turn-1",
            TurnOutcome::Reply("reply".to_string()),
            Some(BINDING),
            &persistence,
        ),
        TerminalResult::Terminated(_)
    ));

    let (retry_sink, _retry_received) = recording_sink();
    assert!(registry
        .register(
            "turn-1",
            "sess-2",
            "user-2",
            "text-2",
            Some(BINDING.to_string()),
            retry_sink,
        )
        .is_none());
}

#[test]
fn concurrent_registration_of_the_same_turn_id_from_two_threads_yields_exactly_one_success() {
    let registry = Arc::new(TurnRegistry::new());
    let barrier = Arc::new(Barrier::new(2));

    let workers: Vec<_> = (0..2)
        .map(|_| {
            let registry = registry.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                let (sink, _received) = recording_sink();
                registry
                    .register(
                        "turn-1",
                        "sess-1",
                        "user-1",
                        "text-1",
                        Some(BINDING.to_string()),
                        sink,
                    )
                    .is_some()
            })
        })
        .collect();

    let successes = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .filter(|succeeded| *succeeded)
        .count();

    assert_eq!(successes, 1);
}

#[test]
fn a_summary_turn_is_found_by_the_message_it_replies_to_and_not_by_another_sessions_turn() {
    let registry = TurnRegistry::new();
    let (sink, _) = recording_sink();
    let _ = registry
        .register(
            "turn-notes",
            "session-a",
            "notes-root-1",
            "turn-notes",
            Some(BINDING.to_string()),
            sink,
        )
        .expect("the summary turn registers");

    assert!(registry.has_reply_to("session-a", "notes-root-1"));
    assert!(!registry.has_reply_to("session-a", "some-other-message"));
    assert!(!registry.has_reply_to("session-b", "notes-root-1"));
}

#[test]
fn resuming_a_summary_replays_what_it_has_written_so_far_to_the_new_listener() {
    let registry = TurnRegistry::new();
    let (sink, _) = recording_sink();
    let _ = registry
        .register(
            "turn-notes",
            "session-a",
            "notes-root-1",
            "turn-notes",
            Some(BINDING.to_string()),
            sink,
        )
        .expect("the summary turn registers");
    registry.push_message_part("turn-notes", Some(BINDING), "half a summary");

    let (resumed_sink, resumed) = recording_sink();
    let turn_id =
        registry.resume_reply_to("session-a", "notes-root-1", Some(BINDING), resumed_sink);

    assert_eq!(turn_id.as_deref(), Some("turn-notes"));
    assert_eq!(deltas(&resumed.lock().unwrap()), vec!["half a summary"]);
}

#[test]
fn a_summary_turn_that_timed_out_is_abandoned_so_a_later_recovery_can_replace_it() {
    let registry = TurnRegistry::new();
    let (sink, _) = recording_sink();
    let _ = registry
        .register(
            "turn-notes",
            "session-a",
            "notes-root-1",
            "turn-notes",
            Some(BINDING.to_string()),
            sink,
        )
        .expect("the summary turn registers");

    assert!(registry.abandon("turn-notes"));
    assert!(!registry.has_reply_to("session-a", "notes-root-1"));
    assert!(registry.is_retired("turn-notes"));
    assert!(!registry.abandon("turn-notes"));
}
