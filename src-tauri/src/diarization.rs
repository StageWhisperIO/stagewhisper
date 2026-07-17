use std::path::PathBuf;
use std::sync::Mutex;

use sw_audio_recording::diarization::{Diarizer, VoicePrintStore};
use tauri::{AppHandle, Manager};

use crate::state::app_state::AppState;

#[derive(serde::Serialize)]
pub struct SessionSpeaker {
    pub speaker_id: String,
    pub speaker_label: Option<String>,
}

fn file_key(app: &AppHandle) -> Result<[u8; 32], String> {
    let data = app.state::<Mutex<AppState>>();
    let state = data.lock().unwrap();
    let device_key = state
        .device_key
        .as_ref()
        .ok_or_else(|| "secure storage is locked".to_string())?;
    device_key.file_key()
}

pub fn voiceprint_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("resolving app data dir: {err}"))?;
    Ok(dir.join("sessions").join("voiceprints.bin"))
}

pub fn open_diarizer(app: &AppHandle) -> Result<Diarizer, String> {
    let key = file_key(app)?;
    let path = voiceprint_path(app)?;
    let store = VoicePrintStore::load(&path, key).map_err(|err| err.to_string())?;
    let model_dir = sw_audio_recording::download::default_model_dir();
    let embedding_model = sw_audio_recording::download::speaker_embedding_path(&model_dir);
    Diarizer::new(&embedding_model, store).map_err(|err| err.to_string())
}

pub fn rename_speaker(app: &AppHandle, speaker_id: &str, label: Option<String>) -> Result<(), String> {
    let key = file_key(app)?;
    let path = voiceprint_path(app)?;
    let mut store = VoicePrintStore::load(&path, key).map_err(|err| err.to_string())?;
    let normalized = label
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    store
        .rename(speaker_id, normalized.clone())
        .map_err(|err| err.to_string())?;

    if let Some(store_handle) = app.try_state::<std::sync::Arc<sw_notes::SessionStore>>() {
        let session_store = store_handle.inner().clone();
        if let Ok(summaries) = session_store.list() {
            for summary in summaries {
                relabel_session(&session_store, &summary.session_id, speaker_id, normalized.as_deref());
            }
        }
    }
    Ok(())
}

fn relabel_session(
    store: &sw_notes::SessionStore,
    session_id: &str,
    speaker_id: &str,
    label: Option<&str>,
) {
    let Ok(Some(mut record)) = store.load(session_id) else {
        return;
    };
    let mut changed = false;
    for segment in &mut record.segments {
        if segment.speaker_id.as_deref() == Some(speaker_id) {
            let next = label.map(|value| value.to_string());
            if segment.speaker_label != next {
                segment.speaker_label = next;
                changed = true;
            }
        }
    }
    if changed {
        let _ = store.save(&record);
    }
}

pub fn list_session_speakers(app: &AppHandle, session_id: &str) -> Result<Vec<SessionSpeaker>, String> {
    let store = app
        .try_state::<std::sync::Arc<sw_notes::SessionStore>>()
        .ok_or_else(|| "session storage unavailable".to_string())?
        .inner()
        .clone();
    let record = store
        .load(session_id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "session not found".to_string())?;

    let mut speakers: Vec<SessionSpeaker> = Vec::new();
    for segment in &record.segments {
        let Some(speaker_id) = segment.speaker_id.as_ref() else {
            continue;
        };
        match speakers.iter_mut().find(|s| &s.speaker_id == speaker_id) {
            Some(existing) => {
                if existing.speaker_label.is_none() {
                    existing.speaker_label = segment.speaker_label.clone();
                }
            }
            None => speakers.push(SessionSpeaker {
                speaker_id: speaker_id.clone(),
                speaker_label: segment.speaker_label.clone(),
            }),
        }
    }
    Ok(speakers)
}
