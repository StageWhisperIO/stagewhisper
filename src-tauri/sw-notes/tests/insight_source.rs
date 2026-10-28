use serde_json::json;
use sw_notes::{InsightNote, InsightSource, SessionRecord, SessionStore};

fn temp_store(tag: &str) -> SessionStore {
    let dir = std::env::temp_dir().join(format!("sw_notes_source_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    SessionStore::new(dir, [0x42; 32]).unwrap()
}

fn renewal_plan() -> InsightSource {
    InsightSource {
        title: "Acme renewal plan".to_string(),
        source_id: "clickup".to_string(),
        url: Some("https://app.clickup.com/t/86acme1".to_string()),
    }
}

#[test]
fn a_live_insight_keeps_the_source_it_was_drawn_from() {
    let store = temp_store("live");
    store.live_begin("sess-src", 1_000).unwrap();

    store
        .live_append_insight(
            "sess-src",
            "green",
            "Mention the renewal date",
            None,
            Some(renewal_plan()),
            9_000,
        )
        .unwrap();
    store
        .live_append_insight("sess-src", "orange", "Slow down", None, None, 10_000)
        .unwrap();

    let live = store.live_load("sess-src").unwrap().unwrap();
    assert_eq!(live.insights[0].source, Some(renewal_plan()));
    assert_eq!(live.insights[0].offset_ms, 8_000);
    assert_eq!(live.insights[1].source, None);
}

#[test]
fn an_insight_written_by_an_older_build_reads_without_a_source() {
    let note: InsightNote = serde_json::from_value(json!({
        "offset_ms": 4_000,
        "severity": "green",
        "message": "Ask about the timeline",
        "segment_index": 2
    }))
    .unwrap();

    assert_eq!(note.source, None);
    assert_eq!(note.segment_index, Some(2));
}

#[test]
fn an_insight_without_a_source_is_written_exactly_as_older_builds_wrote_it() {
    let note = InsightNote {
        offset_ms: 4_000,
        severity: "green".to_string(),
        message: "Ask about the timeline".to_string(),
        segment_index: None,
        source: None,
    };

    assert_eq!(
        serde_json::to_value(&note).unwrap(),
        json!({
            "offset_ms": 4_000,
            "severity": "green",
            "message": "Ask about the timeline",
            "segment_index": null
        })
    );
}

#[test]
fn a_source_without_a_link_is_written_without_one() {
    let source = InsightSource {
        url: None,
        ..renewal_plan()
    };

    assert_eq!(
        serde_json::to_value(&source).unwrap(),
        json!({"title": "Acme renewal plan", "source_id": "clickup"})
    );
}

#[test]
fn a_finished_session_keeps_the_source_of_each_insight_for_the_library() {
    let store = temp_store("finished");
    let mut record: SessionRecord = serde_json::from_value(json!({
        "session_id": "sess-done",
        "relay_session_id": "sess-done",
        "started_at": "2026-10-17T10:00:00Z",
        "ended_at": "2026-10-17T10:30:00Z",
        "segments": [],
        "insights": []
    }))
    .unwrap();
    record.insights.push(InsightNote {
        offset_ms: 8_000,
        severity: "green".to_string(),
        message: "Mention the renewal date".to_string(),
        segment_index: None,
        source: Some(renewal_plan()),
    });

    store.save(&record).unwrap();

    let reloaded = store.load("sess-done").unwrap().unwrap();
    assert_eq!(reloaded.insights[0].source, Some(renewal_plan()));
}
