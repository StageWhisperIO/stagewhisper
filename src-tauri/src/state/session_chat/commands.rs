use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::Manager;

use sw_notes::{ChunkSink, TurnHandle, TurnOutcome, TurnRegistry, UiChunk};

use super::{
    channel_sink, local_producer, relay_producer, StorePersistence, LOCAL_IDENTITY_BINDING,
};

fn ensure_route_available(relay_available: bool, use_local: bool) -> Result<(), String> {
    if !relay_available && !use_local {
        return Err("Relay not configured".to_string());
    }
    Ok(())
}

enum ChatRoute {
    Local,
    Relay {
        relay_session: uuid::Uuid,
        task_id: uuid::Uuid,
    },
}

fn claim_turn_and_record_user_message<S: ChunkSink>(
    registry: &TurnRegistry,
    store: &sw_notes::SessionStore,
    session_id: &str,
    turn_id: &str,
    user_message_id: &str,
    text: &str,
    sink: S,
) -> Result<Arc<TurnHandle>, String> {
    let handle = registry
        .register(
            turn_id.to_string(),
            session_id.to_string(),
            user_message_id.to_string(),
            turn_id.to_string(),
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            sink,
        )
        .ok_or_else(|| "This message was already sent.".to_string())?;

    let now = chrono::Utc::now().to_rfc3339();
    let user_msg = sw_notes::ChatMsg {
        id: user_message_id.to_string(),
        role: "user".to_string(),
        content: text.to_string(),
        status: "pending".to_string(),
        parent_message_id: None,
        error_code: None,
        error_message: None,
        created_at: now,
    };
    let refusal = match store.append_chat(session_id, user_msg) {
        Ok(sw_notes::ChatAppendOutcome::Inserted)
        | Ok(sw_notes::ChatAppendOutcome::IdenticalDuplicate) => None,
        Ok(sw_notes::ChatAppendOutcome::ConflictingDuplicate)
        | Ok(sw_notes::ChatAppendOutcome::MissingSession) => {
            Some("Your message could not be saved.".to_string())
        }
        Err(err) => Some(err.to_string()),
    };

    if let Some(error_text) = refusal {
        let persistence = StorePersistence { store };
        let result = registry.terminate(
            turn_id,
            TurnOutcome::Failure("Your message could not be saved.".to_string()),
            Some(LOCAL_IDENTITY_BINDING),
            &persistence,
        );
        super::log_non_terminated_result("claim_turn_and_record_user_message", turn_id, &result);
        return Err(error_text);
    }

    Ok(handle)
}

#[tauri::command]
pub async fn stream_session_chat_message(
    app: tauri::AppHandle,
    session_id: String,
    text: String,
    turn_id: String,
    on_chunk: Channel<UiChunk>,
) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("message is empty".to_string());
    }
    let store = crate::session_store(&app)?;
    let settings = app
        .state::<Arc<crate::relay_settings::RelaySettingsStore>>()
        .inner()
        .clone()
        .snapshot();
    let relay_available = settings.has_relay();
    let use_local = crate::state::local_llm::local_ready(&app)
        && (crate::state::local_llm::prefers_local(&app) || !relay_available);
    ensure_route_available(relay_available, use_local)?;

    let record = store
        .load(&session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "session not found".to_string())?;

    let route = if use_local {
        ChatRoute::Local
    } else {
        let (relay_session, task_id) = relay_producer::parse_relay_target(&record, &turn_id)?;
        ChatRoute::Relay {
            relay_session,
            task_id,
        }
    };

    let user_message_id = uuid::Uuid::new_v4().to_string();
    let registry = app.state::<Arc<TurnRegistry>>().inner().clone();
    let handle = claim_turn_and_record_user_message(
        &registry,
        &store,
        &session_id,
        &turn_id,
        &user_message_id,
        &text,
        channel_sink(on_chunk),
    )?;

    match route {
        ChatRoute::Local => {
            let _ = store.update_chat_status(&session_id, &user_message_id, "completed", None);
            local_producer::spawn(
                app.clone(),
                registry,
                handle,
                session_id,
                turn_id,
                user_message_id,
                text,
            );
            Ok(())
        }
        ChatRoute::Relay {
            relay_session,
            task_id,
        } => {
            relay_producer::send_relay_turn(
                &app,
                &store,
                &registry,
                handle,
                &settings,
                &record,
                relay_session,
                task_id,
                &session_id,
                &turn_id,
                &user_message_id,
                &text,
            )
            .await
        }
    }
}

