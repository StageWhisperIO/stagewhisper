use std::sync::atomic::Ordering;

use crate::accumulate::{TranscriptSegment, TranscriptSource};
use crate::store::{SessionRecord, SessionStore, SessionSummary};
use crate::summary_index::INDEX_FILE;

fn record(session_id: &str, ended_at: &str) -> SessionRecord {
    SessionRecord {
        session_id: session_id.to_string(),
        relay_session_id: session_id.to_string(),
        backend_session_id: None,
        started_at: "2026-05-25T10:00:00Z".to_string(),
        ended_at: ended_at.to_string(),
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
        notes_status: Some("pending".to_string()),
        notes_error: None,
        notes_root_message_id: None,
        notes_pending_root_message_id: None,
        chat: vec![],
        attendees: vec![],
        calendar_event_id: None,
        playbook: None,
        outcome: None,
    }
}

fn store(tag: &str) -> SessionStore {
    let dir = std::env::temp_dir().join(format!("sw_notes_index_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    SessionStore::new(dir, [0x42; 32]).unwrap()
}

fn decrypted(store: &SessionStore) -> usize {
    store.decrypted_session_files.load(Ordering::SeqCst)
}

fn full_scan(store: &SessionStore) -> Vec<SessionSummary> {
    let mut summaries: Vec<SessionSummary> = store
        .session_file_stats()
        .into_iter()
        .filter_map(|stat| store.load(&stat.session_id).ok().flatten())
        .map(|record| SessionSummary {
            session_id: record.session_id,
            title: record.title,
            ended_at: record.ended_at,
            has_notes: record.notes_markdown.is_some(),
            notes_status: record.notes_status,
            outcome: record.outcome,
        })
        .collect();
    summaries.sort_by(|a, b| b.ended_at.cmp(&a.ended_at));
    summaries
}

fn ids(summaries: &[SessionSummary]) -> Vec<String> {
    summaries.iter().map(|s| s.session_id.clone()).collect()
}

#[test]
fn a_second_list_decrypts_no_unchanged_session_file() {
    let store = store("second");
    store.save(&record("a", "2026-05-20T10:00:00Z")).unwrap();
    store.save(&record("b", "2026-05-21T10:00:00Z")).unwrap();
    store.list().unwrap();
    assert_eq!(decrypted(&store), 2);

    store.list().unwrap();

    assert_eq!(decrypted(&store), 2);
    let _ = std::fs::remove_dir_all(store.dir());
}

#[test]
fn a_session_rewritten_by_an_older_build_is_picked_up() {
    let store = store("older");
    store.save(&record("a", "2026-05-20T10:00:00Z")).unwrap();
    store.list().unwrap();
    let mut changed = record("a", "2026-05-20T10:00:00Z");
    changed.title = Some("Renamed by an older build with a longer title".to_string());
    let index_bytes = std::fs::read(store.dir().join(INDEX_FILE)).unwrap();
    store.save(&changed).unwrap();
    std::fs::write(store.dir().join(INDEX_FILE), index_bytes).unwrap();

    let list = store.list().unwrap();

    assert_eq!(
        list[0].title.as_deref(),
        Some("Renamed by an older build with a longer title")
    );
    let _ = std::fs::remove_dir_all(store.dir());
}

#[test]
fn a_new_session_file_appears_and_only_it_is_decrypted() {
    let store = store("new");
    store.save(&record("a", "2026-05-20T10:00:00Z")).unwrap();
    store.list().unwrap();
    store.save(&record("b", "2026-05-21T10:00:00Z")).unwrap();

    let list = store.list().unwrap();

    assert_eq!(ids(&list), vec!["b", "a"]);
    assert_eq!(decrypted(&store), 2);
    let _ = std::fs::remove_dir_all(store.dir());
}

#[test]
fn a_deleted_session_file_disappears_from_the_list_and_the_index() {
    let store = store("deleted");
    store.save(&record("a", "2026-05-20T10:00:00Z")).unwrap();
    store.save(&record("b", "2026-05-21T10:00:00Z")).unwrap();
    store.list().unwrap();
    store.delete("a").unwrap();

    assert_eq!(ids(&store.list().unwrap()), vec!["b"]);
    let before = decrypted(&store);
    assert_eq!(ids(&store.list().unwrap()), vec!["b"]);
    assert_eq!(decrypted(&store), before);
    let _ = std::fs::remove_dir_all(store.dir());
}

#[test]
fn a_corrupt_index_is_rebuilt_without_error() {
    let store = store("corrupt");
    store.save(&record("a", "2026-05-20T10:00:00Z")).unwrap();
    store.list().unwrap();
    std::fs::write(store.dir().join(INDEX_FILE), b"not an index").unwrap();

    let list = store.list().unwrap();

    assert_eq!(ids(&list), vec!["a"]);
    assert_eq!(decrypted(&store), 2);
    store.list().unwrap();
    assert_eq!(decrypted(&store), 2);
    let _ = std::fs::remove_dir_all(store.dir());
}

#[test]
fn an_index_with_an_unknown_version_is_rebuilt() {
    let store = store("version");
    store.save(&record("a", "2026-05-20T10:00:00Z")).unwrap();
    store.list().unwrap();
    let future = serde_json::to_vec(&serde_json::json!({"version": 99, "entries": []})).unwrap();
    let encrypted = sw_crypto::encrypt_bytes(&[0x42; 32], &future).unwrap();
    std::fs::write(store.dir().join(INDEX_FILE), encrypted).unwrap();

    let list = store.list().unwrap();

    assert_eq!(ids(&list), vec!["a"]);
    let _ = std::fs::remove_dir_all(store.dir());
}

#[test]
fn an_index_encrypted_with_another_key_is_rebuilt() {
    let store = store("wrongkey");
    store.save(&record("a", "2026-05-20T10:00:00Z")).unwrap();
    let foreign = serde_json::to_vec(&serde_json::json!({"version": 1, "entries": []})).unwrap();
    let encrypted = sw_crypto::encrypt_bytes(&[0x01; 32], &foreign).unwrap();
    std::fs::write(store.dir().join(INDEX_FILE), encrypted).unwrap();

    assert_eq!(ids(&store.list().unwrap()), vec!["a"]);
    let _ = std::fs::remove_dir_all(store.dir());
}

#[test]
fn the_index_file_is_not_mistaken_for_a_session_file() {
    let store = store("ext");
    store.save(&record("a", "2026-05-20T10:00:00Z")).unwrap();
    store.list().unwrap();

    assert!(store.dir().join(INDEX_FILE).exists());
    assert_eq!(store.session_file_stats().len(), 1);
    let _ = std::fs::remove_dir_all(store.dir());
}

#[test]
fn the_indexed_list_equals_the_full_scan_output() {
    let store = store("equal");
    let mut notes = record("c", "2026-05-22T10:00:00Z");
    notes.notes_markdown = Some("# Notes".to_string());
    notes.notes_status = Some("completed".to_string());
    notes.title = Some("Titled".to_string());
    store.save(&record("a", "2026-05-20T10:00:00Z")).unwrap();
    store.save(&record("b", "2026-05-21T10:00:00Z")).unwrap();
    store.save(&notes).unwrap();
    std::fs::write(store.dir().join("broken.swsession"), b"garbage").unwrap();

    let first = store.list().unwrap();
    let second = store.list().unwrap();

    let expected = serde_json::to_value(full_scan(&store)).unwrap();
    assert_eq!(serde_json::to_value(&first).unwrap(), expected);
    assert_eq!(serde_json::to_value(&second).unwrap(), expected);
    let _ = std::fs::remove_dir_all(store.dir());
}
