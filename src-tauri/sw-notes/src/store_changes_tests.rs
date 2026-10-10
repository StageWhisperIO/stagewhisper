use std::sync::Mutex;

use super::*;
use crate::accumulate::{TranscriptSegment, TranscriptSource};
use crate::store::SessionRecord;
use uuid::Uuid;

fn temp_store(tag: &str) -> SessionStore {
    let dir = std::env::temp_dir().join(format!(
        "sw_notes_changes_{tag}_{}_{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    SessionStore::new(dir, [0x6b; 32]).unwrap()
}

fn finished(session_id: &str, started_at: &str, utterances: &[&str]) -> SessionRecord {
    SessionRecord {
        session_id: session_id.to_string(),
        relay_session_id: session_id.to_string(),
        backend_session_id: None,
        started_at: started_at.to_string(),
        ended_at: started_at.to_string(),
        title: None,
        title_is_auto: false,
        segments: utterances
            .iter()
            .map(|utterance| TranscriptSegment {
                source: TranscriptSource::Others,
                utterance: utterance.to_string(),
                speaker_id: None,
                speaker_label: None,
            })
            .collect(),
        insights: vec![],
        blocks: vec![],
        notes_markdown: Some("## Budget".to_string()),
        notes_status: Some("completed".to_string()),
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

fn recording(store: &SessionStore) -> Arc<Mutex<Vec<SessionChange>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    store.set_change_observer(Some(Arc::new(move |change| {
        sink.lock().unwrap().push(change);
    })));
    seen
}

fn saved(id: &str) -> SessionChange {
    SessionChange::Saved(id.to_string())
}

fn removed(id: &str) -> SessionChange {
    SessionChange::Removed(id.to_string())
}

#[test]
fn saving_then_deleting_a_session_reports_a_save_then_a_removal() {
    let store = temp_store("save_delete");
    let seen = recording(&store);

    store
        .save(&finished("a", "2026-05-25T10:00:00Z", &["hi"]))
        .unwrap();
    store.delete("a").unwrap();

    assert_eq!(*seen.lock().unwrap(), vec![saved("a"), removed("a")]);
    let _ = std::fs::remove_dir_all(store.dir());
}

#[test]
fn splitting_a_session_reports_two_saves_and_one_removal_of_the_original() {
    let store = temp_store("split");
    store
        .save(&finished("whole", "2026-05-25T10:00:00Z", &["one", "two", "three"]))
        .unwrap();
    let seen = recording(&store);

    let (first, second) = store.split("whole", 1).unwrap();

    assert_eq!(
        *seen.lock().unwrap(),
        vec![saved(&first), saved(&second), removed("whole")]
    );
    let _ = std::fs::remove_dir_all(store.dir());
}

#[test]
fn merging_two_sessions_reports_one_save_and_both_removals() {
    let store = temp_store("merge");
    store
        .save(&finished("early", "2026-05-25T10:00:00Z", &["one"]))
        .unwrap();
    store
        .save(&finished("late", "2026-05-25T11:00:00Z", &["two"]))
        .unwrap();
    let seen = recording(&store);

    let merged = store.merge("late", "early").unwrap();

    assert_eq!(
        *seen.lock().unwrap(),
        vec![saved(&merged), removed("late"), removed("early")]
    );
    let _ = std::fs::remove_dir_all(store.dir());
}

#[test]
fn every_rewrite_of_a_finished_session_reports_a_save() {
    let store = temp_store("rewrites");
    store
        .save(&finished("a", "2026-05-25T10:00:00Z", &["hi"]))
        .unwrap();
    let seen = recording(&store);

    store.update_title("a", Some("Renewal".to_string())).unwrap();
    store
        .replace_segments(
            "a",
            vec![TranscriptSegment {
                source: TranscriptSource::You,
                utterance: "edited".to_string(),
                speaker_id: None,
                speaker_label: None,
            }],
        )
        .unwrap();

    assert_eq!(*seen.lock().unwrap(), vec![saved("a"), saved("a")]);
    let _ = std::fs::remove_dir_all(store.dir());
}

#[test]
fn a_failed_write_reports_nothing() {
    let store = temp_store("failed_write");
    let record = finished("blocked", "2026-05-25T10:00:00Z", &["hi"]);
    std::fs::create_dir_all(store.path_for("blocked").unwrap()).unwrap();
    let seen = recording(&store);

    assert!(store.save(&record).is_err());

    assert!(seen.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(store.dir());
}

#[test]
fn live_session_writes_are_not_reported() {
    let store = temp_store("live");
    let seen = recording(&store);

    store.live_begin("live", 1_000).unwrap();
    store
        .live_replace_segments("live", 1_000, vec![], 2_000)
        .unwrap();
    store.live_delete("live").unwrap();

    assert!(seen.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(store.dir());
}

#[test]
fn clearing_the_observer_stops_reports_and_a_store_without_one_works_unchanged() {
    let store = temp_store("cleared");
    let seen = recording(&store);
    store.set_change_observer(None);

    store
        .save(&finished("a", "2026-05-25T10:00:00Z", &["hi"]))
        .unwrap();
    store.delete("a").unwrap();

    assert!(seen.lock().unwrap().is_empty());
    assert!(store.load("a").unwrap().is_none());
    let _ = std::fs::remove_dir_all(store.dir());
}

#[test]
fn file_stats_list_finished_sessions_by_name_and_stat_only() {
    let store = temp_store("stats");
    store
        .save(&finished("a", "2026-05-25T10:00:00Z", &["hi"]))
        .unwrap();
    store.live_begin("live", 1_000).unwrap();
    std::fs::write(store.dir().join("a.swsession.tmp.1.x"), b"partial").unwrap();
    std::fs::write(store.dir().join("notes.txt"), b"other").unwrap();

    let stats = store.session_file_stats();

    assert_eq!(stats.len(), 1);
    let only = &stats[0];
    assert_eq!(only.session_id, "a");
    let on_disk = std::fs::metadata(store.path_for("a").unwrap()).unwrap();
    assert_eq!(only.len, on_disk.len());
    assert!(only.modified_ms > 0);
    assert_eq!(store.session_file_stat("a"), Some(only.clone()));
    assert_eq!(store.session_file_stat("live"), None);
    assert_eq!(store.session_file_stat("../a"), None);
    let _ = std::fs::remove_dir_all(store.dir());
}
