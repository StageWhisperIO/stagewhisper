use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};
use tokio::task::JoinHandle;
use tokio::time::Instant as WatchdogInstant;

use sw_notes::{TerminalResult, TurnHandle, TurnOutcome, TurnPersistence, TurnRegistry};

use super::{StorePersistence, LOCAL_IDENTITY_BINDING};

const LOCAL_LLM_SYSTEM_PROMPT: &str = "You are StageWhisper, a concise real-time call assistant. \
Answer the user's request directly and briefly, in plain language suitable for reading mid-call.";

const LOCAL_LLM_STREAM_INTERVAL_MS: u128 = 80;
const SESSION_LIBRARY_UNAVAILABLE_ERROR_TEXT: &str = "session library unavailable";
const LOCAL_TURN_QUEUE_UNAVAILABLE_ERROR_TEXT: &str = "the local turn queue is unavailable";
const LOCAL_ENGINE_STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const LOCAL_GENERATION_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const LOCAL_ENGINE_STARTUP_ERROR_TEXT: &str =
    "The on-device assistant is taking too long to start up. Please try again in a moment.";
const LOCAL_GENERATION_STALLED_ERROR_TEXT: &str =
    "The on-device assistant couldn't finish that response. Please try again.";

#[derive(Clone)]
struct LocalGenerationActivity {
    last_touch: Arc<Mutex<WatchdogInstant>>,
    started: Arc<AtomicBool>,
}

impl LocalGenerationActivity {
    fn new() -> Self {
        Self {
            last_touch: Arc::new(Mutex::new(WatchdogInstant::now())),
            started: Arc::new(AtomicBool::new(false)),
        }
    }

    fn touch(&self) {
        self.started.store(true, Ordering::Relaxed);
        *self.last_touch.lock().unwrap() = WatchdogInstant::now();
    }

    fn elapsed(&self) -> Duration {
        self.last_touch.lock().unwrap().elapsed()
    }

    fn has_started(&self) -> bool {
        self.started.load(Ordering::Relaxed)
    }
}

async fn await_local_generation(
    mut generation: JoinHandle<Result<String, String>>,
    activity: LocalGenerationActivity,
    startup_timeout: Duration,
    idle_timeout: Duration,
) -> Result<String, String> {
    loop {
        let timeout = if activity.has_started() {
            idle_timeout
        } else {
            startup_timeout
        };
        let remaining = timeout.saturating_sub(activity.elapsed());
        tokio::select! {
            joined = &mut generation => {
                return match joined {
                    Ok(result) => result,
                    Err(_) => Err(LOCAL_GENERATION_STALLED_ERROR_TEXT.to_string()),
                };
            }
            _ = tokio::time::sleep(remaining) => {
                if activity.elapsed() < timeout {
                    continue;
                }
                generation.abort();
                let message = if activity.has_started() {
                    LOCAL_GENERATION_STALLED_ERROR_TEXT
                } else {
                    LOCAL_ENGINE_STARTUP_ERROR_TEXT
                };
                return Err(message.to_string());
            }
        }
    }
}

struct UnavailablePersistence;

impl TurnPersistence for UnavailablePersistence {
    fn persist_reply(
        &self,
        _session_id: &str,
        _message: sw_notes::ChatMsg,
    ) -> Result<sw_notes::ChatAppendOutcome, String> {
        Err(SESSION_LIBRARY_UNAVAILABLE_ERROR_TEXT.to_string())
    }
}

