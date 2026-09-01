use super::*;
use std::sync::Mutex as StdMutex;

use sw_notes::{TurnPersistence, UiChunk};

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

fn expected_reply_id(turn_id: &str) -> String {
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, turn_id.as_bytes()).to_string()
}

fn temp_store(tag: &str) -> Arc<sw_notes::SessionStore> {
    let dir = std::env::temp_dir().join(format!(
        "sw_free_relay_producer_{tag}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    Arc::new(sw_notes::SessionStore::new(dir, [0x24; 32]).expect("temp session store"))
}

fn sample_record(session_id: &str) -> sw_notes::SessionRecord {
    sw_notes::SessionRecord {
        session_id: session_id.to_string(),
        relay_session_id: uuid::Uuid::new_v4().to_string(),
        started_at: "2026-08-17T10:00:00Z".to_string(),
        ended_at: "2026-08-17T10:30:00Z".to_string(),
        title: None,
        title_is_auto: false,
        segments: vec![],
        insights: vec![],
        blocks: vec![],
        notes_markdown: None,
        notes_status: None,
        notes_error: None,
        notes_root_message_id: None,
        notes_pending_root_message_id: None,
        chat: vec![],
        attendees: vec![],
        calendar_event_id: None,
        playbook: None,
    }
}

#[test]
fn relay_send_failure_fails_the_turn_and_deregisters_it() {
    let registry = TurnRegistry::new();
    let store = temp_store("send_failure");
    store.save(&sample_record("sess-1")).unwrap();
    let (sink, received) = recording_sink();
    let handle = registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "turn-1",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            sink,
        )
        .expect("fresh turn id registers");
    received.lock().unwrap().clear();

    let outcome = apply_send_outcome(
        &registry,
        &store,
        &handle,
        "turn-1",
        Err("relay unreachable".to_string()),
    );

    assert_eq!(outcome, Err("relay unreachable".to_string()));
    assert!(!registry.contains("turn-1"));
    let events = received.lock().unwrap();
    assert_eq!(chunk_kinds(&events), vec!["error", "finish"]);
    match &events[0] {
        UiChunk::Error { error_text } => assert_eq!(error_text, "relay unreachable"),
        other => panic!("expected error chunk, got {other:?}"),
    }
}

#[test]
fn relay_send_success_pushes_a_waiting_activity_and_keeps_the_turn_registered() {
    let registry = TurnRegistry::new();
    let store = temp_store("send_success");
    store.save(&sample_record("sess-1")).unwrap();
    let (sink, received) = recording_sink();
    let handle = registry
        .register(
            "turn-1",
            "sess-1",
            "user-msg-1",
            "turn-1",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            sink,
        )
        .expect("fresh turn id registers");
    received.lock().unwrap().clear();

    let outcome = apply_send_outcome(&registry, &store, &handle, "turn-1", Ok(()));

    assert_eq!(outcome, Ok(()));
    assert!(registry.contains("turn-1"));
    assert_eq!(chunk_kinds(&received.lock().unwrap()), vec!["activity"]);
}

#[test]
fn parsing_a_relay_target_rejects_a_non_uuid_turn_id() {
    let record = sample_record("sess-parse");
    let result = parse_relay_target(&record, "not-a-uuid");
    assert_eq!(result, Err("turn id must be a uuid".to_string()));
}

#[test]
fn parsing_a_relay_target_rejects_a_session_with_an_invalid_relay_id() {
    let mut record = sample_record("sess-parse");
    record.relay_session_id = "not-a-uuid".to_string();
    let turn_id = uuid::Uuid::new_v4().to_string();
    let result = parse_relay_target(&record, &turn_id);
    assert_eq!(result, Err("session has invalid relay id".to_string()));
}

#[tokio::test]
async fn a_relay_turn_whose_callback_never_arrives_is_failed_by_the_watchdog_deregistered_and_persists_an_errored_chat_msg(
) {
    let registry = Arc::new(TurnRegistry::new());
    let (sink, received) = recording_sink();
    registry
        .register(
            "turn-timeout",
            "sess-timeout",
            "user-msg-1",
            "turn-timeout",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            sink,
        )
        .expect("fresh turn id registers");
    received.lock().unwrap().clear();

    let store = temp_store("watchdog_timeout");
    store.save(&sample_record("sess-timeout")).unwrap();

    let pending = Arc::new(PendingReplies::default());
    pending.register("turn-timeout".to_string(), "sess-timeout".to_string());

    run_relay_reply_watchdog(
        registry.clone(),
        store.clone(),
        Some(pending.clone()),
        "turn-timeout".to_string(),
        Duration::from_millis(15),
    )
    .await;

    assert!(!registry.contains("turn-timeout"));
    assert_eq!(
        chunk_kinds(&received.lock().unwrap()),
        vec!["error", "finish"]
    );

    let record = store.load("sess-timeout").unwrap().unwrap();
    let persisted = record
        .chat
        .iter()
        .find(|m| m.id == expected_reply_id("turn-timeout"))
        .expect("watchdog persists an errored assistant message");
    assert_eq!(persisted.status, "errored");
    assert_eq!(
        persisted.error_message.as_deref(),
        Some(RELAY_REPLY_TIMEOUT_MESSAGE)
    );

    assert_eq!(
        pending.reserve("turn-timeout", "sess-timeout"),
        crate::reply_router::ReserveResult::Duplicate
    );
}

#[tokio::test]
async fn activity_touches_within_the_timeout_window_extend_the_watchdog() {
    let registry = Arc::new(TurnRegistry::new());
    let (sink, received) = recording_sink();
    registry
        .register(
            "turn-active",
            "sess-active",
            "user-msg-1",
            "turn-active",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            sink,
        )
        .expect("fresh turn id registers");
    received.lock().unwrap().clear();

    let store = temp_store("watchdog_activity");
    store.save(&sample_record("sess-active")).unwrap();

    let pending = Arc::new(PendingReplies::default());
    pending.register("turn-active".to_string(), "sess-active".to_string());

    let timeout = Duration::from_millis(80);
    let watchdog = tokio::spawn(run_relay_reply_watchdog(
        registry.clone(),
        store.clone(),
        Some(pending.clone()),
        "turn-active".to_string(),
        timeout,
    ));

    tokio::time::sleep(Duration::from_millis(25)).await;
    pending.touch("turn-active");
    tokio::time::sleep(Duration::from_millis(25)).await;
    pending.touch("turn-active");

    assert!(registry.contains("turn-active"));

    watchdog.await.unwrap();
    assert!(!registry.contains("turn-active"));
}

#[tokio::test]
async fn a_touch_landing_just_before_the_naive_deadline_stops_the_watchdog_from_expiring_the_turn()
{
    let registry = Arc::new(TurnRegistry::new());
    let (sink, received) = recording_sink();
    registry
        .register(
            "turn-near-deadline",
            "sess-near-deadline",
            "user-msg-1",
            "turn-near-deadline",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            sink,
        )
        .expect("fresh turn id registers");
    received.lock().unwrap().clear();

    let store = temp_store("watchdog_near_deadline");
    store.save(&sample_record("sess-near-deadline")).unwrap();

    let pending = Arc::new(PendingReplies::default());
    pending.register(
        "turn-near-deadline".to_string(),
        "sess-near-deadline".to_string(),
    );

    let timeout = Duration::from_millis(60);
    let watchdog = tokio::spawn(run_relay_reply_watchdog(
        registry.clone(),
        store.clone(),
        Some(pending.clone()),
        "turn-near-deadline".to_string(),
        timeout,
    ));

    tokio::time::sleep(Duration::from_millis(55)).await;
    pending.touch("turn-near-deadline");

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        registry.contains("turn-near-deadline"),
        "a touch just before the naive deadline must extend the watchdog instead of a false timeout"
    );

    watchdog.await.unwrap();
    assert!(!registry.contains("turn-near-deadline"));
}

