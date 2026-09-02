use std::sync::Mutex;

use super::*;
use crate::accumulate::TranscriptSource;

#[derive(Clone, Copy)]
enum SendOutcome {
    Succeed,
    Fail,
}

struct MockHost {
    send_outcome: SendOutcome,
    use_local_summary: bool,
    pending_user_message_id: Mutex<Option<String>>,
    pending_id_seen_by_local_generation: Mutex<Option<String>>,
}

impl MockHost {
    fn new(send_outcome: SendOutcome) -> Self {
        Self {
            send_outcome,
            use_local_summary: false,
            pending_user_message_id: Mutex::new(None),
            pending_id_seen_by_local_generation: Mutex::new(None),
        }
    }

    fn new_local(send_outcome: SendOutcome) -> Self {
        Self {
            send_outcome,
            use_local_summary: true,
            pending_user_message_id: Mutex::new(None),
            pending_id_seen_by_local_generation: Mutex::new(None),
        }
    }

    fn pending_id(&self) -> String {
        self.pending_user_message_id
            .lock()
            .unwrap()
            .clone()
            .expect("a NotesPending event should have carried the attempt id")
    }

    fn pending_id_seen_by_local_generation(&self) -> Option<String> {
        self.pending_id_seen_by_local_generation
            .lock()
            .unwrap()
            .clone()
    }
}

impl NotesHost for MockHost {
    async fn send_summary(
        &self,
        _session: Uuid,
        _message: String,
        _user_message_id: String,
        _task_id: Uuid,
    ) -> Result<(), String> {
        match self.send_outcome {
            SendOutcome::Succeed => Ok(()),
            SendOutcome::Fail => Err("relay unreachable".to_string()),
        }
    }

    async fn summarize_local(
        &self,
        _session: Uuid,
        _message: String,
        _user_message_id: String,
    ) -> Result<(), String> {
        *self.pending_id_seen_by_local_generation.lock().unwrap() =
            self.pending_user_message_id.lock().unwrap().clone();
        match self.send_outcome {
            SendOutcome::Succeed => Ok(()),
            SendOutcome::Fail => Err("local model busy".to_string()),
        }
    }

    fn register_pending(&self, _task_id: Uuid, _session_id: &str) {}

    fn settings(&self) -> NotesSettings {
        NotesSettings {
            has_relay: true,
            pairing_blocked: None,
            use_local_summary: self.use_local_summary,
        }
    }

    fn emit(&self, event: NotesEvent) {
        if let NotesEvent::NotesPending {
            user_message_id, ..
        } = event
        {
            *self.pending_user_message_id.lock().unwrap() = Some(user_message_id);
        }
    }
}

