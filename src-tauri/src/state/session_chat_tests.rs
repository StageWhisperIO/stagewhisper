use super::*;
use std::sync::Mutex as StdMutex;

#[derive(Debug, Clone, PartialEq)]
enum RecordedChunk {
    Start,
    TextStart,
    TextDelta(String),
    TextEnd,
    Error(String),
    Finish(String),
}

fn capturing_channel() -> (Channel<UiChunk>, Arc<StdMutex<Vec<RecordedChunk>>>) {
    let captured: Arc<StdMutex<Vec<RecordedChunk>>> = Arc::new(StdMutex::new(Vec::new()));
    let recorder = captured.clone();
    let channel = Channel::new(move |body| {
        let tauri::ipc::InvokeResponseBody::Json(raw) = body else {
            return Ok(());
        };
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let recorded = match value["type"].as_str().unwrap_or_default() {
            "start" => RecordedChunk::Start,
            "text-start" => RecordedChunk::TextStart,
            "text-delta" => {
                RecordedChunk::TextDelta(value["delta"].as_str().unwrap_or_default().to_string())
            }
            "text-end" => RecordedChunk::TextEnd,
            "error" => {
                RecordedChunk::Error(value["errorText"].as_str().unwrap_or_default().to_string())
            }
            "finish" => RecordedChunk::Finish(
                value["finishReason"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            ),
            other => panic!("unexpected chunk type in test: {other}"),
        };
        recorder.lock().unwrap().push(recorded);
        Ok(())
    });
    (channel, captured)
}

fn deltas(events: &[RecordedChunk]) -> Vec<String> {
    events
        .iter()
        .filter_map(|chunk| match chunk {
            RecordedChunk::TextDelta(delta) => Some(delta.clone()),
            _ => None,
        })
        .collect()
}

fn temp_store(tag: &str) -> sw_notes::SessionStore {
    let dir = std::env::temp_dir().join(format!(
        "sw_free_session_chat_mod_{tag}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    sw_notes::SessionStore::new(dir, [0x11; 32]).expect("temp session store")
}

fn sample_record(session_id: &str) -> sw_notes::SessionRecord {
    sw_notes::SessionRecord {
        session_id: session_id.to_string(),
        relay_session_id: uuid::Uuid::new_v4().to_string(),
        backend_session_id: None,
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
fn two_concurrent_turns_on_the_same_session_each_receive_only_their_own_deltas_and_terminal_chunk()
{
    let registry = TurnRegistry::new();
    let store = temp_store("two_concurrent_turns");
    store.save(&sample_record("sess-shared")).unwrap();
    let persistence = StorePersistence { store: &store };

    let (channel_a, captured_a) = capturing_channel();
    registry
        .register(
            "turn-a",
            "sess-shared",
            "user-a",
            "turn-a",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            channel_sink(channel_a),
        )
        .expect("fresh turn id registers");
    let (channel_b, captured_b) = capturing_channel();
    registry
        .register(
            "turn-b",
            "sess-shared",
            "user-b",
            "turn-b",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            channel_sink(channel_b),
        )
        .expect("fresh turn id registers");
    captured_a.lock().unwrap().clear();
    captured_b.lock().unwrap().clear();

    assert!(registry.push_text_delta("turn-a", Some(LOCAL_IDENTITY_BINDING), "hello a"));
    assert!(registry.push_text_delta("turn-b", Some(LOCAL_IDENTITY_BINDING), "hello b"));
    registry.terminate(
        "turn-a",
        TurnOutcome::Reply("hello a".to_string()),
        Some(LOCAL_IDENTITY_BINDING),
        &persistence,
    );
    registry.terminate(
        "turn-b",
        TurnOutcome::Reply("hello b".to_string()),
        Some(LOCAL_IDENTITY_BINDING),
        &persistence,
    );

    let events_a = captured_a.lock().unwrap();
    assert_eq!(deltas(&events_a), vec!["hello a".to_string()]);
    assert_eq!(
        events_a.last(),
        Some(&RecordedChunk::Finish("stop".to_string()))
    );

    let events_b = captured_b.lock().unwrap();
    assert_eq!(deltas(&events_b), vec!["hello b".to_string()]);
    assert_eq!(
        events_b.last(),
        Some(&RecordedChunk::Finish("stop".to_string()))
    );
}

#[test]
fn a_turn_whose_host_callback_never_arrives_can_still_be_cancelled_and_leaves_no_registry_entry() {
    let registry = TurnRegistry::new();
    let (channel, _captured) = capturing_channel();
    registry
        .register(
            "turn-1",
            "sess-1",
            "user-1",
            "turn-1",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            channel_sink(channel),
        )
        .expect("fresh turn id registers");
    assert!(registry.contains("turn-1"));

    assert!(registry.cancel("turn-1"));

    assert!(!registry.contains("turn-1"));
    assert!(!registry.cancel("turn-1"));
}

#[test]
fn resume_after_some_deltas_replays_exactly_once_and_continues_live() {
    let registry = TurnRegistry::new();
    let (initial_channel, _initial_captured) = capturing_channel();
    registry
        .register(
            "turn-1",
            "sess-1",
            "user-1",
            "turn-1",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            channel_sink(initial_channel),
        )
        .expect("fresh turn id registers");
    registry.push_text_delta("turn-1", Some(LOCAL_IDENTITY_BINDING), "hel");
    registry.push_text_delta("turn-1", Some(LOCAL_IDENTITY_BINDING), "lo");

    let (resume_channel, resume_captured) = capturing_channel();
    let resumed_turn_id = registry
        .resume(
            "sess-1",
            Some(LOCAL_IDENTITY_BINDING),
            channel_sink(resume_channel),
        )
        .expect("resume should find the in-flight turn");
    assert_eq!(resumed_turn_id, "turn-1");
    assert_eq!(
        deltas(&resume_captured.lock().unwrap()),
        vec!["hello".to_string()]
    );

    registry.push_text_delta("turn-1", Some(LOCAL_IDENTITY_BINDING), "!");
    assert_eq!(
        deltas(&resume_captured.lock().unwrap()),
        vec!["hello".to_string(), "!".to_string()]
    );
}

#[test]
fn resuming_a_registered_turn_with_the_apps_stable_binding_succeeds() {
    let registry = TurnRegistry::new();
    let (sink, _captured) = capturing_channel();
    registry
        .register(
            "turn-resume",
            "sess-resume",
            "user-1",
            "turn-resume",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            channel_sink(sink),
        )
        .expect("fresh turn id registers");

    let (resume_channel, _resume_captured) = capturing_channel();
    let resumed = registry.resume(
        "sess-resume",
        Some(LOCAL_IDENTITY_BINDING),
        channel_sink(resume_channel),
    );

    assert_eq!(resumed, Some("turn-resume".to_string()));
}

#[test]
fn a_relay_callback_arriving_after_the_watchdog_already_terminated_persists_no_second_chat_msg_and_emits_no_second_terminal_sequence(
) {
    let registry = TurnRegistry::new();
    let store = temp_store("late_callback_after_watchdog");
    store.save(&sample_record("sess-late")).unwrap();

    let (channel, captured) = capturing_channel();
    registry
        .register(
            "turn-late",
            "sess-late",
            "user-1",
            "turn-late",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            channel_sink(channel),
        )
        .expect("fresh turn id registers");
    captured.lock().unwrap().clear();

    let watchdog_handled = apply_relay_turn_outcome(
        &registry,
        &store,
        "turn-late",
        "errored",
        None,
        Some("reply_timeout"),
        Some("No reply received before timeout"),
    );
    assert!(watchdog_handled);
    let record_after_watchdog = store.load("sess-late").unwrap().unwrap();
    assert_eq!(record_after_watchdog.chat.len(), 1);
    captured.lock().unwrap().clear();

    let late_callback_handled = apply_relay_turn_outcome(
        &registry,
        &store,
        "turn-late",
        "completed",
        Some("actually, here is the answer"),
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
        captured.lock().unwrap().is_empty(),
        "no second terminal sequence should be emitted"
    );
}

fn expected_reply_id(turn_id: &str) -> String {
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, turn_id.as_bytes()).to_string()
}

#[test]
fn a_reply_whose_persistence_reports_a_conflicting_duplicate_emits_the_error_terminal_not_finish_stop(
) {
    let registry = TurnRegistry::new();
    let store = temp_store("conflicting_duplicate_reply");
    store.save(&sample_record("sess-conflict")).unwrap();
    let persistence = StorePersistence { store: &store };

    let (channel, captured) = capturing_channel();
    registry
        .register(
            "turn-conflict",
            "sess-conflict",
            "user-1",
            "turn-conflict",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            channel_sink(channel),
        )
        .expect("fresh turn id registers");
    captured.lock().unwrap().clear();

    store
        .append_chat(
            "sess-conflict",
            sw_notes::ChatMsg {
                id: expected_reply_id("turn-conflict"),
                role: "assistant".to_string(),
                content: "an earlier, different answer".to_string(),
                status: "completed".to_string(),
                parent_message_id: Some("user-1".to_string()),
                error_code: None,
                error_message: None,
                created_at: "2026-08-17T10:05:00Z".to_string(),
            },
        )
        .unwrap();

    let result = registry.terminate(
        "turn-conflict",
        TurnOutcome::Reply("a genuinely different answer".to_string()),
        Some(LOCAL_IDENTITY_BINDING),
        &persistence,
    );

    assert!(matches!(
        result,
        TerminalResult::Terminated(sw_notes::TerminationOutcome::Failed(_))
    ));
    let events = captured.lock().unwrap();
    assert_eq!(
        events.last(),
        Some(&RecordedChunk::Finish("error".to_string()))
    );
    assert!(events
        .iter()
        .any(|chunk| matches!(chunk, RecordedChunk::Error(_))));

    let record = store.load("sess-conflict").unwrap().unwrap();
    assert_eq!(
        record.chat.len(),
        1,
        "the conflicting reply must not be appended alongside the original"
    );
}

#[test]
fn a_reply_whose_persistence_reports_a_missing_session_emits_the_error_terminal() {
    let registry = TurnRegistry::new();
    let store = temp_store("missing_session_reply");
    let persistence = StorePersistence { store: &store };

    let (channel, captured) = capturing_channel();
    registry
        .register(
            "turn-missing",
            "sess-missing",
            "user-1",
            "turn-missing",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            channel_sink(channel),
        )
        .expect("fresh turn id registers");
    captured.lock().unwrap().clear();

    let result = registry.terminate(
        "turn-missing",
        TurnOutcome::Reply("an answer nobody can store".to_string()),
        Some(LOCAL_IDENTITY_BINDING),
        &persistence,
    );

    assert!(matches!(
        result,
        TerminalResult::Terminated(sw_notes::TerminationOutcome::Failed(_))
    ));
    let events = captured.lock().unwrap();
    assert_eq!(
        events.last(),
        Some(&RecordedChunk::Finish("error".to_string()))
    );
    assert!(events
        .iter()
        .any(|chunk| matches!(chunk, RecordedChunk::Error(_))));
}
