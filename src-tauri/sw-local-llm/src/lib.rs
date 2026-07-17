pub mod download;
pub mod engine;
pub mod registry;
pub mod types;

pub use download::{
    default_llm_dir, delete_model, download_model_files, hf_cache_snapshot, model_exists,
    model_ready, resolve_gguf_path,
};
pub use engine::{LocalLlmEngine, SidecarPaths};
pub use registry::{curated, default_entry, model_dir, resolve, DEFAULT_MODEL_ID};
pub use types::{
    GenerationChunk, InferenceParams, LlmDownloadProgress, LlmError, ModelEntry, ModelKind,
    ModelSource,
};
