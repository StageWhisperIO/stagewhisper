use serde::Deserialize;
use serde_json::{json, Value};

use super::pending::ReserveResult;
use super::probe::ProbeOutcome;
use super::sink::ReplySink;
use super::ChatMessagePayload;

#[derive(Debug, Deserialize)]
pub struct ReplyBody {
    pub task_id: Option<String>,
    pub session_id: String,
    pub user_message_id: Option<String>,
    pub status: String,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub reply_text: Option<String>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub chunk: Option<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropReason {
    SessionEnded,
    SessionMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyDisposition {
    InvalidTaskId,
    TaskIdMismatch,
    SessionIdRequired,
    InvalidStatus,
    ProbeResolved,
    Dropped(DropReason),
    SessionMismatch,
    EmptyMessageIgnored,
    AlreadyFinalized,
    UnregisteredTask,
    PersistFailed,
    Accepted,
}

enum TerminalEmit {
    Created,
    Errored(Value),
}

pub fn route_reply(sink: &dyn ReplySink, task_id: &str, body: ReplyBody) -> ReplyDisposition {
    if uuid::Uuid::parse_str(task_id).is_err() {
        return ReplyDisposition::InvalidTaskId;
    }

    if let Some(body_task_id) = body.task_id.as_ref() {
        if body_task_id != task_id {
            return ReplyDisposition::TaskIdMismatch;
        }
    }

    if body.session_id.trim().is_empty() {
        return ReplyDisposition::SessionIdRequired;
    }
    let known_statuses = [
        "completed",
        "errored",
        "silent",
        "typing",
        "tool_call",
        "message",
        "stream",
    ];
    if !known_statuses.contains(&body.status.as_str()) {
        return ReplyDisposition::InvalidStatus;
    }

    let effective_task_id = body.task_id.clone().unwrap_or_else(|| task_id.to_string());

    if matches!(body.status.as_str(), "completed" | "errored" | "message") {
        let outcome = ProbeOutcome {
            status: body.status.clone(),
            reply_text: body.reply_text.clone(),
            error_message: body.error_message.clone(),
        };
        if sink.resolve_probe(&effective_task_id, outcome) {
            return ReplyDisposition::ProbeResolved;
        }
    }

    let current_session = sink.current_session_id();
    let session_matches = current_session
        .as_ref()
        .map(|s| s == &body.session_id)
        .unwrap_or(false);

    if !session_matches && !sink.session_known(&body.session_id) {
        let reason = if current_session.is_none() {
            DropReason::SessionEnded
        } else {
            DropReason::SessionMismatch
        };
        let reason_label = match reason {
            DropReason::SessionEnded => "session_ended",
            DropReason::SessionMismatch => "session_mismatch",
        };
        eprintln!(
            "[reply-router] dropping late callback task_id={task_id} status={} reason={reason_label}",
            body.status,
        );
        return ReplyDisposition::Dropped(reason);
    }

    if matches!(body.status.as_str(), "typing" | "tool_call") {
        if sink.validate_task_session(&effective_task_id, &body.session_id)
            == ReserveResult::SessionMismatch
        {
            eprintln!(
                "[reply-router] rejecting {} callback with session mismatch task_id={effective_task_id}",
                body.status,
            );
            return ReplyDisposition::SessionMismatch;
        }
        sink.touch_pending(&effective_task_id);
        return ReplyDisposition::Accepted;
    }

    if body.status == "stream" {
        return super::stream::route_stream_chunk(
            sink,
            &effective_task_id,
            &body.session_id,
            body.chunk.as_ref(),
        );
    }

    if body.status == "message" {
        let text = body.reply_text.clone().unwrap_or_default();
        if text.trim().is_empty() {
            return ReplyDisposition::EmptyMessageIgnored;
        }

        if let Some(expected_session) = sink.task_session(&effective_task_id) {
            if expected_session != body.session_id {
                eprintln!(
                    "[reply-router] rejecting message callback with session mismatch task_id={effective_task_id}",
                );
                return ReplyDisposition::SessionMismatch;
            }
        }

        if sink.route_turn_part(&effective_task_id, &text) {
            sink.touch_pending(&effective_task_id);
            return ReplyDisposition::Accepted;
        }

        if sink.turn_is_retired(&effective_task_id) {
            eprintln!(
                "[reply-router] dropping a part that arrived after its turn closed task_id={effective_task_id}",
            );
            return ReplyDisposition::AlreadyFinalized;
        }

        match sink.reserve_terminal(&effective_task_id, &body.session_id) {
            ReserveResult::Reserved => {}
            ReserveResult::Duplicate => return ReplyDisposition::AlreadyFinalized,
            ReserveResult::Unregistered => {
                eprintln!(
                    "[reply-router] rejecting message callback for unknown or evicted task_id={effective_task_id}",
                );
                return ReplyDisposition::UnregisteredTask;
            }
            ReserveResult::SessionMismatch => {
                eprintln!(
                    "[reply-router] rejecting message callback with session mismatch task_id={effective_task_id}",
                );
                return ReplyDisposition::SessionMismatch;
            }
        }

        let message_id = body
            .message_id
            .clone()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                effective_task_id.hash(&mut hasher);
                body.session_id.hash(&mut hasher);
                text.hash(&mut hasher);
                format!("{effective_task_id}:msg:{:016x}", hasher.finish())
            });
        let now = chrono::Utc::now().to_rfc3339();
        let payload = ChatMessagePayload {
            id: message_id,
            session_id: body.session_id.clone(),
            role: "assistant".to_string(),
            content: text,
            status: "completed".to_string(),
            tool_calls: None,
            tool_result_payload: body.model.as_ref().map(|m| json!({ "model": m })),
            parent_message_id: body.user_message_id.clone(),
            suggestion_id: None,
            error_code: None,
            error_message: None,
            created_at: now.clone(),
            updated_at: now.clone(),
            finalized_at: Some(now),
        };
        if !sink.append_message(payload.clone()) {
            sink.release_terminal(&effective_task_id);
            return ReplyDisposition::PersistFailed;
        }
        sink.complete_terminal(&effective_task_id);
        sink.emit_created(&payload);
        return ReplyDisposition::Accepted;
    }