#[tauri::command]
pub fn cancel_session_chat_turn(app: tauri::AppHandle, turn_id: String) -> Result<(), String> {
    let registry = app.state::<Arc<TurnRegistry>>().inner().clone();
    registry.cancel(&turn_id);
    Ok(())
}

#[tauri::command]
pub fn resume_session_chat_turn(
    app: tauri::AppHandle,
    session_id: String,
    on_chunk: Channel<UiChunk>,
) -> Result<bool, String> {
    let registry = app.state::<Arc<TurnRegistry>>().inner().clone();
    Ok(registry
        .resume(
            &session_id,
            Some(LOCAL_IDENTITY_BINDING),
            channel_sink(on_chunk),
        )
        .is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_assistant_available_errors_without_registering_a_turn() {
        let registry = TurnRegistry::default();

        let result = ensure_route_available(false, false);

        assert_eq!(result, Err("Relay not configured".to_string()));
        assert!(!registry.contains("turn-never-registered"));
    }

    #[test]
    fn relay_only_route_is_available() {
        assert!(ensure_route_available(true, false).is_ok());
    }

    #[test]
    fn local_only_route_is_available() {
        assert!(ensure_route_available(false, true).is_ok());
    }

    fn temp_store(tag: &str) -> sw_notes::SessionStore {
        let dir = std::env::temp_dir().join(format!(
            "sw_free_session_chat_commands_{tag}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        sw_notes::SessionStore::new(dir, [0x42; 32]).expect("temp session store")
    }

    fn sample_record(session_id: &str) -> sw_notes::SessionRecord {
        sw_notes::SessionRecord {
            session_id: session_id.to_string(),
            relay_session_id: uuid::Uuid::new_v4().to_string(),
            backend_session_id: None,
            started_at: "2026-08-17T10:00:00Z".to_string(),
            ended_at: "2026-08-17T10:30:00Z".to_string(),
            title: None,
            title_is_auto: false,
            segments: vec![],
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
        }
    }

    #[test]
    fn a_duplicate_turn_id_is_rejected_and_leaves_no_user_message_in_the_store() {
        let registry = TurnRegistry::new();
        let store = temp_store("duplicate_turn");
        let session_id = "sess-duplicate";
        store.save(&sample_record(session_id)).unwrap();

        let first = claim_turn_and_record_user_message(
            &registry,
            &store,
            session_id,
            "turn-reused",
            "user-msg-1",
            "first message",
            |_chunk: UiChunk| {},
        );
        assert!(first.is_ok());

        let second = claim_turn_and_record_user_message(
            &registry,
            &store,
            session_id,
            "turn-reused",
            "user-msg-2",
            "second message with the same turn id",
            |_chunk: UiChunk| {},
        );

        assert_eq!(
            second.err(),
            Some("This message was already sent.".to_string())
        );
        let record = store.load(session_id).unwrap().unwrap();
        assert_eq!(record.chat.len(), 1);
        assert_eq!(record.chat[0].id, "user-msg-1");
        assert!(!record.chat.iter().any(|m| m.id == "user-msg-2"));
    }

    #[test]
    fn appending_the_user_message_when_the_session_is_missing_refuses_the_turn_cleanly() {
        let registry = TurnRegistry::new();
        let store = temp_store("missing_session_user_append");
        let session_id = "sess-never-saved";

        let result = claim_turn_and_record_user_message(
            &registry,
            &store,
            session_id,
            "turn-missing-session",
            "user-msg-1",
            "hello, does anyone exist?",
            |_chunk: UiChunk| {},
        );

        assert_eq!(
            result.err(),
            Some("Your message could not be saved.".to_string())
        );
        assert!(!registry.contains("turn-missing-session"));
        assert!(store.load(session_id).unwrap().is_none());
    }
}
