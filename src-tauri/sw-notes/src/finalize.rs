use uuid::Uuid;

use crate::{
    accumulate::TranscriptSegment, blocks::BlockSummary, build_local_summary_message,
    build_relay_summary_message, ChatMsg, InsightNote, SessionRecord, SessionStore,
};

pub struct NotesSettings {
    pub has_relay: bool,
    pub pairing_blocked: Option<String>,
    pub use_local_summary: bool,
    pub use_hosted_summary: bool,
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
    async fn send_summary(
        &self,
        session: Uuid,
        message: String,
        user_message_id: String,
        task_id: Uuid,
    ) -> Result<(), String>;
    #[allow(async_fn_in_trait)]
    async fn summarize_local(
        &self,
        session: Uuid,
        message: String,
        user_message_id: String,
    ) -> Result<(), String>;
    #[allow(async_fn_in_trait)]
    async fn build_local_prompt(
        &self,
        segments: &[TranscriptSegment],
        blocks: &[BlockSummary],
        screen_context: Option<&str>,
        playbook: Option<&str>,
    ) -> String {
        build_local_summary_message(
            segments,
            blocks,
            screen_context,
            playbook,
            crate::LOCAL_PROMPT_BUDGET_CHARS,
        )
    }
    fn register_pending(&self, task_id: Uuid, session_id: &str);
    fn settings(&self) -> NotesSettings;
    fn screen_context(&self, _session_id: &str) -> Option<String> {
        None
    }
    fn playbook(&self) -> Option<String> {
        None
    }
    fn emit(&self, event: NotesEvent);
}

pub fn ms_to_rfc3339(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339()
}

#[derive(Debug, Clone)]
pub struct SessionParticipants {
    pub attendees: Vec<String>,
    pub calendar_event_id: Option<String>,
}

fn carry_over_chat(mut kept: Vec<ChatMsg>, incoming: Vec<ChatMsg>) -> Vec<ChatMsg> {
    for message in incoming {
        if !kept.iter().any(|existing| existing.id == message.id) {
            kept.push(message);
        }
    }
    kept
}

fn drop_previous_notes_reply(
    chat: Vec<ChatMsg>,
    previous_notes_root_message_id: Option<&str>,
) -> Vec<ChatMsg> {
    let Some(root) = previous_notes_root_message_id else {
        return chat;
    };
    chat.into_iter()
        .filter(|message| message.parent_message_id.as_deref() != Some(root))
        .collect()
}

fn begin_notes_attempt(
    record: &mut SessionRecord,
    has_existing_summary: bool,
    user_message_id: &str,
) {
    if has_existing_summary {
        record.notes_pending_root_message_id = Some(user_message_id.to_string());
    } else {
        record.notes_root_message_id = Some(user_message_id.to_string());
    }
}