#[test]
fn a_touch_that_lands_before_the_deadline_check_keeps_the_turn_fresh_instead_of_expiring() {
    let pending = PendingReplies::default();
    pending.register("turn-1".to_string(), "sess-1".to_string());
    std::thread::sleep(Duration::from_millis(20));
    pending.touch("turn-1");

    let check = pending.check_or_claim_timeout("turn-1", Duration::from_millis(10));

    assert!(matches!(check, TimeoutCheck::StillFresh { .. }));
}

#[tokio::test]
async fn the_watchdog_leaves_an_already_finished_turn_untouched() {
    let registry = Arc::new(TurnRegistry::new());
    let store = temp_store("watchdog_already_done");
    store.save(&sample_record("sess-done")).unwrap();

    let (sink, received) = recording_sink();
    registry
        .register(
            "turn-done",
            "sess-done",
            "user-msg-1",
            "turn-done",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            sink,
        )
        .expect("fresh turn id registers");
    struct NoopPersistence;
    impl TurnPersistence for NoopPersistence {
        fn persist_reply(
            &self,
            _session_id: &str,
            _message: sw_notes::ChatMsg,
        ) -> Result<sw_notes::ChatAppendOutcome, String> {
            Ok(sw_notes::ChatAppendOutcome::Inserted)
        }
    }
    registry.terminate(
        "turn-done",
        TurnOutcome::Reply("already answered".to_string()),
        Some(LOCAL_IDENTITY_BINDING),
        &NoopPersistence,
    );
    received.lock().unwrap().clear();

    let pending = Arc::new(PendingReplies::default());
    pending.register("turn-done".to_string(), "sess-done".to_string());

    run_relay_reply_watchdog(
        registry.clone(),
        store.clone(),
        Some(pending),
        "turn-done".to_string(),
        Duration::from_millis(10),
    )
    .await;

    let record = store.load("sess-done").unwrap().unwrap();
    assert!(!record
        .chat
        .iter()
        .any(|m| m.id == expected_reply_id("turn-done")));
    assert!(received.lock().unwrap().is_empty());
}

