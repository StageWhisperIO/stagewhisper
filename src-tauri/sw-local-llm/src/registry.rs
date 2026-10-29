use crate::types::{ModelEntry, ModelKind, ModelSource};
use std::path::{Path, PathBuf};

pub const DEFAULT_MODEL_ID: &str = "gemma-4-e2b-it";

pub fn curated() -> Vec<ModelEntry> {
    vec![
        ModelEntry {
            id: "gemma-4-e2b-it".to_string(),
            repo_id: "unsloth/gemma-4-E2B-it-qat-GGUF".to_string(),
            revision: "main".to_string(),
            label: "Gemma 4 E2B".to_string(),
            ram_hint_gb: 4.0,
            recommended: true,
            kind: ModelKind::Gguf {
                file: "gemma-4-E2B-it-qat-UD-Q4_K_XL.gguf".to_string(),
            },
            source: ModelSource::Remote,
        },
        ModelEntry {
            id: "gemma-4-e4b-it".to_string(),
            repo_id: "unsloth/gemma-4-E4B-it-qat-GGUF".to_string(),
            revision: "main".to_string(),
            label: "Gemma 4 E4B".to_string(),
            ram_hint_gb: 6.0,
            recommended: false,
            kind: ModelKind::Gguf {
                file: "gemma-4-E4B-it-qat-UD-Q4_K_XL.gguf".to_string(),
            },
            source: ModelSource::Remote,
        },
        ModelEntry {
            id: "gemma-4-12b-it".to_string(),
            repo_id: "unsloth/gemma-4-12B-it-qat-GGUF".to_string(),
            revision: "main".to_string(),
            label: "Gemma 4 12B".to_string(),
            ram_hint_gb: 16.0,
            recommended: false,
            kind: ModelKind::Gguf {
                file: "gemma-4-12B-it-qat-UD-Q4_K_XL.gguf".to_string(),
            },
            source: ModelSource::Remote,
        },
    ]
}

pub fn resolve(id_or_repo: &str) -> Option<ModelEntry> {
    let trimmed = id_or_repo.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(entry) = curated().into_iter().find(|m| m.id == trimmed) {
        return Some(entry);
    }

    if curated().iter().any(|m| m.repo_id == trimmed) {
        return curated().into_iter().find(|m| m.repo_id == trimmed);
    }

    let path = Path::new(trimmed);
    if path.is_absolute() && path.is_dir() {
        return local_dir_entry(path);
    }

    if trimmed.contains('/') && !trimmed.contains(char::is_whitespace) {
        return Some(ModelEntry {
            id: trimmed.to_string(),
            repo_id: trimmed.to_string(),
            revision: "main".to_string(),
            label: trimmed.to_string(),
            ram_hint_gb: 0.0,
            recommended: false,
            kind: ModelKind::Gguf {
                file: String::new(),
            },
            source: ModelSource::Remote,
        });
    }

    None
}

pub fn local_dir_entry(path: &Path) -> Option<ModelEntry> {
    let kind = crate::download::detect_local_kind(path)?;
    let label = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    Some(ModelEntry {
        id: path.display().to_string(),
        repo_id: path.display().to_string(),
        revision: "local".to_string(),
        label,
        ram_hint_gb: 0.0,
        recommended: false,
        kind,
        source: ModelSource::Local,
    })
}

pub fn default_entry() -> ModelEntry {
    resolve(DEFAULT_MODEL_ID).expect("default model must resolve")
}

pub fn sanitize_repo(repo_id: &str) -> String {
    repo_id.replace(['/', '\\'], "__")
}

pub fn model_dir(base_dir: &Path, entry: &ModelEntry) -> PathBuf {
    match entry.source {
        ModelSource::Local => PathBuf::from(&entry.repo_id),
        ModelSource::Remote => base_dir.join(sanitize_repo(&entry.repo_id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_gemma_e2b() {
        let entry = default_entry();
        assert_eq!(entry.id, "gemma-4-e2b-it");
        assert_eq!(entry.repo_id, "unsloth/gemma-4-E2B-it-qat-GGUF");
        assert!(entry.recommended);
        assert!(matches!(entry.kind, ModelKind::Gguf { file } if !file.is_empty()));
    }

    #[test]
    fn exactly_one_recommended() {
        assert_eq!(curated().iter().filter(|m| m.recommended).count(), 1);
    }

    #[test]
    fn resolve_curated_by_id() {
        let entry = resolve("gemma-4-e4b-it").unwrap();
        assert_eq!(entry.repo_id, "unsloth/gemma-4-E4B-it-qat-GGUF");
        assert!(matches!(entry.kind, ModelKind::Gguf { .. }));
    }

    #[test]
    fn resolve_custom_repo_as_gguf() {
        let entry = resolve("unsloth/Qwen3-4B-Instruct-GGUF").unwrap();
        assert_eq!(entry.repo_id, "unsloth/Qwen3-4B-Instruct-GGUF");
        assert_eq!(entry.revision, "main");
        assert!(!entry.recommended);
        assert!(matches!(entry.kind, ModelKind::Gguf { file } if file.is_empty()));
    }

    #[test]
    fn resolve_rejects_garbage() {
        assert!(resolve("").is_none());
        assert!(resolve("not a repo id").is_none());
        assert!(resolve("singleword").is_none());
    }

    #[test]
    fn resolve_local_safetensors_dir_is_none() {
        let dir = std::env::temp_dir().join("sw-llm-local-st");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), "{}").unwrap();
        std::fs::write(dir.join("model.safetensors"), b"x").unwrap();

        assert!(resolve(&dir.display().to_string()).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_local_gguf_dir() {
        let dir = std::env::temp_dir().join("sw-llm-local-gguf");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model-Q4_K_M.gguf"), b"x").unwrap();

        let entry = resolve(&dir.display().to_string()).unwrap();
        assert_eq!(entry.source, ModelSource::Local);
        assert!(matches!(entry.kind, ModelKind::Gguf { .. }));
        assert_eq!(model_dir(Path::new("/base"), &entry), dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_local_dir_without_weights_is_none() {
        let dir = std::env::temp_dir().join("sw-llm-local-empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(resolve(&dir.display().to_string()).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_replaces_slash() {
        assert_eq!(
            sanitize_repo("unsloth/Qwen3.6-35B-A3B-GGUF"),
            "unsloth__Qwen3.6-35B-A3B-GGUF"
        );
    }
}