pub async fn finalize_inner(
    host: &impl NotesHost,
    store: &SessionStore,
    relay_session: Uuid,
    segments: Vec<TranscriptSegment>,
    insights: Vec<InsightNote>,
    blocks: Vec<BlockSummary>,
    started_at_ms: u64,
    ended_at_ms: u64,
    participants: SessionParticipants,
    initial_chat: Vec<ChatMsg>,
) {
    let playbook = host.playbook();
    let previous = store.load(&relay_session.to_string()).ok().flatten();
    let has_existing_summary = previous
        .as_ref()
        .is_some_and(|record| record.notes_markdown.is_some());
    let previous_notes_root_message_id = previous
        .as_ref()
        .and_then(|record| record.notes_root_message_id.clone());
    let backend_session_id = store
        .live_load(&relay_session.to_string())
        .ok()
        .flatten()
        .and_then(|live| live.backend_session_id)
        .or_else(|| {
            previous
                .as_ref()
                .and_then(|record| record.backend_session_id.clone())
        });
    let mut record = SessionRecord {
        session_id: relay_session.to_string(),
        relay_session_id: relay_session.to_string(),
        backend_session_id,
        started_at: ms_to_rfc3339(started_at_ms),
        ended_at: ms_to_rfc3339(ended_at_ms),
        title: previous.as_ref().and_then(|record| record.title.clone()),
        title_is_auto: previous.as_ref().is_some_and(|record| record.title_is_auto),
        segments,
        insights,
        blocks,
        notes_markdown: if has_existing_summary {
            previous
                .as_ref()
                .and_then(|record| record.notes_markdown.clone())
        } else {
            None
        },
        notes_status: if has_existing_summary {
            previous
                .as_ref()
                .and_then(|record| record.notes_status.clone())
        } else {
            Some("pending".to_string())
        },
        notes_error: None,
        notes_root_message_id: if has_existing_summary {
            previous_notes_root_message_id.clone()
        } else {
            None
        },
        notes_pending_root_message_id: None,
        chat: carry_over_chat(
            if has_existing_summary {
                previous.map(|record| record.chat).unwrap_or_default()
            } else {
                drop_previous_notes_reply(
                    previous.map(|record| record.chat).unwrap_or_default(),
                    previous_notes_root_message_id.as_deref(),
                )
            },
            initial_chat,
        ),
        attendees: participants.attendees,
        calendar_event_id: participants.calendar_event_id,
        playbook: playbook.clone(),
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
    let screen_context = host.screen_context(&relay_session.to_string());

    if settings.use_local_summary {
        let user_message_id = Uuid::new_v4().to_string();
        begin_notes_attempt(&mut record, has_existing_summary, &user_message_id);
        if let Err(err) = store.save(&record) {
            finalize_error(
                host,
                store,
                &mut record,
                has_existing_summary,
                relay_session.to_string(),
                &format!("persist failed: {err}"),
            );
            return;
        }
        host.emit(NotesEvent::NotesPending {
            session_id: relay_session.to_string(),
            user_message_id: user_message_id.clone(),
        });
        let message = host
            .build_local_prompt(
                &record.segments,
                &record.blocks,
                screen_context.as_deref(),
                playbook.as_deref(),
            )
            .await;
        if let Err(err) = host
            .summarize_local(relay_session, message, user_message_id)
            .await
        {
            finalize_error(
                host,
                store,
                &mut record,
                has_existing_summary,
                relay_session.to_string(),
                &err,
            );
        }
        return;
    }

    if !settings.has_relay && !settings.use_hosted_summary {
        finalize_error(
            host,
            store,
            &mut record,
            has_existing_summary,
            relay_session.to_string(),
            "Relay not configured",
        );
        return;
    }
    if let Some(msg) = settings.pairing_blocked {
        finalize_error(
            host,
            store,
            &mut record,
            has_existing_summary,
            relay_session.to_string(),
            &msg,
        );
        return;
    }

    let user_message_id = Uuid::new_v4().to_string();
    begin_notes_attempt(&mut record, has_existing_summary, &user_message_id);
    if let Err(err) = store.save(&record) {
        finalize_error(
            host,
            store,
            &mut record,
            has_existing_summary,
            relay_session.to_string(),
            &format!("persist failed: {err}"),
        );
        return;
    }

    let task_id = Uuid::new_v4();
    host.register_pending(task_id, &relay_session.to_string());

    let message = build_relay_summary_message(
        &record.segments,
        &record.blocks,
        screen_context.as_deref(),
        playbook.as_deref(),
    );
    match host
        .send_summary(relay_session, message, user_message_id.clone(), task_id)
        .await
    {
        Ok(_) => host.emit(NotesEvent::NotesPending {
            session_id: relay_session.to_string(),
            user_message_id: user_message_id,
        }),
        Err(err) => finalize_error(
            host,
            store,
            &mut record,
            has_existing_summary,
            relay_session.to_string(),
            &err,
        ),
    }
}

fn finalize_error(
    host: &impl NotesHost,
    store: &SessionStore,
    record: &mut SessionRecord,
    has_existing_summary: bool,
    session_id: String,
    message: &str,
) {
    record.notes_pending_root_message_id = None;
    if !has_existing_summary {
        record.notes_status = Some("errored".to_string());
        record.notes_error = Some(message.to_string());
    }
    let _ = store.save(record);
    let _ = host.emit(NotesEvent::NotesError {
        session_id,
        message: message.to_string(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, content: &str) -> ChatMsg {
        ChatMsg {
            id: id.to_string(),
            role: "user".to_string(),
            content: content.to_string(),
            status: "completed".to_string(),
            parent_message_id: None,
            error_code: None,
            error_message: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn reply_to(id: &str, content: &str, parent: &str) -> ChatMsg {
        ChatMsg {
            role: "assistant".to_string(),
            parent_message_id: Some(parent.to_string()),
            ..msg(id, content)
        }
    }

    #[test]
    fn regenerating_a_summary_keeps_the_chat_already_on_disk() {
        let kept = carry_over_chat(vec![msg("a", "first")], Vec::new());
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "a");
    }

    #[test]
    fn chat_captured_during_the_call_is_appended_without_duplicating_what_is_stored() {
        let kept = carry_over_chat(
            vec![msg("a", "first"), msg("b", "second")],
            vec![msg("b", "second"), msg("c", "third")],
        );
        let ids: Vec<&str> = kept.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn regenerating_a_completed_summary_replaces_it() {
        let chat = vec![
            msg("user-question", "what did we agree on?"),
            reply_to("old-summary-reply", "the old summary", "old-root"),
            reply_to("chat-reply", "we agreed on x", "user-question"),
        ];

        let kept = drop_previous_notes_reply(chat, Some("old-root"));

        let ids: Vec<&str> = kept.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["user-question", "chat-reply"]);
    }

    #[test]
    fn a_session_finalizing_for_the_first_time_has_nothing_to_drop() {
        let chat = vec![msg("user-question", "what did we agree on?")];

        let kept = drop_previous_notes_reply(chat.clone(), None);

        assert_eq!(kept, chat);
    }
}

#[cfg(test)]
#[path = "finalize_regeneration_tests.rs"]
mod regeneration_tests;
