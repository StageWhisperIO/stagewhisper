use serde::Serialize;

#[derive(Debug, Clone, PartialEq)]
pub enum ModelKind {
    Gguf { file: String },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum ModelSource {
    #[default]
    Remote,
    Local,
}

#[derive(Debug, Clone)]
pub struct ModelEntry {
    pub id: String,
    pub repo_id: String,
    pub revision: String,
    pub label: String,
    pub ram_hint_gb: f32,
    pub recommended: bool,
    pub kind: ModelKind,
    pub source: ModelSource,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmDownloadProgress {
    pub file_name: String,
    pub bytes_downloaded: u64,
    pub bytes_total: u64,
    pub files_completed: usize,
    pub files_total: usize,
}

#[derive(Debug, Clone)]
pub struct GenerationChunk {
    pub text: String,
    pub done: bool,
}

#[derive(Debug, Clone)]
pub struct InferenceParams {
    pub max_tokens: usize,
    pub temperature: f32,
    pub top_p: f32,
}

impl Default for InferenceParams {
    fn default() -> Self {
        Self {
            max_tokens: 1024,
            temperature: 0.7,
            top_p: 0.95,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("download failed: {0}")]
    Download(String),
    #[error("download canceled")]
    Cancelled,
    #[error("model not found: {0}")]
    ModelNotFound(String),
    #[error("failed to load model: {0}")]
    Load(String),
    #[error("inference failed: {0}")]
    Inference(String),
    #[error("inference timed out: {0}")]
    Timeout(String),
}
