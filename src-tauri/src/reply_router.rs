use serde::{Deserialize, Serialize};
use serde_json::Value;

mod pending;
mod probe;
mod routing;
mod sink;
mod stream;

#[allow(unused_imports)]
pub use pending::ReserveResult;
pub use pending::{PendingReplies, TimeoutCheck};
pub use probe::{ProbeOutcome, ProbeRegistry};
#[allow(unused_imports)]
pub use routing::{route_reply, DropReason, ReplyBody, ReplyDisposition};
pub use sink::{ReplySink, TauriReplySink};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessagePayload {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub status: String,
    pub tool_calls: Option<Vec<Value>>,
    pub tool_result_payload: Option<Value>,
    pub parent_message_id: Option<String>,
    pub suggestion_id: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub finalized_at: Option<String>,
}

#[cfg(test)]
#[path = "reply_router/cross_session_direct_tests.rs"]
mod cross_session_tests;

#[cfg(test)]
#[path = "reply_router/pending_backed_direct_tests.rs"]
mod pending_backed_tests;
