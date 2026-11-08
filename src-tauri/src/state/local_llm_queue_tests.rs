use std::sync::{Arc, Mutex};
use std::time::Duration;

use sw_notes::{
    ChatAppendOutcome, ChatMsg, TerminalResult, TerminationOutcome, TurnOutcome, TurnPersistence,
    TurnRegistry, UiChunk,
};

use super::{LocalTurnQueue, QueuedTurn};

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

#[tokio::test]
async fn a_panicking_turn_does_not_stop_the_queue_from_running_the_next_turn() {
    let queue = LocalTurnQueue::new();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    queue
        .enqueue(|_reason: String| {}, async {
            panic!("a turn panicking should not kill the queue worker");
        })
        .unwrap();

    queue
        .enqueue(|_reason: String| {}, async move {
            let _ = tx.send(());
        })
        .unwrap();

    let ran = tokio::time::timeout(Duration::from_secs(5), rx).await;
    assert!(
        ran.is_ok(),
        "the turn queued after a panicking turn never ran"
    );
    assert!(ran.unwrap().is_ok());
}

#[tokio::test]
async fn a_panicking_turn_is_terminated_with_an_error_and_a_finish_and_is_removed_from_the_registry(
) {
    let queue = LocalTurnQueue::new();
    let registry = Arc::new(TurnRegistry::new());
    let received = Arc::new(Mutex::new(Vec::new()));
    let recorder = received.clone();
    registry
        .register(
            "turn-1",
            "sess-1",
            "user-1",
            "text-1",
            Some("1".to_string()),
            move |chunk: UiChunk| recorder.lock().unwrap().push(chunk),
        )
        .expect("fresh turn id registers");
    received.lock().unwrap().clear();

    let registry_for_guard = registry.clone();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    let done_tx = Mutex::new(Some(done_tx));

    queue
        .enqueue(
            move |reason: String| {
                let result = registry_for_guard.terminate(
                    "turn-1",
                    TurnOutcome::Failure(reason),
                    Some("1"),
                    &NoopPersistence,
                );
                assert!(matches!(
                    result,
                    TerminalResult::Terminated(TerminationOutcome::Failed(_))
                ));
                if let Some(tx) = done_tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
            },
            async {
                panic!("a queued turn panicking must still terminate its registry entry");
            },
        )
        .unwrap();

    let signalled = tokio::time::timeout(Duration::from_secs(5), done_rx).await;
    assert!(signalled.is_ok(), "the panicking turn's guard never fired");

    let events = received.lock().unwrap();
    assert!(events
        .iter()
        .any(|chunk| matches!(chunk, UiChunk::Error { .. })));
    assert!(events.iter().any(
        |chunk| matches!(chunk, UiChunk::Finish { finish_reason } if finish_reason == "error")
    ));
    assert!(!registry.contains("turn-1"));
}

#[tokio::test]
async fn a_turn_that_completes_normally_is_not_double_terminated_by_the_supervisor() {
    let queue = LocalTurnQueue::new();
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fired_for_guard = fired.clone();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    queue
        .enqueue(
            move |_reason: String| {
                fired_for_guard.store(true, std::sync::atomic::Ordering::SeqCst);
            },
            async move {
                let _ = done_tx.send(());
            },
        )
        .unwrap();

    let completed = tokio::time::timeout(Duration::from_secs(5), done_rx).await;
    assert!(completed.is_ok(), "the queued turn never ran");

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !fired.load(std::sync::atomic::Ordering::SeqCst),
        "the supervisor fired its failure guard even though the turn completed normally"
    );
}

#[tokio::test]
async fn a_turn_that_cannot_be_enqueued_ends_with_an_error_terminal_outcome_rather_than_hanging() {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<QueuedTurn>();
    drop(rx);
    let queue = LocalTurnQueue { tx };

    let enqueue_result = queue.enqueue(|_reason: String| {}, async {});
    assert!(enqueue_result.is_err());

    let registry = TurnRegistry::new();
    let received = Arc::new(Mutex::new(Vec::new()));
    let recorder = received.clone();
    let handle = registry.register(
        "turn-1",
        "sess-1",
        "user-1",
        "text-1",
        Some("1".to_string()),
        move |chunk: UiChunk| recorder.lock().unwrap().push(chunk),
    );
    assert!(handle.is_some());
    received.lock().unwrap().clear();

    let result = registry.terminate(
        "turn-1",
        TurnOutcome::Failure("the local turn queue is unavailable".to_string()),
        Some("1"),
        &NoopPersistence,
    );

    assert!(matches!(
        result,
        TerminalResult::Terminated(TerminationOutcome::Failed(_))
    ));
    let events = received.lock().unwrap();
    assert!(events
        .iter()
        .any(|chunk| matches!(chunk, UiChunk::Error { .. })));
    assert!(events.iter().any(
        |chunk| matches!(chunk, UiChunk::Finish { finish_reason } if finish_reason == "error")
    ));
}
