use super::*;

#[test]
fn completed_reply_matching_notes_root_settles_the_notes_turn() {
    assert!(is_notes_turn_settled(
        "completed",
        Some("notes-root"),
        None,
        Some("notes-root")
    ));
}

#[test]
fn message_status_reply_matching_notes_root_settles_the_notes_turn() {
    assert!(is_notes_turn_settled(
        "message",
        Some("notes-root"),
        None,
        Some("notes-root")
    ));
}

#[test]
fn message_status_reply_with_mismatched_parent_does_not_settle_the_notes_turn() {
    assert!(!is_notes_turn_settled(
        "message",
        Some("notes-root"),
        None,
        Some("chat-msg-2")
    ));
}

#[test]
fn errored_reply_matching_notes_root_still_settles_the_notes_turn() {
    assert!(is_notes_turn_settled(
        "errored",
        Some("notes-root"),
        None,
        Some("notes-root")
    ));
}

#[test]
fn typing_status_never_settles_the_notes_turn() {
    assert!(!is_notes_turn_settled(
        "typing",
        Some("notes-root"),
        None,
        Some("notes-root")
    ));
}

#[test]
fn completed_reply_with_mismatched_parent_does_not_settle_the_notes_turn() {
    assert!(!is_notes_turn_settled(
        "completed",
        Some("notes-root"),
        None,
        Some("chat-msg-2")
    ));
}

#[test]
fn missing_notes_root_never_settles_the_notes_turn() {
    assert!(!is_notes_turn_settled(
        "completed",
        None,
        None,
        Some("notes-root")
    ));
}

#[test]
fn missing_parent_never_settles_the_notes_turn() {
    assert!(!is_notes_turn_settled(
        "completed",
        Some("notes-root"),
        None,
        None
    ));
}

#[test]
fn completed_reply_matching_a_pending_regeneration_id_settles_the_notes_turn_even_though_the_committed_root_differs(
) {
    assert!(is_notes_turn_settled(
        "completed",
        Some("old-root"),
        Some("pending-root"),
        Some("pending-root")
    ));
}

#[test]
fn errored_reply_matching_a_pending_regeneration_id_settles_the_notes_turn() {
    assert!(is_notes_turn_settled(
        "errored",
        Some("old-root"),
        Some("pending-root"),
        Some("pending-root")
    ));
}
