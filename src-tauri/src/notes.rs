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

    fn register_pending(&self, task_id: Uuid, session_id: &str) {
        if let Some(pending) = self.app.try_state::<Arc<PendingReplies>>() {
            pending.register(task_id.to_string(), session_id.to_string());
        }
    }

    fn settings(&self) -> NotesSettings {
        let settings = self.settings.snapshot();
        NotesSettings {
            has_relay: settings.has_relay(),
            pairing_blocked: if !settings.paired_verified {
                Some(
                    "Your assistant hasn't approved this device yet. Open Connection settings to finish pairing, then record again."
                        .to_string(),
                )
            } else {
                None
            },
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
                started_at_ms,
                ended_at_ms,
                SessionParticipants {
                    attendees: Vec::new(),
                    calendar_event_id: None,
                },
            ));
        });
    if spawn.is_err() {
        eprintln!("[notes] failed to spawn finalize thread");
    }
}