fn temp_store(tag: &str) -> SessionStore {
    let dir = std::env::temp_dir().join(format!(
        "sw-notes-finalize-regeneration-{tag}-{}-{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    SessionStore::new(dir, [7u8; 32]).unwrap()
}

fn segments() -> Vec<TranscriptSegment> {
    vec![TranscriptSegment {
        source: TranscriptSource::You,
        utterance: "hello world".to_string(),
        speaker_id: None,
        speaker_label: None,
    }]
}

fn seed_completed_summary(store: &SessionStore, session_id: Uuid) {
    let record = SessionRecord {
        session_id: session_id.to_string(),
        relay_session_id: session_id.to_string(),
        started_at: "2026-01-01T00:00:00Z".to_string(),
        ended_at: "2026-01-01T00:05:00Z".to_string(),
        title: None,
        title_is_auto: false,
        segments: segments(),
        insights: Vec::new(),
        blocks: Vec::new(),
        notes_markdown: Some("# Old summary\n\nthe original recap".to_string()),
        notes_status: Some("completed".to_string()),
        notes_error: None,
        notes_root_message_id: Some("old-root".to_string()),
        notes_pending_root_message_id: None,
        chat: vec![ChatMsg {
            id: "old-reply".to_string(),
            role: "assistant".to_string(),
            content: "# Old summary\n\nthe original recap".to_string(),
            status: "completed".to_string(),
            parent_message_id: Some("old-root".to_string()),
            error_code: None,
            error_message: None,
            created_at: "2026-01-01T00:05:00Z".to_string(),
        }],
        attendees: Vec::new(),
        calendar_event_id: None,
        playbook: None,
    };
    store.save(&record).unwrap();
}

async fn run_regeneration(store: &SessionStore, session_id: Uuid, host: &MockHost) {
    finalize_inner(
        host,
        store,
        session_id,
        segments(),
        Vec::new(),
        Vec::new(),
        0,
        1,
        SessionParticipants {
            attendees: Vec::new(),
            calendar_event_id: None,
        },
        Vec::new(),
    )
    .await;
}

#[tokio::test]
async fn a_regeneration_that_fails_at_relay_send_leaves_the_previous_summary_intact_and_still_visible(
) {
    let store = temp_store("send-failure");
    let session_id = Uuid::new_v4();
    seed_completed_summary(&store, session_id);
    let host = MockHost::new(SendOutcome::Fail);

    run_regeneration(&store, session_id, &host).await;

    let record = store.load(&session_id.to_string()).unwrap().unwrap();
    assert_eq!(
        record.notes_markdown.as_deref(),
        Some("# Old summary\n\nthe original recap")
    );
    assert_eq!(record.notes_status.as_deref(), Some("completed"));
    assert_eq!(record.notes_root_message_id.as_deref(), Some("old-root"));
    assert!(record.notes_pending_root_message_id.is_none());
    assert!(record.chat.iter().any(|m| m.id == "old-reply"));
}

#[tokio::test]
async fn a_regeneration_that_succeeds_replaces_the_summary() {
    let store = temp_store("succeeds");
    let session_id = Uuid::new_v4();
    seed_completed_summary(&store, session_id);
    let host = MockHost::new(SendOutcome::Succeed);

    run_regeneration(&store, session_id, &host).await;

    let mid_flight = store.load(&session_id.to_string()).unwrap().unwrap();
    assert_eq!(
        mid_flight.notes_markdown.as_deref(),
        Some("# Old summary\n\nthe original recap")
    );
    assert_eq!(
        mid_flight.notes_root_message_id.as_deref(),
        Some("old-root")
    );
    let pending_id = host.pending_id();
    assert!(mid_flight
        .notes_pending_root_message_id
        .as_deref()
        .is_some_and(|id| id == pending_id));

    store
        .record_reply(
            &session_id.to_string(),
            Some(&pending_id),
            "new-reply",
            "# Fresh summary\n\nthe updated recap",
            "completed",
            None,
            None,
            "2026-01-01T00:10:00Z",
        )
        .unwrap();

    let settled = store.load(&session_id.to_string()).unwrap().unwrap();
    assert_eq!(
        settled.notes_markdown.as_deref(),
        Some("# Fresh summary\n\nthe updated recap")
    );
    assert_eq!(settled.notes_status.as_deref(), Some("completed"));
    assert_eq!(
        settled.notes_root_message_id.as_deref(),
        Some(pending_id.as_str())
    );
    assert!(settled.notes_pending_root_message_id.is_none());
    assert!(!settled.chat.iter().any(|m| m.id == "old-reply"));
    assert!(settled.chat.iter().any(|m| m.id == "new-reply"));
}

#[tokio::test]
async fn a_cancelled_regeneration_preserves_the_old_summary() {
    let store = temp_store("cancelled");
    let session_id = Uuid::new_v4();
    seed_completed_summary(&store, session_id);
    let host = MockHost::new(SendOutcome::Succeed);

    run_regeneration(&store, session_id, &host).await;
    let pending_id = host.pending_id();

    store
        .record_reply(
            &session_id.to_string(),
            Some(&pending_id),
            "cancelled-reply",
            "",
            "cancelled",
            None,
            None,
            "2026-01-01T00:10:00Z",
        )
        .unwrap();

    let record = store.load(&session_id.to_string()).unwrap().unwrap();
    assert_eq!(
        record.notes_markdown.as_deref(),
        Some("# Old summary\n\nthe original recap")
    );
    assert_eq!(record.notes_status.as_deref(), Some("completed"));
    assert_eq!(record.notes_root_message_id.as_deref(), Some("old-root"));
    assert!(record.notes_pending_root_message_id.is_none());
    assert!(record.chat.iter().any(|m| m.id == "old-reply"));
    assert!(!record.chat.iter().any(|m| m.id == "cancelled-reply"));
}

#[tokio::test]
async fn a_local_model_regeneration_persists_and_announces_the_pending_attempt_before_generation_starts(
) {
    let store = temp_store("local-pending-before-generation");
    let session_id = Uuid::new_v4();
    seed_completed_summary(&store, session_id);
    let host = MockHost::new_local(SendOutcome::Succeed);

    run_regeneration(&store, session_id, &host).await;

    let pending_id = host.pending_id();
    assert_eq!(
        host.pending_id_seen_by_local_generation(),
        Some(pending_id.clone()),
        "local generation should already see the persisted pending id once it starts",
    );

    let mid_flight = store.load(&session_id.to_string()).unwrap().unwrap();
    assert!(mid_flight
        .notes_pending_root_message_id
        .as_deref()
        .is_some_and(|id| id == pending_id));

    store
        .record_reply(
            &session_id.to_string(),
            Some(&pending_id),
            "new-local-reply",
            "# Fresh local summary\n\nthe updated recap",
            "completed",
            None,
            None,
            "2026-01-01T00:10:00Z",
        )
        .unwrap();

    let settled = store.load(&session_id.to_string()).unwrap().unwrap();
    assert_eq!(
        settled.notes_root_message_id.as_deref(),
        Some(pending_id.as_str())
    );
    assert!(settled.notes_pending_root_message_id.is_none());
}

#[tokio::test]
async fn a_failed_local_model_regeneration_leaves_the_previous_summary_intact_and_still_visible() {
    let store = temp_store("local-send-failure");
    let session_id = Uuid::new_v4();
    seed_completed_summary(&store, session_id);
    let host = MockHost::new_local(SendOutcome::Fail);

    run_regeneration(&store, session_id, &host).await;

    assert!(!host.pending_id().is_empty());
    let record = store.load(&session_id.to_string()).unwrap().unwrap();
    assert_eq!(
        record.notes_markdown.as_deref(),
        Some("# Old summary\n\nthe original recap")
    );
    assert_eq!(record.notes_status.as_deref(), Some("completed"));
    assert_eq!(record.notes_root_message_id.as_deref(), Some("old-root"));
    assert!(record.notes_pending_root_message_id.is_none());
    assert!(record.chat.iter().any(|m| m.id == "old-reply"));
}

#[tokio::test]
async fn the_local_branch_and_the_relay_branch_emit_the_same_notes_pending_event() {
    let relay_store = temp_store("parity-relay");
    let relay_session_id = Uuid::new_v4();
    seed_completed_summary(&relay_store, relay_session_id);
    let relay_host = MockHost::new(SendOutcome::Succeed);
    run_regeneration(&relay_store, relay_session_id, &relay_host).await;

    let local_store = temp_store("parity-local");
    let local_session_id = Uuid::new_v4();
    seed_completed_summary(&local_store, local_session_id);
    let local_host = MockHost::new_local(SendOutcome::Succeed);
    run_regeneration(&local_store, local_session_id, &local_host).await;

    let relay_pending = relay_store
        .load(&relay_session_id.to_string())
        .unwrap()
        .unwrap()
        .notes_pending_root_message_id;
    let local_pending = local_store
        .load(&local_session_id.to_string())
        .unwrap()
        .unwrap()
        .notes_pending_root_message_id;

    assert_eq!(relay_pending, Some(relay_host.pending_id()));
    assert_eq!(local_pending, Some(local_host.pending_id()));
}