fn finalize_local_reply(
    registry: &TurnRegistry,
    persistence: &dyn TurnPersistence,
    turn_id: &str,
    outcome: TurnOutcome,
) -> TerminalResult {
    registry.terminate(turn_id, outcome, Some(LOCAL_IDENTITY_BINDING), persistence)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn(
    app: AppHandle,
    registry: Arc<TurnRegistry>,
    handle: Arc<TurnHandle>,
    session_id: String,
    turn_id: String,
    parent_id: String,
    prompt: String,
) {
    let queue = app.state::<crate::state::local_llm::LocalTurnQueue>();
    let app_bg = app.clone();
    let queue_key = session_id.clone();
    let registry_for_queue_failure = registry.clone();
    let turn_id_for_queue_failure = turn_id.clone();

    let app_for_incomplete = app.clone();
    let registry_for_incomplete = registry.clone();
    let turn_id_for_incomplete = turn_id.clone();
    let on_incomplete = move |reason: String| {
        let outcome = TurnOutcome::Failure(reason);
        let result = match crate::session_store(&app_for_incomplete) {
            Ok(store) => finalize_local_reply(
                &registry_for_incomplete,
                &StorePersistence {
                    store: store.as_ref(),
                },
                &turn_id_for_incomplete,
                outcome,
            ),
            Err(_) => finalize_local_reply(
                &registry_for_incomplete,
                &UnavailablePersistence,
                &turn_id_for_incomplete,
                outcome,
            ),
        };
        super::log_non_terminated_result(
            "local_producer queue supervisor",
            &turn_id_for_incomplete,
            &result,
        );
    };

    let enqueue_result = queue.enqueue_for(&queue_key, on_incomplete, async move {
        let Ok(store) = crate::session_store(&app_bg) else {
            let result = finalize_local_reply(
                &registry,
                &UnavailablePersistence,
                &turn_id,
                TurnOutcome::Failure(SESSION_LIBRARY_UNAVAILABLE_ERROR_TEXT.to_string()),
            );
            super::log_non_terminated_result(
                "local_producer session store unavailable",
                &turn_id,
                &result,
            );
            return;
        };
        let record = store.load(&session_id).ok().flatten();
        let (segments, chat, summary) = record
            .map(|r| (r.segments, r.chat, r.notes_markdown))
            .unwrap_or_default();
        let previous_chat: Vec<sw_notes::ChatMsg> =
            chat.into_iter().filter(|m| m.id != parent_id).collect();
        let composed_prompt = sw_notes::build_session_chat_prompt(
            &segments,
            &previous_chat,
            summary.as_deref(),
            &prompt,
        );

        let activity = LocalGenerationActivity::new();
        let activity_for_callback = activity.clone();
        let stream_handle = handle.clone();
        let generation: JoinHandle<Result<String, String>> = tokio::spawn(async move {
            let mut pending_delta = String::new();
            let mut last_emit = Instant::now();
            crate::state::local_llm::generate_reply_streaming(
                &app_bg,
                Some(LOCAL_LLM_SYSTEM_PROMPT),
                &composed_prompt,
                move |chunk| {
                    activity_for_callback.touch();
                    if stream_handle.is_cancelled() {
                        return;
                    }
                    if !chunk.text.is_empty() {
                        pending_delta.push_str(&chunk.text);
                    }
                    let should_flush = chunk.done
                        || last_emit.elapsed().as_millis() >= LOCAL_LLM_STREAM_INTERVAL_MS;
                    if should_flush && !pending_delta.is_empty() {
                        stream_handle.push_text_delta(&pending_delta);
                        pending_delta.clear();
                        last_emit = Instant::now();
                    }
                },
            )
            .await
        });

        let result = await_local_generation(
            generation,
            activity,
            LOCAL_ENGINE_STARTUP_TIMEOUT,
            LOCAL_GENERATION_IDLE_TIMEOUT,
        )
        .await;

        if handle.is_cancelled() {
            return;
        }

        let persistence = StorePersistence {
            store: store.as_ref(),
        };
        let outcome = match result {
            Ok(content) => TurnOutcome::Reply(content),
            Err(err) => TurnOutcome::Failure(err),
        };
        let termination = finalize_local_reply(&registry, &persistence, &turn_id, outcome);
        super::log_non_terminated_result("local_producer reply", &turn_id, &termination);
    });

    if enqueue_result.is_err() {
        let result = finalize_local_reply(
            &registry_for_queue_failure,
            &UnavailablePersistence,
            &turn_id_for_queue_failure,
            TurnOutcome::Failure(LOCAL_TURN_QUEUE_UNAVAILABLE_ERROR_TEXT.to_string()),
        );
        super::log_non_terminated_result(
            "local_producer queue unavailable",
            &turn_id_for_queue_failure,
            &result,
        );
    }
}

#[cfg(test)]
#[path = "local_producer_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "local_producer_watchdog_tests.rs"]
mod watchdog_tests;
