use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ActivityData {
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum UiChunk {
    #[serde(rename_all = "camelCase")]
    Start {
        message_id: String,
    },
    TextStart {
        id: String,
    },
    TextDelta {
        id: String,
        delta: String,
    },
    TextEnd {
        id: String,
    },
    #[serde(rename = "data-activity")]
    Activity {
        data: ActivityData,
        transient: bool,
    },
    #[serde(rename_all = "camelCase")]
    Error {
        error_text: String,
    },
    #[serde(rename_all = "camelCase")]
    Finish {
        finish_reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn start_serialises_to_camel_case_fields() {
        let chunk = UiChunk::Start {
            message_id: "msg-1".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&chunk).unwrap(),
            json!({ "type": "start", "messageId": "msg-1" })
        );
    }

    #[test]
    fn text_start_serialises() {
        let chunk = UiChunk::TextStart {
            id: "1".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&chunk).unwrap(),
            json!({ "type": "text-start", "id": "1" })
        );
    }

    #[test]
    fn text_delta_serialises() {
        let chunk = UiChunk::TextDelta {
            id: "1".to_string(),
            delta: "hello".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&chunk).unwrap(),
            json!({ "type": "text-delta", "id": "1", "delta": "hello" })
        );
    }

    #[test]
    fn text_end_serialises() {
        let chunk = UiChunk::TextEnd {
            id: "1".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&chunk).unwrap(),
            json!({ "type": "text-end", "id": "1" })
        );
    }

    #[test]
    fn activity_serialises_to_data_activity_tag() {
        let chunk = UiChunk::Activity {
            data: ActivityData {
                label: "Thinking...".to_string(),
            },
            transient: true,
        };
        assert_eq!(
            serde_json::to_value(&chunk).unwrap(),
            json!({
                "type": "data-activity",
                "data": { "label": "Thinking..." },
                "transient": true
            })
        );
    }

    #[test]
    fn error_serialises_to_camel_case_fields() {
        let chunk = UiChunk::Error {
            error_text: "boom".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&chunk).unwrap(),
            json!({ "type": "error", "errorText": "boom" })
        );
    }

    #[test]
    fn finish_serialises_to_camel_case_fields() {
        let chunk = UiChunk::Finish {
            finish_reason: "stop".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&chunk).unwrap(),
            json!({ "type": "finish", "finishReason": "stop" })
        );
    }
}
