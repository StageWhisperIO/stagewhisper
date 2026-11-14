use serde_json::json;
use sw_notes::accumulate::TranscriptSegment;
use sw_notes::finalize::{
    finalize_inner, NotesEvent, NotesHost, NotesSettings, SessionParticipants,
};
use sw_notes::insights::append_missing_insights;
use sw_notes::store::LiveSessionRecord;
use sw_notes::{
    FeedbackSurface, InsightFeedback, InsightNote, InsightProducer, InsightVerdict, NewInsight,
    SessionRecord, SessionStore, TranscriptSource,
};
use uuid::Uuid;

fn temp_store(tag: &str) -> SessionStore {
    let dir = std::env::temp_dir().join(format!(
        "sw_notes_feedback_compat_{tag}_{}_{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    SessionStore::new(dir, [0x3c; 32]).unwrap()
}

fn rated(verdict: InsightVerdict, rated_at: &str) -> InsightFeedback {
    InsightFeedback {
        verdict,
        rated_at: rated_at.to_string(),
        surface: FeedbackSurface::Live,
        publish_pending: false,
    }
}

fn identified(
    id: &str,
    offset_ms: u64,
    segment_index: usize,
    feedback: Option<InsightFeedback>,
) -> InsightNote {
    InsightNote {
        offset_ms,
        severity: "green".to_string(),
        message: format!("cue {id}"),
        segment_index: Some(segment_index),
        id: Some(id.to_string()),
        producer: Some(InsightProducer::OnDevice),
        feedback,
        ..InsightNote::default()
    }
}

fn said(text: &str) -> TranscriptSegment {
    TranscriptSegment {
        source: TranscriptSource::Others,
        utterance: text.to_string(),
        speaker_id: None,
        speaker_label: None,
    }
}

fn finished(
    session_id: &str,
    started_at: &str,
    segments: usize,
    insights: Vec<InsightNote>,
) -> SessionRecord {
    let mut record: SessionRecord = serde_json::from_value(json!({
        "session_id": session_id,
        "relay_session_id": session_id,
        "started_at": started_at,
        "ended_at": started_at,
        "segments": [],
        "notes_status": "completed",
    }))
    .unwrap();
    record.segments = (0..segments)
        .map(|index| said(&format!("line {index}")))
        .collect();
    record.insights = insights;
    record
}

#[test]
fn an_insight_written_by_an_older_build_round_trips_without_new_keys() {
    let older = json!({
        "offset_ms": 4_000,
        "severity": "green",
        "message": "Ask about the timeline",
        "segment_index": 2
    });

    let note: InsightNote = serde_json::from_value(older.clone()).unwrap();

    assert_eq!(note.id, None);
    assert_eq!(note.producer, None);
    assert_eq!(note.feedback, None);
    assert_eq!(serde_json::to_value(&note).unwrap(), older);
}

#[test]
fn a_live_record_written_by_an_older_build_keeps_its_exact_insight_shape() {
    let older_insights = json!([
        {"offset_ms": 1_000, "severity": "green", "message": "Ask why", "segment_index": null},
        {"offset_ms": 2_000, "severity": "orange", "message": "Slow down", "segment_index": 0,
         "source": {"title": "Plan", "source_id": "clickup"}}
    ]);
    let record: LiveSessionRecord = serde_json::from_value(json!({
        "session_id": "sess-old",
        "started_at_ms": 1,
        "insights": older_insights.clone(),
    }))
    .unwrap();

    let rewritten = serde_json::to_value(&record).unwrap();

    assert_eq!(rewritten["insights"], older_insights);
}

#[test]
fn the_new_fields_use_the_shared_vocabulary() {
    let note = InsightNote {
        producer: Some(InsightProducer::Signals),
        feedback: Some(InsightFeedback {
            surface: FeedbackSurface::Library,
            publish_pending: true,
            ..rated(InsightVerdict::NotHelpful, "2026-10-30T10:01:00Z")
        }),
        ..identified("i-1", 4_000, 0, None)
    };

    let value = serde_json::to_value(&note).unwrap();

    assert_eq!(value["id"], "i-1");
    assert_eq!(value["producer"], "signals");
    assert_eq!(
        value["feedback"],
        json!({"verdict": "not_helpful", "rated_at": "2026-10-30T10:01:00Z", "surface": "library", "publish_pending": true})
    );
    assert!(value.get("feedback_withdraw_pending").is_none());
    let back: InsightNote = serde_json::from_value(value).unwrap();
    assert_eq!(back, note);
}

#[test]
fn a_value_from_a_later_build_does_not_break_the_record() {
    let note: InsightNote = serde_json::from_value(json!({
        "offset_ms": 4_000,
        "severity": "green",
        "message": "Ask why",
        "id": "i-1",
        "producer": "a_future_producer",
        "feedback": {"verdict": "a_future_verdict", "rated_at": "2026-10-30T10:01:00Z", "surface": "live"}
    }))
    .unwrap();

    assert_eq!(note.id.as_deref(), Some("i-1"));
    assert_eq!(note.producer, None);
    assert_eq!(note.feedback, None);
}

#[test]
fn split_and_merge_keep_ids_producers_and_feedback() {
    let store = temp_store("split_merge");
    let helpful = rated(InsightVerdict::Helpful, "2026-10-30T10:01:00Z");
    let dismissed = rated(InsightVerdict::Dismissed, "2026-10-30T10:02:00Z");
    store
        .save(&finished(
            "sess-whole",
            "2026-10-30T10:00:00Z",
            4,
            vec![
                identified("early", 1_000, 0, Some(helpful.clone())),
                identified("late", 3_000, 3, Some(dismissed.clone())),
            ],
        ))
        .unwrap();

    let (first_id, second_id) = store.split("sess-whole", 2).unwrap();
    let first = store.load(&first_id).unwrap().unwrap();
    let second = store.load(&second_id).unwrap().unwrap();
    assert_eq!(
        first.insights,
        vec![identified("early", 1_000, 0, Some(helpful.clone()))]
    );
    assert_eq!(
        second.insights,
        vec![identified("late", 3_000, 1, Some(dismissed.clone()))]
    );

    let merged_id = store.merge(&first_id, &second_id).unwrap();
    let merged = store.load(&merged_id).unwrap().unwrap();
    assert_eq!(merged.insights[0].feedback, Some(helpful));
    assert_eq!(merged.insights[1].id.as_deref(), Some("late"));
    assert_eq!(merged.insights[1].producer, Some(InsightProducer::OnDevice));
    assert_eq!(merged.insights[1].feedback, Some(dismissed));
    assert_eq!(merged.insights[1].segment_index, Some(3));
}

#[test]
fn appending_matches_insights_by_id_and_carries_the_latest_verdict() {
    let mut saved = vec![identified("i-1", 4_000, 0, None)];
    let later_offset = identified(
        "i-1",
        4_250,
        0,
        Some(rated(InsightVerdict::NotHelpful, "2026-10-30T10:01:00Z")),
    );

    assert!(append_missing_insights(&mut saved, &[later_offset]));
    assert_eq!(saved.len(), 1);
    assert_eq!(
        saved[0].feedback.as_ref().unwrap().verdict,
        InsightVerdict::NotHelpful
    );

    assert!(append_missing_insights(
        &mut saved,
        &[identified("i-1", 4_000, 0, None)]
    ));
    assert_eq!(saved[0].feedback, None);
    assert!(!append_missing_insights(
        &mut saved,
        &[identified("i-1", 4_000, 0, None)]
    ));
}

#[test]
fn an_insight_without_an_id_still_matches_by_offset_and_text() {
    let older = InsightNote {
        offset_ms: 4_000,
        severity: "green".to_string(),
        message: "Ask why".to_string(),
        ..InsightNote::default()
    };
    let mut saved = vec![older.clone()];

    assert!(!append_missing_insights(&mut saved, &[older]));
    assert_eq!(saved.len(), 1);
}

struct QuietHost;

impl NotesHost for QuietHost {
    async fn send_summary(&self, _: Uuid, _: String, _: String, _: Uuid) -> Result<(), String> {
        Ok(())
    }

    async fn summarize_local(&self, _: Uuid, _: String, _: String) -> Result<(), String> {
        Ok(())
    }

    fn register_pending(&self, _: Uuid, _: &str) {}

    fn settings(&self) -> NotesSettings {
        NotesSettings {
            has_relay: true,
            pairing_blocked: None,
            use_local_summary: false,
            use_hosted_summary: false,
        }
    }

    fn emit(&self, _: NotesEvent) {}
}

#[tokio::test]
async fn a_verdict_given_while_the_call_is_finalized_reaches_the_library_record() {
    let store = temp_store("finalize");
    let session = Uuid::new_v4();
    let key = session.to_string();
    store.live_begin(&key, 1_000).unwrap();
    store.live_set_backend_session(&key, "backend-9").unwrap();
    store
        .live_append_segment(&key, TranscriptSource::Others, "we would wait", 2_000)
        .unwrap();
    store
        .live_append_new_insight(
            &key,
            NewInsight {
                id: Some("sig".to_string()),
                producer: Some(InsightProducer::Signals),
                severity: "green".to_string(),
                message: "Ask what waiting costs".to_string(),
                segment_index: Some(0),
                source: None,
            },
            5_000,
        )
        .unwrap();
    let live = store.live_load(&key).unwrap().unwrap();

    store
        .set_insight_feedback(
            &key,
            "sig",
            Some(rated(InsightVerdict::Helpful, "2026-10-30T10:01:00Z")),
        )
        .unwrap();
    finalize_inner(
        &QuietHost,
        &store,
        session,
        live.segments,
        live.insights,
        Vec::new(),
        1_000,
        9_000,
        SessionParticipants {
            attendees: Vec::new(),
            calendar_event_id: None,
        },
        Vec::new(),
    )
    .await;
    store.live_merge_insights_then_delete(&key).unwrap();

    let saved = store.load(&key).unwrap().unwrap();
    assert_eq!(saved.insights.len(), 1);
    assert_eq!(saved.insights[0].id.as_deref(), Some("sig"));
    assert_eq!(saved.insights[0].producer, Some(InsightProducer::Signals));
    let feedback = saved.insights[0].feedback.as_ref().unwrap();
    assert_eq!(feedback.verdict, InsightVerdict::Helpful);
    assert!(feedback.publish_pending);
    let unpublished = store.unpublished_insight_feedback().unwrap();
    assert_eq!(unpublished.len(), 1);
    assert_eq!(unpublished[0].backend_session_id, "backend-9");
}
