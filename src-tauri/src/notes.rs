use std::sync::Arc;

use serde_json::json;
use sw_notes::finalize::{NotesEvent, NotesHost, NotesSettings, SessionParticipants};
use sw_notes::SessionStore;
use sw_notes::{accumulate::TranscriptSegment, finalize::finalize_inner};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::chat_reply_listener::PendingReplies;
use crate::relay::RelayClient;
use crate::relay_settings::RelaySettingsStore;

pub(crate) fn apply_auto_title(app: &AppHandle, session_id: &str) {
    let Some(store) = app.try_state::<Arc<SessionStore>>() else {
        return;
    };
    match store.apply_auto_title(session_id) {
        Ok(true) => {
            let _ = app.emit("session-updated", json!({ "session_id": session_id }));
        }
        Ok(false) => {}
        Err(err) => eprintln!("[auto-titles] failed for session {session_id}: {err}"),
    }
}

struct AppNotesHost {
    app: AppHandle,
    relay: Arc<RwLock<RelayClient>>,
    settings: Arc<RelaySettingsStore>,
}

impl NotesHost for AppNotesHost {
    async fn has_callback(&self) -> bool {
        self.relay.read().await.has_callback()
    }

    async fn send_summary(
        &self,
        session: Uuid,
        message: String,
        user_message_id: String,
        task_id: Uuid,
    ) -> Result<(), String> {
        if crate::state::local_llm::prefers_local(&self.app)
            && !crate::state::local_llm::local_ready(&self.app)
        {
            return Err("Your local model isn't ready yet.".to_string());
        }
        let settings_snapshot = self.settings.snapshot();
        let guard = self.relay.read().await;
        guard
            .send_session_chat(
                &settings_snapshot,
                session,
                message,
                Some(user_message_id),
                task_id,
            )
            .await
            .map_err(|e| e.to_string())
    }

    async fn summarize_local(
        &self,
        session: Uuid,
        message: String,
        user_message_id: String,
    ) -> Result<(), String> {
        let markdown = crate::state::local_llm::generate_reply(&self.app, None, &message).await?;
        let task_id = Uuid::new_v4().to_string();
        let created_at = chrono::Utc::now().to_rfc3339();
        if let Some(store) = self.app.try_state::<Arc<SessionStore>>() {
            store
                .record_reply(
                    &session.to_string(),
                    Some(&user_message_id),
                    &task_id,
                    &markdown,
                    "completed",
                    None,
                    None,
                    &created_at,
                )
                .map_err(|e| e.to_string())?;
        }
        apply_auto_title(&self.app, &session.to_string());
        let _ = self.app.emit(
            "chat-message-created",
            json!({
                "id": task_id,
                "session_id": session.to_string(),
                "role": "assistant",
                "content": markdown,
                "status": "completed",
                "parent_message_id": user_message_id,
                "created_at": created_at,
                "updated_at": created_at,
                "finalized_at": created_at,
            }),
        );
        Ok(())
    }

    fn register_pending(&self, task_id: Uuid, session_id: &str) {
        if let Some(pending) = self.app.try_state::<Arc<PendingReplies>>() {
            pending.register(task_id.to_string(), session_id.to_string());
        }
    }

    fn settings(&self) -> NotesSettings {
        let settings = self.settings.snapshot();
        let has_relay = settings.has_relay();
        let local_ready = crate::state::local_llm::local_ready(&self.app);
        let prefers_local = crate::state::local_llm::prefers_local(&self.app);
        NotesSettings {
            has_relay,
            pairing_blocked: if !settings.paired_verified {
                Some(
                    "Your assistant hasn't approved this device yet. Open Connection settings to finish pairing, then record again."
                        .to_string(),
                )
            } else {
                None
            },
            use_local_summary: local_ready && (prefers_local || !has_relay),
        }
    }

    fn emit(&self, event: NotesEvent) {
        match event {
            NotesEvent::SessionFinalized { session_id } => {
                let _ = self
                    .app
                    .emit("session-finalized", json!({ "session_id": session_id }));
            }
            NotesEvent::NotesPending {
                session_id,
                user_message_id,
            } => {
                let _ = self.app.emit(
                    "notes-pending",
                    json!({ "session_id": session_id, "user_message_id": user_message_id }),
                );
            }
            NotesEvent::NotesError {
                session_id,
                message,
            } => {
                let _ = self.app.emit(
                    "notes-error",
                    json!({ "session_id": session_id, "message": message }),
                );
            }
            NotesEvent::OpenSettings => {
                let _ = self.app.emit("open-settings", json!({}));
            }
        }
    }
}

pub fn finalize_session(
    app: AppHandle,
    relay_session: Uuid,
    segments: Vec<TranscriptSegment>,
    started_at_ms: u64,
    ended_at_ms: u64,
) {
    if segments.is_empty() {
        return;
    }
    let spawn = std::thread::Builder::new()
        .name("sw-notes-finalize".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    let _ = app.emit(
                        "notes-error",
                        json!({ "message": format!("notes runtime: {err}") }),
                    );
                    return;
                }
            };

            if let Err(err) = crate::ensure_session_storage(&app) {
                let _ = app.emit(
                    "notes-error",
                    json!({ "message": format!("session storage: {err}") }),
                );
                return;
            }

            let store = app.state::<Arc<SessionStore>>().inner().clone();
            let host = AppNotesHost {
                app: app.clone(),
                relay: app.state::<Arc<RwLock<RelayClient>>>().inner().clone(),
                settings: app.state::<Arc<RelaySettingsStore>>().inner().clone(),
            };
            rt.block_on(finalize_inner(
                &host,
                &store,
                relay_session,
                segments,
                Vec::new(),
                started_at_ms,
                ended_at_ms,
                SessionParticipants {
                    attendees: Vec::new(),
                    calendar_event_id: None,
                },
                Vec::new(),
            ));
        });
    if spawn.is_err() {
        eprintln!("[notes] failed to spawn finalize thread");
    }
}