    let now = chrono::Utc::now().to_rfc3339();
    let is_terminal = matches!(body.status.as_str(), "completed" | "errored" | "silent");

    let to_persist: Option<(ChatMessagePayload, TerminalEmit)> = match body.status.as_str() {
        "completed" => {
            let reply_text = body.reply_text.clone().unwrap_or_default();
            let payload = ChatMessagePayload {
                id: effective_task_id.clone(),
                session_id: body.session_id.clone(),
                role: "assistant".to_string(),
                content: reply_text,
                status: "completed".to_string(),
                tool_calls: None,
                tool_result_payload: body.model.as_ref().map(|m| json!({ "model": m })),
                parent_message_id: body.user_message_id.clone(),
                suggestion_id: None,
                error_code: None,
                error_message: None,
                created_at: now.clone(),
                updated_at: now.clone(),
                finalized_at: Some(now.clone()),
            };
            Some((payload, TerminalEmit::Created))
        }
        "errored" => {
            let placeholder = ChatMessagePayload {
                id: effective_task_id.clone(),
                session_id: body.session_id.clone(),
                role: "assistant".to_string(),
                content: String::new(),
                status: "errored".to_string(),
                tool_calls: None,
                tool_result_payload: None,
                parent_message_id: body.user_message_id.clone(),
                suggestion_id: None,
                error_code: body.error_code.clone(),
                error_message: body.error_message.clone(),
                created_at: now.clone(),
                updated_at: now.clone(),
                finalized_at: Some(now.clone()),
            };
            let event = json!({
                "task_id": effective_task_id.clone(),
                "session_id": body.session_id,
                "user_message_id": body.user_message_id,
                "error_code": body.error_code,
                "error_message": body.error_message,
            });
            Some((placeholder, TerminalEmit::Errored(event)))
        }
        "silent" if body.user_message_id.is_some() => {
            let placeholder = ChatMessagePayload {
                id: effective_task_id.clone(),
                session_id: body.session_id.clone(),
                role: "assistant".to_string(),
                content: String::new(),
                status: "cancelled".to_string(),
                tool_calls: None,
                tool_result_payload: None,
                parent_message_id: body.user_message_id,
                suggestion_id: None,
                error_code: None,
                error_message: None,
                created_at: now.clone(),
                updated_at: now.clone(),
                finalized_at: Some(now),
            };
            Some((placeholder, TerminalEmit::Created))
        }
        _ => None,
    };

    if is_terminal {
        match sink.reserve_terminal(&effective_task_id, &body.session_id) {
            ReserveResult::Reserved => {}
            ReserveResult::Duplicate => return ReplyDisposition::AlreadyFinalized,
            ReserveResult::Unregistered => {
                eprintln!(
                    "[reply-router] rejecting terminal callback for unknown or evicted task_id={effective_task_id} status={}",
                    body.status,
                );
                return ReplyDisposition::UnregisteredTask;
            }
            ReserveResult::SessionMismatch => {
                eprintln!(
                    "[reply-router] rejecting terminal callback with session mismatch task_id={effective_task_id} status={}",
                    body.status,
                );
                return ReplyDisposition::SessionMismatch;
            }
        }
        if sink.forward_to_turn(
            &effective_task_id,
            &body.status,
            body.reply_text.as_deref(),
            body.error_code.as_deref(),
            body.error_message.as_deref(),
        ) {
            sink.complete_terminal(&effective_task_id);
            return ReplyDisposition::Accepted;
        }
    }

    if let Some((payload, _)) = to_persist.as_ref() {
        if !sink.append_message(payload.clone()) {
            if is_terminal {
                sink.release_terminal(&effective_task_id);
            }
            return ReplyDisposition::PersistFailed;
        }
    }

    if is_terminal {
        sink.complete_terminal(&effective_task_id);
    }

    if let Some((payload, emit)) = to_persist {
        match emit {
            TerminalEmit::Created => sink.emit_created(&payload),
            TerminalEmit::Errored(event) => {
                sink.emit_created(&payload);
                sink.emit_errored(&event);
            }
        }
    }

    ReplyDisposition::Accepted
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;
