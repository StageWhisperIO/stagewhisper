use std::backtrace::Backtrace;
use std::panic;
use std::sync::RwLock;

use tauri_plugin_log::{Builder, RotationStrategy, Target, TargetKind};

const LOG_FILE_STEM: &str = "stagewhisper-lite";
const MAX_LOG_FILE_BYTES: u128 = 10 * 1024 * 1024;
const LEVEL_ENV_VAR: &str = "STAGEWHISPER_LOG";
const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3fZ";

static ACTIVE_SESSION: RwLock<Option<String>> = RwLock::new(None);

pub fn set_active_session(session_id: &str) {
    if let Ok(mut active) = ACTIVE_SESSION.write() {
        *active = Some(session_id.to_string());
    }
}

#[cfg(test)]
pub fn clear_active_session() {
    if let Ok(mut active) = ACTIVE_SESSION.write() {
        *active = None;
    }
}

#[cfg(test)]
pub static ACTIVE_SESSION_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub fn active_session_tag() -> Option<String> {
    ACTIVE_SESSION.read().ok().and_then(|active| active.clone())
}

fn active_session_field() -> String {
    ACTIVE_SESSION
        .read()
        .ok()
        .and_then(|active| active.as_deref().map(|id| format!("[session={id}]")))
        .unwrap_or_default()
}

fn configured_level() -> log::LevelFilter {
    std::env::var(LEVEL_ENV_VAR)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(log::LevelFilter::Info)
}

pub fn plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    Builder::new()
        .clear_targets()
        .targets([
            Target::new(TargetKind::Stdout),
            Target::new(TargetKind::LogDir {
                file_name: Some(LOG_FILE_STEM.to_string()),
            }),
        ])
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{}][{:<5}][{}]{} {message}",
                chrono::Utc::now().format(TIMESTAMP_FORMAT),
                record.level(),
                record.target(),
                active_session_field(),
            ))
        })
        .level(configured_level())
        .max_file_size(MAX_LOG_FILE_BYTES)
        .rotation_strategy(RotationStrategy::KeepOne)
        .build()
}

pub fn startup(step: &str) {
    log::info!("[startup] {step}");
}

pub fn install_panic_hook() {
    let previous_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        log::error!("{info}\n{}", Backtrace::force_capture());
        previous_hook(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::{
        active_session_field, active_session_tag, clear_active_session, set_active_session,
    };

    #[test]
    fn the_session_field_is_empty_until_a_session_starts_and_is_replaced_by_the_next_one() {
        let _guard = super::ACTIVE_SESSION_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_active_session();
        assert_eq!(active_session_field(), "");

        set_active_session("a48752a1-c5b8-4cd9-af88-c87ebb4b36ed");
        assert_eq!(
            active_session_field(),
            "[session=a48752a1-c5b8-4cd9-af88-c87ebb4b36ed]"
        );

        set_active_session("ffbe1ec9-e5e0-4a20-a91d-d3b2f1f04c77");
        assert_eq!(
            active_session_field(),
            "[session=ffbe1ec9-e5e0-4a20-a91d-d3b2f1f04c77]"
        );

        clear_active_session();
    }

    #[test]
    fn teardown_after_a_session_ends_stays_attributed_to_it_until_the_next_one_starts() {
        let _guard = super::ACTIVE_SESSION_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let first = "a48752a1-c5b8-4cd9-af88-c87ebb4b36ed";
        let second = "ffbe1ec9-e5e0-4a20-a91d-d3b2f1f04c77";
        set_active_session(first);
        assert_eq!(active_session_tag().as_deref(), Some(first));

        set_active_session(second);

        assert_eq!(active_session_tag().as_deref(), Some(second));

        clear_active_session();
    }
}
