use crate::audio::AudioStopSignal;

use super::device_key::DeviceKeyManager;
use super::local_llm::LocalLlmPrefs;
use super::session::SessionState;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PipelineMode {
    #[allow(dead_code)]
    Cloud,
    #[default]
    Local,
}

pub struct AppState {
    pub is_screen_sharing_private: bool,
    pub session_state: SessionState,
    pub audio_stop_tx: Option<AudioStopSignal>,
    pub session_id: Option<String>,
    pub pipeline_mode: PipelineMode,
    pub models_ready: bool,
    pub device_key: Option<DeviceKeyManager>,
    pub local_llm_prefs: LocalLlmPrefs,
    pub local_llm_ready: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            is_screen_sharing_private: true,
            session_state: SessionState::Stopped,
            audio_stop_tx: None,
            session_id: None,
            pipeline_mode: PipelineMode::default(),
            models_ready: false,
            device_key: None,
            local_llm_prefs: LocalLlmPrefs::default(),
            local_llm_ready: false,
        }
    }
}
