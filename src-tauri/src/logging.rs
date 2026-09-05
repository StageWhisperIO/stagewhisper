use std::backtrace::Backtrace;
use std::panic;

use tauri_plugin_log::{Builder, RotationStrategy, Target, TargetKind};

const LOG_FILE_STEM: &str = "stagewhisper-lite";
const MAX_LOG_FILE_BYTES: u128 = 10 * 1024 * 1024;
const LEVEL_ENV_VAR: &str = "STAGEWHISPER_LOG";

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
