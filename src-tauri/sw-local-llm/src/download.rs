use crate::registry;
use crate::types::{LlmDownloadProgress, LlmError, ModelEntry, ModelKind, ModelSource};
use futures_util::StreamExt;
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::fs;
use tokio::io::AsyncWriteExt;

const HF_BASE_URL: &str = "https://huggingface.co";
const USER_AGENT: &str = "stagewhisper-desktop/0.1.0";
const READY_MARKER: &str = ".ready";

pub fn default_llm_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.stagewhisper.app")
        .join("models")
        .join("llm")
}

fn kind_files_present(dir: &Path, kind: &ModelKind) -> bool {
    match kind {
        ModelKind::Gguf { file } if !file.is_empty() => dir.join(file).exists(),
        ModelKind::Gguf { .. } => has_any_gguf(dir),
    }
}

pub fn model_ready(base_dir: &Path, entry: &ModelEntry) -> bool {
    let dir = registry::model_dir(base_dir, entry);
    match entry.source {
        ModelSource::Local => kind_files_present(&dir, &entry.kind),
        ModelSource::Remote => {
            dir.join(READY_MARKER).exists() && kind_files_present(&dir, &entry.kind)
        }
    }
}

pub fn model_exists(base_dir: &Path, entry: &ModelEntry) -> bool {
    kind_files_present(&registry::model_dir(base_dir, entry), &entry.kind)
}

pub fn detect_local_kind(dir: &Path) -> Option<ModelKind> {
    first_gguf(dir).map(|file| ModelKind::Gguf { file })
}

pub fn resolve_gguf_path(model_dir: &Path, entry: &ModelEntry) -> Option<PathBuf> {
    match &entry.kind {
        ModelKind::Gguf { file } if !file.is_empty() => {
            let path = model_dir.join(file);
            path.exists().then_some(path)
        }
        ModelKind::Gguf { .. } => first_gguf(model_dir).map(|file| model_dir.join(file)),
    }
}

fn collect_gguf_paths(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_gguf_paths(base, &path, out);
        } else if path
            .file_name()
            .map(|name| is_gguf_file(&name.to_string_lossy()))
            .unwrap_or(false)
        {
            if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.to_string_lossy().to_string());
            }
        }
    }
}

fn first_gguf(dir: &Path) -> Option<String> {
    let mut names = Vec::new();
    collect_gguf_paths(dir, dir, &mut names);
    names.sort();
    names.into_iter().next()
}

fn hf_cache_bases() -> Vec<PathBuf> {
    let mut bases = Vec::new();
    if let Ok(value) = std::env::var("HUGGINGFACE_HUB_CACHE") {
        if !value.trim().is_empty() {
            bases.push(PathBuf::from(value));
        }
    }
    if let Ok(value) = std::env::var("HF_HOME") {
        if !value.trim().is_empty() {
            bases.push(PathBuf::from(value).join("hub"));
        }
    }
    if let Some(home) = dirs::home_dir() {
        bases.push(home.join(".cache").join("huggingface").join("hub"));
    }
    bases
}

pub fn hf_cache_snapshot(repo_id: &str) -> Option<PathBuf> {
    hf_cache_snapshot_in(&hf_cache_bases(), repo_id)
}

fn hf_cache_snapshot_in(bases: &[PathBuf], repo_id: &str) -> Option<PathBuf> {
    let repo = repo_id.trim();
    if !repo.contains('/') {
        return None;
    }
    let folder = format!("models--{}", repo.replace('/', "--"));
    for base in bases {
        let model_dir = base.join(&folder);
        if let Some(snapshot) = resolve_snapshot(&model_dir) {
            if detect_local_kind(&snapshot).is_some() {
                return Some(snapshot);
            }
        }
    }
    None
}

fn resolve_snapshot(model_dir: &Path) -> Option<PathBuf> {
    let snapshots = model_dir.join("snapshots");
    if let Ok(hash) = std::fs::read_to_string(model_dir.join("refs").join("main")) {
        let snapshot = snapshots.join(hash.trim());
        if snapshot.is_dir() {
            return Some(snapshot);
        }
    }
    std::fs::read_dir(&snapshots)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.is_dir())
}

pub async fn delete_model(base_dir: &Path, entry: &ModelEntry) -> Result<(), LlmError> {
    if entry.source == ModelSource::Local {
        return Ok(());
    }
    let dir = registry::model_dir(base_dir, entry);
    if dir.exists() {
        fs::remove_dir_all(&dir)
            .await
            .map_err(|e| LlmError::Download(format!("failed to delete {}: {e}", dir.display())))?;
    }
    Ok(())
}

fn has_any_gguf(dir: &Path) -> bool {
    first_gguf(dir).is_some()
}

