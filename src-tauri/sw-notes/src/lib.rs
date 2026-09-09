pub mod accumulate;
pub mod blocks;
pub mod chat_stream;
pub mod chat_turns;
pub mod chunking;
pub mod finalize;
pub mod insights;
pub mod ordering;
pub mod session_chat;
pub mod store;
pub mod summary;

pub const HOST_TEXT_MAX_CHARS: usize = 16_000;

pub use accumulate::{TranscriptAccumulator, TranscriptSource};
pub use blocks::{
    build_block_summary_message, build_relay_block_summary_message, carried_playbook_state,
    BlockSummary, BLOCK_CHAR_TARGET, BLOCK_PROMPT_BUDGET_CHARS,
};
pub use chat_stream::{ActivityData, UiChunk};
pub use chat_turns::{
    ChunkSink, TerminalResult, TerminationOutcome, TurnHandle, TurnOutcome, TurnPersistence,
    TurnRegistry, TurnSnapshot,
};
pub use chunking::chunk_to_char_budget;
pub use ordering::{OrderedUtterance, TranscriptOrderer};
pub use session_chat::{
    build_hosted_chat_history, build_relay_chat_context, build_relay_chat_outbound,
    build_relay_chat_outbound_with_screen_context, build_session_chat_prompt,
    render_session_transcript, strip_prompt_delimiters, tail_chars, CHAT_CONTEXT_CHARS,
    HOSTED_CHAT_HISTORY_CHARS, HOSTED_QUESTION_CHARS, RELAY_CHAT_TEXT_LIMIT, SUMMARY_CONTEXT_CHARS,
    TRANSCRIPT_CONTEXT_CHARS,
};
pub use store::{
    derive_title, ChatAppendOutcome, ChatMsg, InsightNote, SessionRecord, SessionStore,
    SessionSummary, StoreError,
};
pub use summary::{
    build_local_summary_message, build_relay_summary_message, LOCAL_PROMPT_BUDGET_CHARS,
};
