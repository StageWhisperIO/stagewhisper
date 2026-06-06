use std::time::Duration;

use uuid::Uuid;

use crate::{accumulate::TranscriptSegment, build_summary_message, SessionRecord, SessionStore};

const CALLBACK_WAIT_ATTEMPTS: usize = 30;
const CALLBACK_WAIT_INTERVAL: Duration = Duration::from_millis(100);

pub struct NotesSettings {
    pub has_relay: bool,
    pub pairing_blocked: Option<String>,
}

pub enum NotesEvent {
    SessionFinalized {
        session_id: String,
    },
    NotesPending {
        session_id: String,
        user_message_id: String,
    },
    NotesError {
        session_id: String,
        message: String,
    },
    OpenSettings,
}

pub trait NotesHost {
    #[allow(async_fn_in_trait)]
    async fn has_callback(&self) -> bool;
    #[allow(async_fn_in_trait)]
    async fn send_summary(
        &self,
        session: Uuid,
        message: String,
        user_message_id: String,
        task_id: Uuid,
    ) -> Result<(), String>;
    fn register_pending(&self, task_id: Uuid, session_id: &str);
    fn settings(&self) -> NotesSettings;
    fn emit(&self, event: NotesEvent);
}

fn ms_to_rfc3339(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339()
}

pub struct SessionParticipants {
    pub attendees: Vec<String>,
    pub calendar_event_id: Option<String>,
}

pub async fn finalize_inner(
    host: &impl NotesHost,
    store: &SessionStore,
    relay_session: Uuid,
    segments: Vec<TranscriptSegment>,
    started_at_ms: u64,
    ended_at_ms: u64,
    participants: SessionParticipants,
) {
    let mut record = SessionRecord {
        session_id: relay_session.to_string(),
        relay_session_id: relay_session.to_string(),
        started_at: ms_to_rfc3339(started_at_ms),
        ended_at: ms_to_rfc3339(ended_at_ms),
        title: None,
        segments,
        notes_markdown: None,
        notes_status: Some("pending".to_string()),
        notes_error: None,
        notes_root_message_id: None,
        chat: vec![],
        attendees: participants.attendees,
        calendar_event_id: participants.calendar_event_id,
    };

    if let Err(err) = store.save(&record) {
        let _ = host.emit(NotesEvent::NotesError {
            session_id: relay_session.to_string(),
            message: format!("persist failed: {err}"),
        });

        return;
    }

    let _ = host.emit(NotesEvent::SessionFinalized {
        session_id: relay_session.to_string(),
    });

    let settings = host.settings();

    if !settings.has_relay {
        finalize_error(
            host,
            store,
            &mut record,
            relay_session.to_string(),
            "Relay not configured",
        );
        return;
    }
    if let Some(msg) = settings.pairing_blocked {
        finalize_error(host, store, &mut record, relay_session.to_string(), &msg);
        return;
    }

    let mut callback_ready = false;
    for _ in 0..CALLBACK_WAIT_ATTEMPTS {
        if host.has_callback().await {
            callback_ready = true;
            break;
        }
        tokio::time::sleep(CALLBACK_WAIT_INTERVAL).await;
    }
    if !callback_ready {
        finalize_error(
            host,
            store,
            &mut record,
            relay_session.to_string(),
            "Reply listener not ready",
        );
        return;
    }

    let user_message_id = Uuid::new_v4().to_string();
    record.notes_root_message_id = Some(user_message_id.clone());
    if let Err(err) = store.save(&record) {
        finalize_error(
            host,
            store,
            &mut record,
            relay_session.to_string(),
            &format!("persist failed: {err}"),
        );
        return;
    }

    let task_id = Uuid::new_v4();
    host.register_pending(task_id, &relay_session.to_string());

    let message = build_summary_message(&record.segments);
    match host
        .send_summary(relay_session, message, user_message_id.clone(), task_id)
        .await
    {
        Ok(_) => host.emit(NotesEvent::NotesPending {
            session_id: relay_session.to_string(),
            user_message_id: user_message_id,
        }),
        Err(err) => finalize_error(host, store, &mut record, relay_session.to_string(), &err),
    }
}

fn finalize_error(
    host: &impl NotesHost,
    store: &SessionStore,
    record: &mut SessionRecord,
    session_id: String,
    message: &str,
) {
    record.notes_status = Some("errored".to_string());
    record.notes_error = Some(message.to_string());
    let _ = store.save(record);
    let _ = host.emit(NotesEvent::NotesError {
        session_id,
        message: message.to_string(),
    });
}
