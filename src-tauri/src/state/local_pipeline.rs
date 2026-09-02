use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::app_state::{AppState, PipelineMode};

const MODEL_DOWNLOAD_PROGRESS_EVENT: &str = "model-download-progress";
const MODEL_DOWNLOAD_COMPLETE_EVENT: &str = "model-download-complete";
const MODEL_DOWNLOAD_ERROR_EVENT: &str = "model-download-error";

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatusPayload {
    pub ready: bool,
    pub exists: bool,
    pub model_dir: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DownloadProgressPayload {
    file_name: String,
    bytes_downloaded: u64,
    bytes_total: u64,
    files_completed: usize,
    files_total: usize,
}

fn model_dir() -> PathBuf {
    sw_audio_recording::download::default_model_dir()
}

pub fn all_models_ready(dir: &std::path::Path) -> bool {
    let base = sw_audio_recording::download::models_ready(dir);
    #[cfg(feature = "diarization")]
    {
        base && sw_audio_recording::download::speaker_models_ready(dir)
    }
    #[cfg(not(feature = "diarization"))]
    {
        base
    }
}

#[tauri::command]
pub async fn get_model_status(
    data: State<'_, Mutex<AppState>>,
) -> Result<ModelStatusPayload, String> {
    let dir = model_dir();
    let ready = tokio::task::spawn_blocking({
        let dir = dir.clone();
        move || all_models_ready(&dir)
    })
    .await
    .map_err(|e| format!("model status check failed: {e}"))?;
    if let Ok(mut state) = data.lock() {
        state.models_ready = ready;
    }
    Ok(ModelStatusPayload {
        ready,
        exists: sw_audio_recording::download::model_exists(&dir),
        model_dir: dir.display().to_string(),
    })
}

#[tauri::command]
pub async fn download_models(app: AppHandle) -> Result<(), String> {
    let dir = model_dir();

    let dir_check = dir.clone();
    let already_ready = tokio::task::spawn_blocking(move || all_models_ready(&dir_check))
        .await
        .map_err(|e| format!("model check failed: {e}"))?;

    if already_ready {
        let _ = app.emit(MODEL_DOWNLOAD_COMPLETE_EVENT, ());
        return Ok(());
    }

    let app_for_progress = app.clone();

    sw_audio_recording::vad::ensure_vad_model(&dir)
        .await
        .map_err(|e| format!("Failed to download Silero VAD: {e}"))?;

    let result = sw_audio_recording::download::download_model(&dir, false, move |progress| {
        let _ = app_for_progress.emit(
            MODEL_DOWNLOAD_PROGRESS_EVENT,
            DownloadProgressPayload {
                file_name: progress.file_name.clone(),
                bytes_downloaded: progress.bytes_downloaded,
                bytes_total: progress.bytes_total,
                files_completed: progress.files_completed,
                files_total: progress.files_total,
            },
        );
    })
    .await;

    match result {
        Ok(()) => {
            #[cfg(feature = "diarization")]
            {
                let app_for_speaker = app.clone();
                if let Err(e) =
                    sw_audio_recording::download::download_speaker_models(&dir, move |progress| {
                        let _ = app_for_speaker.emit(
                            MODEL_DOWNLOAD_PROGRESS_EVENT,
                            DownloadProgressPayload {
                                file_name: progress.file_name.clone(),
                                bytes_downloaded: progress.bytes_downloaded,
                                bytes_total: progress.bytes_total,
                                files_completed: progress.files_completed,
                                files_total: progress.files_total,
                            },
                        );
                    })
                    .await
                {
                    let msg = format!("{e}");
                    let _ = app.emit(MODEL_DOWNLOAD_ERROR_EVENT, &msg);
                    return Err(msg);
                }
            }
            let data = app.state::<Mutex<AppState>>();
            if let Ok(mut state) = data.lock() {
                state.models_ready = true;
            }
            let _ = app.emit(MODEL_DOWNLOAD_COMPLETE_EVENT, ());
            Ok(())
        }
        Err(e) => {
            let msg = format!("{e}");
            let _ = app.emit(MODEL_DOWNLOAD_ERROR_EVENT, &msg);
            Err(msg)
        }
    }
}

#[tauri::command]
pub fn get_pipeline_mode(data: State<'_, Mutex<AppState>>) -> String {
    let state = data.lock().unwrap();
    match state.pipeline_mode {
        PipelineMode::Cloud => "cloud".to_string(),
        PipelineMode::Local => "local".to_string(),
    }
}

#[tauri::command]
pub async fn check_models_ready(data: State<'_, Mutex<AppState>>) -> Result<bool, String> {
    let dir = model_dir();
    let ready = tokio::task::spawn_blocking(move || all_models_ready(&dir))
        .await
        .map_err(|e| format!("model check failed: {e}"))?;
    if let Ok(mut state) = data.lock() {
        state.models_ready = ready;
    }
    Ok(ready)
}
