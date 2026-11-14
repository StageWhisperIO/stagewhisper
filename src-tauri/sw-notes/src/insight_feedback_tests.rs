use super::*;
use crate::accumulate::TranscriptSource;
use crate::insight_store::NewInsight;
use crate::store::SessionRecord;

const BACKEND_SESSION: &str = "backend-7";

fn temp_store(tag: &str) -> SessionStore {
    let dir = std::env::temp_dir().join(format!(
        "sw_notes_insight_feedback_{tag}_{}_{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    SessionStore::new(dir, [0x5e; 32]).unwrap()
}

fn rating(verdict: InsightVerdict, rated_at: &str) -> InsightFeedback {
    InsightFeedback {
        verdict,
        rated_at: rated_at.to_string(),
        surface: FeedbackSurface::Live,
        publish_pending: false,
    }
}

fn insight(id: &str, producer: InsightProducer, message: &str) -> NewInsight {
    NewInsight {
        id: Some(id.to_string()),
        producer: Some(producer),
        severity: "green".to_string(),
        message: message.to_string(),
        segment_index: Some(0),
        source: None,
    }
}

fn live_call(store: &SessionStore, session_id: &str, backend: Option<&str>) {
    store.live_begin(session_id, 1_000).unwrap();
    if let Some(backend) = backend {
        store.live_set_backend_session(session_id, backend).unwrap();
    }
    store
        .live_append_segment(session_id, TranscriptSource::Others, "we would wait", 2_000)
        .unwrap();
}

fn finished_record(
    session_id: &str,
    backend: Option<&str>,
    insights: Vec<InsightNote>,
) -> SessionRecord {
    let mut record: SessionRecord = serde_json::from_value(serde_json::json!({
        "session_id": session_id,
        "relay_session_id": session_id,
        "started_at": "2026-10-30T10:00:00Z",
        "ended_at": "2026-10-30T10:30:00Z",
        "segments": [],
    }))
    .unwrap();
    record.backend_session_id = backend.map(str::to_string);
    record.insights = insights;
    record
}

fn note(id: &str, producer: InsightProducer, message: &str) -> InsightNote {
    InsightNote {
        offset_ms: 4_000,
        severity: "green".to_string(),
        message: message.to_string(),
        id: Some(id.to_string()),
        producer: Some(producer),
        ..InsightNote::default()
    }
}

fn live_note(store: &SessionStore, session_id: &str, insight_id: &str) -> InsightNote {
    store
        .live_load(session_id)
        .unwrap()
        .unwrap()
        .insights
        .into_iter()
        .find(|note| note.has_id(insight_id))
        .unwrap()
}

fn saved_note(store: &SessionStore, session_id: &str, insight_id: &str) -> InsightNote {
    store
        .load(session_id)
        .unwrap()
        .unwrap()
        .insights
        .into_iter()
        .find(|note| note.has_id(insight_id))
        .unwrap()
}

#[test]
fn a_live_insight_keeps_its_id_producer_and_dispatch_time_anchor() {
    let store = temp_store("append");
    live_call(&store, "sess-a", None);
    store
        .live_append_segment("sess-a", TranscriptSource::You, "later remark", 3_000)
        .unwrap();

    let appended = store
        .live_append_new_insight(
            "sess-a",
            insight("i-1", InsightProducer::OnDevice, "Ask why"),
            9_000,
        )
        .unwrap();
    let repeated_id = store
        .live_append_new_insight(
            "sess-a",
            insight("i-1", InsightProducer::OnDevice, "Other text"),
            9_500,
        )
        .unwrap();
    let repeated_text = store
        .live_append_new_insight(
            "sess-a",
            insight("i-2", InsightProducer::OnDevice, "Ask why"),
            9_600,
        )
        .unwrap();

    assert!(appended);
    assert!(!repeated_id);
    assert!(!repeated_text);
    let live = store.live_load("sess-a").unwrap().unwrap();
    assert_eq!(live.insights.len(), 1);
    assert_eq!(live.insights[0].id.as_deref(), Some("i-1"));
    assert_eq!(live.insights[0].producer, Some(InsightProducer::OnDevice));
    assert_eq!(live.insights[0].segment_index, Some(0));
    assert_eq!(live.insights[0].offset_ms, 8_000);
}

#[test]
fn feedback_is_set_and_cleared_on_the_live_record() {
    let store = temp_store("live");
    live_call(&store, "sess-l", None);
    store
        .live_append_new_insight(
            "sess-l",
            insight("i-1", InsightProducer::OnDevice, "Ask why"),
            5_000,
        )
        .unwrap();

    let set = store
        .set_insight_feedback(
            "sess-l",
            "i-1",
            Some(rating(InsightVerdict::NotHelpful, "2026-10-30T10:01:00Z")),
        )
        .unwrap();
    assert!(set);
    let rated = live_note(&store, "sess-l", "i-1");
    assert_eq!(
        rated.feedback.as_ref().unwrap().verdict,
        InsightVerdict::NotHelpful
    );
    assert!(!rated.feedback_unpublished());

    assert!(store.set_insight_feedback("sess-l", "i-1", None).unwrap());
    let cleared = live_note(&store, "sess-l", "i-1");
    assert_eq!(cleared.feedback, None);
    assert!(!cleared.feedback_withdraw_pending);
}

#[test]
fn feedback_falls_back_to_the_finished_record() {
    let store = temp_store("finished");
    store
        .save(&finished_record(
            "sess-f",
            None,
            vec![note("i-1", InsightProducer::Assistant, "Ask why")],
        ))
        .unwrap();

    let feedback = InsightFeedback {
        surface: FeedbackSurface::Library,
        ..rating(InsightVerdict::Helpful, "2026-10-30T11:00:00Z")
    };
    assert!(store
        .set_insight_feedback("sess-f", "i-1", Some(feedback.clone()))
        .unwrap());
    assert_eq!(saved_note(&store, "sess-f", "i-1").feedback, Some(feedback));

    assert!(store.set_insight_feedback("sess-f", "i-1", None).unwrap());
    assert_eq!(saved_note(&store, "sess-f", "i-1").feedback, None);
}

#[test]
fn an_unknown_insight_id_is_reported_and_nothing_is_written() {
    let store = temp_store("unknown");
    live_call(&store, "sess-u", None);
    store
        .save(&finished_record(
            "sess-u2",
            None,
            vec![note("i-1", InsightProducer::OnDevice, "Ask why")],
        ))
        .unwrap();
    let helpful = || Some(rating(InsightVerdict::Helpful, "2026-10-30T10:01:00Z"));

    assert!(!store
        .set_insight_feedback("sess-u", "missing", helpful())
        .unwrap());
    assert!(!store
        .set_insight_feedback("sess-u2", "missing", helpful())
        .unwrap());
    assert!(!store
        .set_insight_feedback("sess-u2", "", helpful())
        .unwrap());
    assert!(!store
        .set_insight_feedback("no-such-session", "i-1", helpful())
        .unwrap());
}

#[test]
fn while_both_records_exist_both_get_the_verdict() {
    let store = temp_store("both");
    live_call(&store, "sess-b", None);
    store
        .live_append_new_insight(
            "sess-b",
            insight("i-1", InsightProducer::OnDevice, "Ask why"),
            5_000,
        )
        .unwrap();
    let snapshot = store.live_load("sess-b").unwrap().unwrap().insights;
    store
        .save(&finished_record("sess-b", None, snapshot))
        .unwrap();

    store
        .set_insight_feedback(
            "sess-b",
            "i-1",
            Some(rating(InsightVerdict::Dismissed, "2026-10-30T10:02:00Z")),
        )
        .unwrap();

    assert_eq!(
        live_note(&store, "sess-b", "i-1").feedback.unwrap().verdict,
        InsightVerdict::Dismissed
    );
    assert_eq!(
        saved_note(&store, "sess-b", "i-1")
            .feedback
            .unwrap()
            .verdict,
        InsightVerdict::Dismissed
    );
}

#[test]
fn only_a_signals_verdict_on_a_backend_session_waits_to_be_published() {
    let store = temp_store("pending");
    live_call(&store, "sess-s", Some(BACKEND_SESSION));
    live_call(&store, "sess-n", None);
    for (session, id, producer) in [
        ("sess-s", "sig", InsightProducer::Signals),
        ("sess-s", "dev", InsightProducer::OnDevice),
        ("sess-s", "asst", InsightProducer::Assistant),
        ("sess-n", "sig-no-backend", InsightProducer::Signals),
    ] {
        store
            .live_append_new_insight(session, insight(id, producer, id), 5_000)
            .unwrap();
        let mut asked = rating(InsightVerdict::Helpful, "2026-10-30T10:01:00Z");
        asked.publish_pending = producer != InsightProducer::Signals;
        store
            .set_insight_feedback(session, id, Some(asked))
            .unwrap();
    }

    assert!(
        live_note(&store, "sess-s", "sig")
            .feedback
            .unwrap()
            .publish_pending
    );
    assert!(
        !live_note(&store, "sess-s", "dev")
            .feedback
            .unwrap()
            .publish_pending
    );
    assert!(
        !live_note(&store, "sess-s", "asst")
            .feedback
            .unwrap()
            .publish_pending
    );
    assert!(
        !live_note(&store, "sess-n", "sig-no-backend")
            .feedback
            .unwrap()
            .publish_pending
    );
    let unpublished = store.unpublished_insight_feedback().unwrap();
    assert_eq!(unpublished.len(), 1);
    assert_eq!(unpublished[0].session_id, "sess-s");
    assert_eq!(unpublished[0].backend_session_id, BACKEND_SESSION);
    assert_eq!(unpublished[0].insight_id, "sig");
}

#[test]
fn a_published_verdict_stops_being_retried_but_a_newer_one_is_kept() {
    let store = temp_store("publish");
    store
        .save(&finished_record(
            "sess-p",
            Some(BACKEND_SESSION),
            vec![note("sig", InsightProducer::Signals, "Ask why")],
        ))
        .unwrap();
    store
        .set_insight_feedback(
            "sess-p",
            "sig",
            Some(rating(InsightVerdict::Helpful, "2026-10-30T11:00:00Z")),
        )
        .unwrap();
    let first = store.unpublished_insight_feedback().unwrap();
    assert_eq!(first.len(), 1);
    let sent = first[0].feedback.clone().unwrap();

    store
        .set_insight_feedback(
            "sess-p",
            "sig",
            Some(rating(InsightVerdict::NotHelpful, "2026-10-30T11:00:05Z")),
        )
        .unwrap();
    store
        .mark_insight_feedback_published("sess-p", "sig", Some(&sent))
        .unwrap();
    let still_waiting = store.unpublished_insight_feedback().unwrap();
    assert_eq!(still_waiting.len(), 1);
    assert_eq!(
        still_waiting[0].feedback.as_ref().unwrap().verdict,
        InsightVerdict::NotHelpful
    );

    store
        .mark_insight_feedback_published("sess-p", "sig", still_waiting[0].feedback.as_ref())
        .unwrap();
    assert!(store.unpublished_insight_feedback().unwrap().is_empty());
    assert!(
        !saved_note(&store, "sess-p", "sig")
            .feedback
            .unwrap()
            .publish_pending
    );
}

#[test]
fn clearing_a_signals_verdict_waits_until_the_backend_forgets_it() {
    let store = temp_store("withdraw");
    store
        .save(&finished_record(
            "sess-w",
            Some(BACKEND_SESSION),
            vec![note("sig", InsightProducer::Signals, "Ask why")],
        ))
        .unwrap();
    let helpful = rating(InsightVerdict::Helpful, "2026-10-30T11:00:00Z");
    store
        .set_insight_feedback("sess-w", "sig", Some(helpful.clone()))
        .unwrap();
    store
        .mark_insight_feedback_published("sess-w", "sig", Some(&helpful))
        .unwrap();

    store.set_insight_feedback("sess-w", "sig", None).unwrap();

    let withdraw = store.unpublished_insight_feedback().unwrap();
    assert_eq!(withdraw.len(), 1);
    assert_eq!(withdraw[0].feedback, None);
    store
        .mark_insight_feedback_published("sess-w", "sig", None)
        .unwrap();
    assert!(store.unpublished_insight_feedback().unwrap().is_empty());
    let settled = saved_note(&store, "sess-w", "sig");
    assert_eq!(settled.feedback, None);
    assert!(!settled.feedback_withdraw_pending);
}

#[test]
fn rating_again_after_a_clear_replaces_the_pending_withdrawal() {
    let store = temp_store("rerate");
    store
        .save(&finished_record(
            "sess-r",
            Some(BACKEND_SESSION),
            vec![note("sig", InsightProducer::Signals, "Ask why")],
        ))
        .unwrap();
    store
        .set_insight_feedback(
            "sess-r",
            "sig",
            Some(rating(InsightVerdict::Helpful, "2026-10-30T11:00:00Z")),
        )
        .unwrap();
    store.set_insight_feedback("sess-r", "sig", None).unwrap();
    store
        .set_insight_feedback(
            "sess-r",
            "sig",
            Some(rating(InsightVerdict::Dismissed, "2026-10-30T11:00:09Z")),
        )
        .unwrap();

    let unpublished = store.unpublished_insight_feedback().unwrap();
    assert_eq!(unpublished.len(), 1);
    assert_eq!(
        unpublished[0].feedback.as_ref().unwrap().verdict,
        InsightVerdict::Dismissed
    );
    assert!(!saved_note(&store, "sess-r", "sig").feedback_withdraw_pending);
}

#[test]
fn a_verdict_rated_during_the_call_is_retried_once_on_both_records() {
    let store = temp_store("dual_pending");
    live_call(&store, "sess-d", Some(BACKEND_SESSION));
    store
        .live_append_new_insight(
            "sess-d",
            insight("sig", InsightProducer::Signals, "Ask why"),
            5_000,
        )
        .unwrap();
    let mut snapshot = finished_record("sess-d", Some(BACKEND_SESSION), Vec::new());
    snapshot.insights = store.live_load("sess-d").unwrap().unwrap().insights;
    store.save(&snapshot).unwrap();
    let helpful = rating(InsightVerdict::Helpful, "2026-10-30T10:01:00Z");
    store
        .set_insight_feedback("sess-d", "sig", Some(helpful.clone()))
        .unwrap();

    assert_eq!(store.unpublished_insight_feedback().unwrap().len(), 1);
    store
        .mark_insight_feedback_published("sess-d", "sig", Some(&helpful))
        .unwrap();
    assert!(store.unpublished_insight_feedback().unwrap().is_empty());
    assert!(
        !live_note(&store, "sess-d", "sig")
            .feedback
            .unwrap()
            .publish_pending
    );
    assert!(
        !saved_note(&store, "sess-d", "sig")
            .feedback
            .unwrap()
            .publish_pending
    );
}
