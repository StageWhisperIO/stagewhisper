use serde_json::Value;

use super::routing::ReplyDisposition;
use super::sink::ReplySink;

pub(super) fn route_stream_chunk(
    sink: &dyn ReplySink,
    task_id: &str,
    session_id: &str,
    chunk: Option<&Value>,
) -> ReplyDisposition {
    if let Some(expected_session) = sink.task_session(task_id) {
        if expected_session != session_id {
            eprintln!(
                "[reply-router] rejecting stream chunk with session mismatch task_id={task_id}"
            );
            return ReplyDisposition::SessionMismatch;
        }
    }

    let delta = chunk
        .filter(|chunk| chunk.get("type").and_then(Value::as_str) == Some("text-delta"))
        .and_then(|chunk| chunk.get("delta").and_then(Value::as_str))
        .unwrap_or_default();
    if delta.is_empty() {
        return ReplyDisposition::Accepted;
    }

    if sink.route_turn_text_delta(task_id, delta) {
        sink.touch_pending(task_id);
        return ReplyDisposition::Accepted;
    }

    if sink.turn_is_retired(task_id) {
        eprintln!(
            "[reply-router] dropping a stream chunk that arrived after its turn closed task_id={task_id}"
        );
        return ReplyDisposition::AlreadyFinalized;
    }

    ReplyDisposition::Accepted
}