fn token_from_env() -> Option<String> {
    std::env::var("HF_TOKEN")
        .or_else(|_| std::env::var("HUGGING_FACE_HUB_TOKEN"))
        .ok()
        .filter(|t| !t.trim().is_empty())
}

#[derive(serde::Deserialize)]
struct HfTreeEntry {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    #[serde(default)]
    size: u64,
}

struct PlannedFile {
    path: String,
    size: u64,
}

pub async fn download_model_files(
    base_dir: &Path,
    entry: &ModelEntry,
    hf_token: Option<String>,
    cancel: &AtomicBool,
    on_progress: impl Fn(LlmDownloadProgress) + Send + 'static,
) -> Result<(), LlmError> {
    if entry.source == ModelSource::Local {
        return Ok(());
    }
    let dir = registry::model_dir(base_dir, entry);
    fs::create_dir_all(&dir)
        .await
        .map_err(|e| LlmError::Download(format!("failed to create {}: {e}", dir.display())))?;

    let token = hf_token.filter(|t| !t.trim().is_empty()).or_else(token_from_env);

    let client = Client::builder()
        .https_only(true)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| LlmError::Download(e.to_string()))?;

    let files = plan_files(&client, entry, token.as_deref()).await?;
    let files_total = files.len();
    if files_total == 0 {
        return Err(LlmError::Download(
            "no downloadable files found for this model".to_string(),
        ));
    }

    for (index, file) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(LlmError::Cancelled);
        }
        let dest = dir.join(&file.path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| LlmError::Download(e.to_string()))?;
        }

        if dest.exists() && file.size > 0 {
            let on_disk = fs::metadata(&dest).await.map(|m| m.len()).unwrap_or(0);
            if on_disk == file.size {
                on_progress(LlmDownloadProgress {
                    file_name: file.path.clone(),
                    bytes_downloaded: file.size,
                    bytes_total: file.size,
                    files_completed: index + 1,
                    files_total,
                });
                continue;
            }
        }

        download_file(
            &client,
            entry,
            file,
            &dest,
            index,
            files_total,
            token.as_deref(),
            cancel,
            &on_progress,
        )
        .await?;
    }

    fs::write(dir.join(READY_MARKER), "ok")
        .await
        .map_err(|e| LlmError::Download(e.to_string()))?;

    Ok(())
}

async fn plan_files(
    client: &Client,
    entry: &ModelEntry,
    token: Option<&str>,
) -> Result<Vec<PlannedFile>, LlmError> {
    match &entry.kind {
        ModelKind::Gguf { file } if !file.is_empty() => Ok(vec![PlannedFile {
            path: file.clone(),
            size: 0,
        }]),
        ModelKind::Gguf { .. } => {
            let tree = fetch_tree(client, entry, token).await?;
            let planned = select_gguf_files(tree);
            if planned.is_empty() {
                return Err(LlmError::Download(format!(
                    "no .gguf files found in {}",
                    entry.repo_id
                )));
            }
            Ok(planned)
        }
    }
}

fn select_gguf_files(tree: Vec<HfTreeEntry>) -> Vec<PlannedFile> {
    use std::collections::BTreeMap;

    let mut groups: BTreeMap<String, Vec<PlannedFile>> = BTreeMap::new();
    for entry in tree {
        if entry.kind != "file" || !is_safe_relative_path(&entry.path) || !is_gguf_file(&entry.path)
        {
            continue;
        }
        groups
            .entry(shard_base(&entry.path))
            .or_default()
            .push(PlannedFile {
                path: entry.path,
                size: entry.size,
            });
    }

    let key = groups
        .keys()
        .find(|k| k.to_ascii_lowercase().contains("q4_k_m"))
        .or_else(|| groups.keys().next())
        .cloned();

    let mut planned = key
        .and_then(|k| groups.remove(&k))
        .unwrap_or_default();
    planned.sort_by(|a, b| a.path.cmp(&b.path));
    planned
}

fn is_gguf_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".gguf") && !lower.contains("mmproj")
}

fn shard_base(path: &str) -> String {
    let stem = path
        .strip_suffix(".gguf")
        .or_else(|| path.strip_suffix(".GGUF"))
        .unwrap_or(path);
    if let Some(of_pos) = stem.to_ascii_lowercase().rfind("-of-") {
        let before = &stem[..of_pos];
        let after = &stem[of_pos + 4..];
        if let Some(dash) = before.rfind('-') {
            let shard = &before[dash + 1..];
            if !shard.is_empty()
                && shard.bytes().all(|b| b.is_ascii_digit())
                && !after.is_empty()
                && after.bytes().all(|b| b.is_ascii_digit())
            {
                return before[..dash].to_string();
            }
        }
    }
    stem.to_string()
}

