use super::*;
use crate::accumulate::{TranscriptSegment, TranscriptSource};
use crate::store::SessionRecord;
use uuid::Uuid;

fn temp_store(tag: &str) -> SessionStore {
    let dir = std::env::temp_dir().join(format!(
        "sw_notes_outcome_{tag}_{}_{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    SessionStore::new(dir, [0x3c; 32]).unwrap()
}

fn saved_session(store: &SessionStore, session_id: &str) {
    store
        .save(&SessionRecord {
            session_id: session_id.to_string(),
            relay_session_id: session_id.to_string(),
            backend_session_id: None,
            started_at: "2026-09-23T10:00:00Z".to_string(),
            ended_at: "2026-09-23T10:30:00Z".to_string(),
            title: None,
            title_is_auto: false,
            segments: vec![TranscriptSegment {
                source: TranscriptSource::Others,
                utterance: "we will take it to the board".to_string(),
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
            outcome: None,
        })
        .unwrap();
}

fn crm_write(converted: OutcomeConverted) -> OutcomeWrite {
    OutcomeWrite {
        felt: None,
        converted: Some(converted),
        decided_at: Some(CLOSED_AT.to_string()),
        provenance: OutcomeProvenance::Crm,
        source: Some("pipedrive".to_string()),
    }
}

const NOW: &str = "2026-09-23T10:31:00Z";
const CLOSED_AT: &str = "2026-10-14T09:00:00Z";

#[test]
fn recording_how_a_call_felt_stores_it_as_a_human_label() {
    let store = temp_store("felt");
    saved_session(&store, "s1");

    let result = store
        .set_outcome(
            "s1",
            &OutcomeWrite::by_user(Some(OutcomeFelt::Good), None),
            NOW,
        )
        .unwrap()
        .unwrap();

    assert_eq!(result.felt, LabelWriteResult::Recorded);
    assert_eq!(result.converted, LabelWriteResult::NotOffered);
    let felt = store.outcome("s1").unwrap().unwrap().felt.unwrap();
    assert_eq!(felt.value, OutcomeFelt::Good);
    assert_eq!(felt.provenance, OutcomeProvenance::User);
    assert_eq!(felt.decided_at, NOW);
}

#[test]
fn a_crm_can_record_the_close_weeks_after_the_user_said_how_it_felt() {
    let store = temp_store("fill");
    saved_session(&store, "s1");
    store
        .set_outcome(
            "s1",
            &OutcomeWrite::by_user(Some(OutcomeFelt::Good), None),
            NOW,
        )
        .unwrap();

    let result = store
        .set_outcome("s1", &crm_write(OutcomeConverted::Won), NOW)
        .unwrap()
        .unwrap();

    assert_eq!(result.converted, LabelWriteResult::Recorded);
    let outcome = store.outcome("s1").unwrap().unwrap();
    assert_eq!(outcome.felt.as_ref().unwrap().value, OutcomeFelt::Good);
    assert_eq!(
        outcome.felt.as_ref().unwrap().provenance,
        OutcomeProvenance::User
    );
    let converted = outcome.converted.unwrap();
    assert_eq!(converted.value, OutcomeConverted::Won);
    assert_eq!(converted.provenance, OutcomeProvenance::Crm);
    assert_eq!(converted.decided_at, CLOSED_AT);
    assert_eq!(converted.source.as_deref(), Some("pipedrive"));
}

#[test]
fn a_crm_never_overwrites_the_conversion_a_human_already_recorded() {
    let store = temp_store("refuse");
    saved_session(&store, "s1");
    store
        .set_outcome(
            "s1",
            &OutcomeWrite::by_user(None, Some(OutcomeConverted::Won)),
            NOW,
        )
        .unwrap();

    let result = store
        .set_outcome("s1", &crm_write(OutcomeConverted::Lost), NOW)
        .unwrap()
        .unwrap();

    assert_eq!(result.converted, LabelWriteResult::RefusedOverHumanLabel);
    let converted = store.outcome("s1").unwrap().unwrap().converted.unwrap();
    assert_eq!(converted.value, OutcomeConverted::Won);
    assert_eq!(converted.provenance, OutcomeProvenance::User);
}

#[test]
fn a_human_label_replaces_one_that_a_crm_recorded_earlier() {
    let store = temp_store("replace");
    saved_session(&store, "s1");
    store
        .set_outcome("s1", &crm_write(OutcomeConverted::Lost), NOW)
        .unwrap();

    let result = store
        .set_outcome(
            "s1",
            &OutcomeWrite::by_user(None, Some(OutcomeConverted::Won)),
            NOW,
        )
        .unwrap()
        .unwrap();

    assert_eq!(result.converted, LabelWriteResult::Recorded);
    let converted = store.outcome("s1").unwrap().unwrap().converted.unwrap();
    assert_eq!(converted.value, OutcomeConverted::Won);
    assert_eq!(converted.provenance, OutcomeProvenance::User);
    assert_eq!(converted.source, None);
}

#[test]
fn repeating_the_same_write_leaves_the_record_untouched() {
    let store = temp_store("idempotent");
    saved_session(&store, "s1");
    let write = crm_write(OutcomeConverted::Won);
    store.set_outcome("s1", &write, NOW).unwrap();

    let result = store.set_outcome("s1", &write, NOW).unwrap().unwrap();

    assert_eq!(result.converted, LabelWriteResult::Unchanged);
    assert!(!result.changed_anything());
}

#[test]
fn a_write_carrying_no_label_records_nothing() {
    let store = temp_store("empty");
    saved_session(&store, "s1");

    let result = store
        .set_outcome("s1", &OutcomeWrite::by_user(None, None), NOW)
        .unwrap()
        .unwrap();

    assert_eq!(result.felt, LabelWriteResult::NotOffered);
    assert_eq!(result.converted, LabelWriteResult::NotOffered);
    assert!(store.outcome("s1").unwrap().is_none());
}

#[test]
fn labelling_a_session_that_is_not_in_the_library_reports_it_as_missing() {
    let store = temp_store("missing");

    let result = store
        .set_outcome(
            "nope",
            &OutcomeWrite::by_user(Some(OutcomeFelt::Bad), None),
            NOW,
        )
        .unwrap();

    assert!(result.is_none());
}

#[test]
fn a_recorded_outcome_shows_up_in_the_library_listing() {
    let store = temp_store("listing");
    saved_session(&store, "s1");
    store
        .set_outcome(
            "s1",
            &OutcomeWrite::by_user(Some(OutcomeFelt::Bad), None),
            NOW,
        )
        .unwrap();

    let summaries = store.list().unwrap();

    assert_eq!(summaries.len(), 1);
    assert_eq!(
        summaries[0]
            .outcome
            .as_ref()
            .and_then(|o| o.felt.as_ref())
            .map(|l| l.value),
        Some(OutcomeFelt::Bad)
    );
}

#[test]
fn merging_two_calls_keeps_the_human_label_over_the_inferred_one() {
    let store = temp_store("merge");
    saved_session(&store, "s1");
    saved_session(&store, "s2");
    store
        .set_outcome("s1", &crm_write(OutcomeConverted::Lost), NOW)
        .unwrap();
    store
        .set_outcome(
            "s2",
            &OutcomeWrite::by_user(Some(OutcomeFelt::Good), Some(OutcomeConverted::Won)),
            NOW,
        )
        .unwrap();

    let merged_id = store.merge("s1", "s2").unwrap();

    let outcome = store.outcome(&merged_id).unwrap().unwrap();
    assert_eq!(outcome.felt.unwrap().value, OutcomeFelt::Good);
    let converted = outcome.converted.unwrap();
    assert_eq!(converted.value, OutcomeConverted::Won);
    assert_eq!(converted.provenance, OutcomeProvenance::User);
}

#[test]
fn an_older_build_reading_a_labelled_session_file_is_not_broken_by_it() {
    let store = temp_store("compat");
    saved_session(&store, "s1");
    store
        .set_outcome(
            "s1",
            &OutcomeWrite::by_user(Some(OutcomeFelt::Good), None),
            NOW,
        )
        .unwrap();
    let record = store.load("s1").unwrap().unwrap();
    let encoded = serde_json::to_value(&record).unwrap();

    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct RecordWithoutOutcome {
        session_id: String,
        relay_session_id: String,
        started_at: String,
        ended_at: String,
        segments: Vec<TranscriptSegment>,
    }

    let older: RecordWithoutOutcome = serde_json::from_value(encoded).unwrap();
    assert_eq!(older.session_id, "s1");
}

#[test]
fn a_session_file_written_before_outcomes_existed_still_loads() {
    let store = temp_store("forward");
    saved_session(&store, "s1");
    let mut encoded = serde_json::to_value(store.load("s1").unwrap().unwrap()).unwrap();
    encoded.as_object_mut().unwrap().remove("outcome");

    let record: SessionRecord = serde_json::from_value(encoded).unwrap();

    assert!(record.outcome.is_none());
}

fn saved_backend_session(store: &SessionStore, session_id: &str) {
    saved_session(store, session_id);
    let mut record = store.load(session_id).unwrap().unwrap();
    record.backend_session_id = Some(format!("backend-{session_id}"));
    store.save(&record).unwrap();
}

#[test]
fn a_label_on_a_call_the_backend_knows_waits_to_be_published() {
    let store = temp_store("pending");
    saved_backend_session(&store, "s1");

    store
        .set_outcome("s1", &crm_write(OutcomeConverted::Won), NOW)
        .unwrap();

    let unpublished = store.unpublished_outcomes().unwrap();
    assert_eq!(unpublished.len(), 1);
    assert_eq!(unpublished[0].backend_session_id, "backend-s1");
    assert_eq!(unpublished[0].write, crm_write(OutcomeConverted::Won));
}

#[test]
fn a_label_on_a_call_the_backend_never_saw_is_not_waiting_for_anything() {
    let store = temp_store("no_backend");
    saved_session(&store, "s1");

    store
        .set_outcome("s1", &crm_write(OutcomeConverted::Won), NOW)
        .unwrap();

    assert!(store.unpublished_outcomes().unwrap().is_empty());
}

#[test]
fn marking_a_label_published_stops_it_from_being_retried() {
    let store = temp_store("published");
    saved_backend_session(&store, "s1");
    store
        .set_outcome("s1", &crm_write(OutcomeConverted::Won), NOW)
        .unwrap();

    store
        .mark_outcome_published("s1", &crm_write(OutcomeConverted::Won))
        .unwrap();

    assert!(store.unpublished_outcomes().unwrap().is_empty());
    assert_eq!(
        store
            .outcome("s1")
            .unwrap()
            .unwrap()
            .converted
            .unwrap()
            .value,
        OutcomeConverted::Won
    );
}

#[test]
fn a_late_publish_confirmation_does_not_clear_a_newer_label() {
    let store = temp_store("newer");
    saved_backend_session(&store, "s1");
    store
        .set_outcome("s1", &crm_write(OutcomeConverted::Open), NOW)
        .unwrap();
    store
        .set_outcome(
            "s1",
            &OutcomeWrite::by_user(None, Some(OutcomeConverted::Won)),
            NOW,
        )
        .unwrap();

    store
        .mark_outcome_published("s1", &crm_write(OutcomeConverted::Open))
        .unwrap();

    let unpublished = store.unpublished_outcomes().unwrap();
    assert_eq!(unpublished.len(), 1);
    assert_eq!(unpublished[0].write.converted, Some(OutcomeConverted::Won));
    assert_eq!(unpublished[0].write.provenance, OutcomeProvenance::User);
}

#[test]
fn a_session_file_without_the_pending_marker_loads_as_already_published() {
    let store = temp_store("marker_forward");
    saved_backend_session(&store, "s1");
    store
        .set_outcome("s1", &crm_write(OutcomeConverted::Won), NOW)
        .unwrap();
    store
        .mark_outcome_published("s1", &crm_write(OutcomeConverted::Won))
        .unwrap();

    let encoded = serde_json::to_value(store.load("s1").unwrap().unwrap()).unwrap();

    assert!(encoded["outcome"]["converted"]
        .get("publish_pending")
        .is_none());
    let record: SessionRecord = serde_json::from_value(encoded).unwrap();
    assert!(!record.outcome.unwrap().converted.unwrap().publish_pending);
}
