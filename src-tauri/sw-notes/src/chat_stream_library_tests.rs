use super::*;
use serde_json::json;

fn wire(chunk: UiChunk) -> serde_json::Value {
    serde_json::to_value(&chunk).unwrap()
}

#[test]
fn step_boundaries_serialise_to_the_ai_sdk_step_chunks() {
    assert_eq!(wire(UiChunk::StartStep), json!({ "type": "start-step" }));
    assert_eq!(wire(UiChunk::FinishStep), json!({ "type": "finish-step" }));
}

#[test]
fn a_tool_input_start_omits_dynamic_unless_it_is_set() {
    assert_eq!(
        wire(UiChunk::ToolInputStart {
            tool_call_id: "call_1".to_string(),
            tool_name: "searchLibrary".to_string(),
            dynamic: None,
        }),
        json!({ "type": "tool-input-start", "toolCallId": "call_1", "toolName": "searchLibrary" })
    );
    assert_eq!(
        wire(UiChunk::ToolInputStart {
            tool_call_id: "call_2".to_string(),
            tool_name: "deleteFiles".to_string(),
            dynamic: Some(true),
        }),
        json!({
            "type": "tool-input-start",
            "toolCallId": "call_2",
            "toolName": "deleteFiles",
            "dynamic": true
        })
    );
}

#[test]
fn tool_input_chunks_use_the_ai_sdk_field_names() {
    assert_eq!(
        wire(UiChunk::ToolInputDelta {
            tool_call_id: "call_1".to_string(),
            input_text_delta: "{\"qu".to_string(),
        }),
        json!({ "type": "tool-input-delta", "toolCallId": "call_1", "inputTextDelta": "{\"qu" })
    );
    assert_eq!(
        wire(UiChunk::ToolInputAvailable {
            tool_call_id: "call_1".to_string(),
            tool_name: "searchLibrary".to_string(),
            input: json!({ "query": "renewal" }),
            dynamic: None,
        }),
        json!({
            "type": "tool-input-available",
            "toolCallId": "call_1",
            "toolName": "searchLibrary",
            "input": { "query": "renewal" }
        })
    );
}

#[test]
fn tool_results_use_the_ai_sdk_field_names() {
    assert_eq!(
        wire(UiChunk::ToolOutputAvailable {
            tool_call_id: "call_1".to_string(),
            output: json!({ "sessions": [] }),
        }),
        json!({ "type": "tool-output-available", "toolCallId": "call_1", "output": { "sessions": [] } })
    );
    assert_eq!(
        wire(UiChunk::ToolOutputError {
            tool_call_id: "call_1".to_string(),
            error_text: "unknown tool".to_string(),
        }),
        json!({ "type": "tool-output-error", "toolCallId": "call_1", "errorText": "unknown tool" })
    );
}

#[test]
fn a_library_notice_is_a_data_part_that_carries_only_its_kind() {
    assert_eq!(
        wire(UiChunk::LibraryNotice {
            data: LibraryNoticeData {
                kind: "signals_not_live".to_string(),
            },
        }),
        json!({ "type": "data-library-notice", "data": { "kind": "signals_not_live" } })
    );
}

#[test]
fn the_existing_chunks_keep_their_wire_names() {
    assert_eq!(
        wire(UiChunk::Finish {
            finish_reason: "stop".to_string(),
        }),
        json!({ "type": "finish", "finishReason": "stop" })
    );
    assert_eq!(
        wire(UiChunk::Activity {
            data: ActivityData {
                label: "Thinking".to_string(),
            },
            transient: true,
        }),
        json!({ "type": "data-activity", "data": { "label": "Thinking" }, "transient": true })
    );
}