#[test]
fn a_relay_callback_arriving_after_the_watchdog_already_terminated_persists_no_second_chat_msg_and_emits_no_second_terminal_sequence(
) {
    let registry = TurnRegistry::new();
    let store = temp_store("late_callback_after_watchdog_relay_producer");
    store.save(&sample_record("sess-late")).unwrap();

    let (sink, received) = recording_sink();
    registry
        .register(
            "turn-late",
            "sess-late",
            "user-msg-1",
            "turn-late",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            sink,
        )
        .expect("fresh turn id registers");
    received.lock().unwrap().clear();

    let watchdog_handled = crate::state::session_chat::apply_relay_turn_outcome(
        &registry,
        &store,
        "turn-late",
        "errored",
        None,
        Some("reply_timeout"),
        Some(RELAY_REPLY_TIMEOUT_MESSAGE),
    );
    assert!(watchdog_handled);
    let record_after_watchdog = store.load("sess-late").unwrap().unwrap();
    assert_eq!(record_after_watchdog.chat.len(), 1);
    received.lock().unwrap().clear();

    let late_callback_handled = crate::state::session_chat::apply_relay_turn_outcome(
        &registry,
        &store,
        "turn-late",
        "completed",
        Some("the real answer arrived late"),
        None,
        None,
    );
    assert!(late_callback_handled);

    let record_after_late_callback = store.load("sess-late").unwrap().unwrap();
    assert_eq!(
        record_after_late_callback.chat.len(),
        1,
        "no second chat message should be persisted"
    );
    assert!(
        received.lock().unwrap().is_empty(),
        "no second terminal sequence should be emitted"
    );
}