fn is_safe_relative_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.starts_with('\\') || path.contains(':') {
        return false;
    }
    !path
        .split(['/', '\\'])
        .any(|component| component == ".." || component == "." || component.is_empty())
}

async fn fetch_tree(
    client: &Client,
    entry: &ModelEntry,
    token: Option<&str>,
) -> Result<Vec<HfTreeEntry>, LlmError> {
    let url = format!(
        "{HF_BASE_URL}/api/models/{}/tree/{}?recursive=true",
        entry.repo_id, entry.revision
    );
    let mut req = client.get(&url);
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    let response = req
        .send()
        .await
        .map_err(|e| LlmError::Download(format!("failed to list {}: {e}", entry.repo_id)))?;

    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(LlmError::Download(format!(
            "{} is gated or private. Accept its license on Hugging Face, then add an access token.",
            entry.repo_id
        )));
    }
    if !response.status().is_success() {
        return Err(LlmError::Download(format!(
            "failed to list {}: HTTP {}",
            entry.repo_id,
            response.status()
        )));
    }

    response
        .json::<Vec<HfTreeEntry>>()
        .await
        .map_err(|e| LlmError::Download(format!("invalid file listing for {}: {e}", entry.repo_id)))
}

#[allow(clippy::too_many_arguments)]
async fn download_file(
    client: &Client,
    entry: &ModelEntry,
    file: &PlannedFile,
    dest: &Path,
    file_index: usize,
    files_total: usize,
    token: Option<&str>,
    cancel: &AtomicBool,
    on_progress: &(impl Fn(LlmDownloadProgress) + Send + 'static),
) -> Result<(), LlmError> {
    let url = format!(
        "{HF_BASE_URL}/{}/resolve/{}/{}",
        entry.repo_id, entry.revision, file.path
    );

    let mut req = client.get(&url);
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    let response = req
        .send()
        .await
        .map_err(|e| LlmError::Download(format!("failed to request {}: {e}", file.path)))?;

    if !response.status().is_success() {
        return Err(LlmError::Download(format!(
            "failed to download {}: HTTP {}",
            file.path,
            response.status()
        )));
    }

    let bytes_total = response.content_length().unwrap_or(file.size);

    let tmp_path = temp_path(dest);
    let _ = fs::remove_file(&tmp_path).await;
    let mut out = fs::File::create(&tmp_path)
        .await
        .map_err(|e| LlmError::Download(format!("failed to create {}: {e}", tmp_path.display())))?;

    let mut stream = response.bytes_stream();
    let mut bytes_downloaded: u64 = 0;

    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            drop(out);
            let _ = fs::remove_file(&tmp_path).await;
            return Err(LlmError::Cancelled);
        }
        let chunk = chunk.map_err(|e| LlmError::Download(format!("error downloading {}: {e}", file.path)))?;
        out.write_all(&chunk)
            .await
            .map_err(|e| LlmError::Download(e.to_string()))?;
        bytes_downloaded += chunk.len() as u64;

        on_progress(LlmDownloadProgress {
            file_name: file.path.clone(),
            bytes_downloaded,
            bytes_total,
            files_completed: file_index,
            files_total,
        });
    }

    out.flush().await.map_err(|e| LlmError::Download(e.to_string()))?;
    out.sync_all().await.map_err(|e| LlmError::Download(e.to_string()))?;
    drop(out);

    fs::rename(&tmp_path, dest).await.map_err(|e| {
        LlmError::Download(format!(
            "failed to finalize {} -> {}: {e}",
            tmp_path.display(),
            dest.display()
        ))
    })?;

    on_progress(LlmDownloadProgress {
        file_name: file.path.clone(),
        bytes_downloaded: bytes_total.max(bytes_downloaded),
        bytes_total: bytes_total.max(bytes_downloaded),
        files_completed: file_index + 1,
        files_total,
    });

    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    let name = tmp
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    tmp.set_file_name(format!(".{name}.tmp"));
    tmp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_gguf_file_excludes_mmproj() {
        assert!(is_gguf_file("Qwen3.6-35B-A3B-UD-Q4_K_M.gguf"));
        assert!(is_gguf_file("model-00001-of-00003.gguf"));
        assert!(!is_gguf_file("mmproj-F16.gguf"));
        assert!(!is_gguf_file("model.safetensors"));
        assert!(!is_gguf_file("README.md"));
    }

    #[test]
    fn select_gguf_prefers_q4_k_m_and_groups_shards() {
        let tree = vec![
            HfTreeEntry {
                kind: "file".to_string(),
                path: "Model-UD-Q8_0.gguf".to_string(),
                size: 9,
            },
            HfTreeEntry {
                kind: "file".to_string(),
                path: "Model-UD-Q4_K_M-00001-of-00002.gguf".to_string(),
                size: 5,
            },
            HfTreeEntry {
                kind: "file".to_string(),
                path: "Model-UD-Q4_K_M-00002-of-00002.gguf".to_string(),
                size: 5,
            },
            HfTreeEntry {
                kind: "file".to_string(),
                path: "mmproj-F16.gguf".to_string(),
                size: 1,
            },
        ];
        let picked: Vec<String> = select_gguf_files(tree).into_iter().map(|f| f.path).collect();
        assert_eq!(
            picked,
            vec![
                "Model-UD-Q4_K_M-00001-of-00002.gguf".to_string(),
                "Model-UD-Q4_K_M-00002-of-00002.gguf".to_string(),
            ]
        );
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(is_safe_relative_path("model.gguf"));
        assert!(is_safe_relative_path("weights/model-00001.gguf"));
        assert!(!is_safe_relative_path("../escape.gguf"));
        assert!(!is_safe_relative_path("a/../../etc/passwd"));
        assert!(!is_safe_relative_path("/abs/path.gguf"));
        assert!(!is_safe_relative_path("C:\\windows\\system32"));
        assert!(!is_safe_relative_path(""));
    }

    #[test]
    fn ready_false_without_marker() {
        let tmp = std::env::temp_dir().join("sw-llm-test-empty");
        let entry = registry::default_entry();
        assert!(!model_ready(&tmp, &entry));
    }

    #[test]
    fn detect_local_kind_gguf_only() {
        let gguf_dir = std::env::temp_dir().join("sw-llm-detect-gguf");
        let _ = std::fs::remove_dir_all(&gguf_dir);
        std::fs::create_dir_all(&gguf_dir).unwrap();
        std::fs::write(gguf_dir.join("m-Q4_K_M.gguf"), b"x").unwrap();
        assert!(matches!(
            detect_local_kind(&gguf_dir),
            Some(ModelKind::Gguf { .. })
        ));
        let _ = std::fs::remove_dir_all(&gguf_dir);

        let st_dir = std::env::temp_dir().join("sw-llm-detect-st");
        let _ = std::fs::remove_dir_all(&st_dir);
        std::fs::create_dir_all(&st_dir).unwrap();
        std::fs::write(st_dir.join("config.json"), "{}").unwrap();
        std::fs::write(st_dir.join("model.safetensors"), b"x").unwrap();
        assert!(detect_local_kind(&st_dir).is_none());
        let _ = std::fs::remove_dir_all(&st_dir);
    }

    #[test]
    fn first_gguf_finds_nested_file() {
        let dir = std::env::temp_dir().join("sw-llm-nested-gguf");
        let _ = std::fs::remove_dir_all(&dir);
        let nested = dir.join("weights");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("model-Q4_K_M.gguf"), b"x").unwrap();

        assert!(has_any_gguf(&dir));
        let found = first_gguf(&dir).expect("nested gguf should be found");
        assert_eq!(dir.join(&found), nested.join("model-Q4_K_M.gguf"));
        assert!(matches!(
            detect_local_kind(&dir),
            Some(ModelKind::Gguf { .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_gguf_skips_mmproj_projector() {
        let dir = std::env::temp_dir().join("sw-llm-mmproj-gguf");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mmproj-F16.gguf"), b"x").unwrap();
        std::fs::write(dir.join("model-Q4_K_M.gguf"), b"x").unwrap();

        let found = first_gguf(&dir).expect("model gguf should be found");
        assert_eq!(found, "model-Q4_K_M.gguf");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hf_cache_snapshot_resolves_main_ref() {
        let base = std::env::temp_dir().join("sw-llm-hfcache");
        let _ = std::fs::remove_dir_all(&base);
        let model = base.join("models--unsloth--Qwen3.6-35B-A3B-GGUF");
        std::fs::create_dir_all(model.join("refs")).unwrap();
        let snap = model.join("snapshots").join("abc123");
        std::fs::create_dir_all(&snap).unwrap();
        std::fs::write(model.join("refs").join("main"), "abc123\n").unwrap();
        std::fs::write(snap.join("model-Q4_K_M.gguf"), b"x").unwrap();

        let found = hf_cache_snapshot_in(&[base.clone()], "unsloth/Qwen3.6-35B-A3B-GGUF");
        assert_eq!(found.as_deref(), Some(snap.as_path()));
        assert!(hf_cache_snapshot_in(&[base.clone()], "missing/repo").is_none());
        let _ = std::fs::remove_dir_all(&base);
    }
}
