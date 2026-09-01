use super::*;
use crate::accumulate::{TranscriptSegment, TranscriptSource};
use uuid::Uuid;

fn temp_store(tag: &str) -> SessionStore {
    let dir = std::env::temp_dir().join(format!(
        "sw_notes_append_chat_{tag}_{}_{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    SessionStore::new(dir, [0x5a; 32]).unwrap()
}

fn sample_record(session_id: &str) -> SessionRecord {
    SessionRecord {
        session_id: session_id.to_string(),
        relay_session_id: session_id.to_string(),
        started_at: "2026-05-25T10:00:00Z".to_string(),
        ended_at: "2026-05-25T10:30:00Z".to_string(),
        title: None,
        title_is_auto: false,
        segments: vec![TranscriptSegment {
            source: TranscriptSource::You,
            utterance: "hello world".to_string(),
            speaker_id: None,
            speaker_label: None,
        }],
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

fn chat_msg(id: &str, content: &str) -> ChatMsg {
    ChatMsg {
        id: id.to_string(),
        role: "assistant".to_string(),
        content: content.to_string(),
        status: "completed".to_string(),
        parent_message_id: Some("user-1".to_string()),
        error_code: None,
        error_message: None,
        created_at: "2026-05-25T10:31:00Z".to_string(),
    }
}

#[test]
fn appending_a_message_with_an_identical_payload_to_an_existing_id_reports_identical_duplicate_and_does_not_corrupt_the_record(
) {
    let store = temp_store("identical");
    store.save(&sample_record("sess-1")).unwrap();
    let first = chat_msg("msg-1", "hello there");
    assert_eq!(
        store.append_chat("sess-1", first.clone()).unwrap(),
        ChatAppendOutcome::Inserted
    );

    let mut replayed = first.clone();
    replayed.created_at = "2026-05-25T10:45:00Z".to_string();
    let outcome = store.append_chat("sess-1", replayed).unwrap();

    assert_eq!(outcome, ChatAppendOutcome::IdenticalDuplicate);
    let saved = store.load("sess-1").unwrap().unwrap();
    assert_eq!(saved.chat.len(), 1);
    assert_eq!(saved.chat[0].content, "hello there");
    assert_eq!(saved.chat[0].created_at, "2026-05-25T10:31:00Z");
}

#[test]
fn appending_a_message_with_a_different_payload_to_an_existing_id_reports_conflicting_duplicate_and_does_not_overwrite_the_stored_message(
) {
    let store = temp_store("conflicting");
    store.save(&sample_record("sess-2")).unwrap();
    let first = chat_msg("msg-1", "the original reply");
    assert_eq!(
        store.append_chat("sess-2", first).unwrap(),
        ChatAppendOutcome::Inserted
    );

    let conflicting = chat_msg("msg-1", "a completely different reply");
    let outcome = store.append_chat("sess-2", conflicting).unwrap();

    assert_eq!(outcome, ChatAppendOutcome::ConflictingDuplicate);
    let saved = store.load("sess-2").unwrap().unwrap();
    assert_eq!(saved.chat.len(), 1);
    assert_eq!(saved.chat[0].content, "the original reply");
}

#[test]
fn appending_to_a_session_that_does_not_exist_reports_missing_session() {
    let store = temp_store("missing");
    let outcome = store
        .append_chat("no-such-session", chat_msg("msg-1", "hi"))
        .unwrap();
    assert_eq!(outcome, ChatAppendOutcome::MissingSession);
}

#[test]
fn appending_a_genuinely_new_message_reports_inserted_and_is_readable_back() {
    let store = temp_store("inserted");
    store.save(&sample_record("sess-3")).unwrap();
    let outcome = store
        .append_chat("sess-3", chat_msg("msg-1", "brand new reply"))
        .unwrap();

    assert_eq!(outcome, ChatAppendOutcome::Inserted);
    let saved = store.load("sess-3").unwrap().unwrap();
    assert_eq!(saved.chat.len(), 1);
    assert_eq!(saved.chat[0].id, "msg-1");
    assert_eq!(saved.chat[0].content, "brand new reply");
}
