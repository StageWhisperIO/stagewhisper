use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::app_state::AppState;
use sw_local_llm::types::{
    ChatMessage, GenerationChunk, InferenceParams, LlmError, ModelEntry, ModelKind,
};

const DOWNLOAD_PROGRESS_EVENT: &str = "local-llm-download-progress";
const DOWNLOAD_COMPLETE_EVENT: &str = "local-llm-download-complete";
const DOWNLOAD_ERROR_EVENT: &str = "local-llm-download-error";
const DOWNLOAD_CANCELLED_EVENT: &str = "local-llm-download-cancelled";
const RESPONDER_PREFERENCE_EVENT: &str = "responder-preference-changed";
const STATUS_CHANGED_EVENT: &str = "local-llm-status-changed";
const ENGINE_READINESS_EVENT: &str = "engine-readiness-changed";

#[derive(Default)]
pub struct LocalLlmDownloads {
    cancel: Arc<AtomicBool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalLlmPrefs {
    #[serde(default = "default_primary_responder")]
    pub primary_responder: String,
    #[serde(default = "default_local_llm_model")]
    pub selected_model_id: String,
}

fn default_primary_responder() -> String {
    "external".to_string()
}

fn default_local_llm_model() -> String {
    "gemma-4-e2b-it".to_string()
}

impl Default for LocalLlmPrefs {
    fn default() -> Self {
        Self {
            primary_responder: default_primary_responder(),
            selected_model_id: default_local_llm_model(),
        }
    }
}

impl LocalLlmPrefs {
    pub fn prefers_local(&self) -> bool {
        self.primary_responder == "local"
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LlmModelInfo {
    pub id: String,
    pub repo_id: String,
    pub label: String,
    pub ram_hint_gb: f32,
    pub recommended: bool,
    pub kind: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LlmStatusPayload {
    pub ready: bool,
    pub exists: bool,
    pub model_dir: String,
    pub selected_id: String,
    pub label: String,
    pub primary_responder: String,
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

fn kind_label(kind: &ModelKind) -> String {
    match kind {
        ModelKind::Gguf { .. } => "gguf".to_string(),
    }
}

const LLAMA_SERVER_BIN: &str = if cfg!(windows) {
    "llama-server.exe"
} else {
    "llama-server"
};

fn llama_dir(app: &AppHandle) -> PathBuf {
    if let Ok(custom) = std::env::var("SW_LLAMA_DIR") {
        if !custom.trim().is_empty() {
            return PathBuf::from(custom);
        }
    }
    if let Ok(resource) = app.path().resource_dir() {
        let dir = resource.join("llama");
        if dir.join(LLAMA_SERVER_BIN).exists() {
            return dir;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("sidecar")
        .join("llama")
}

fn sidecar_paths(app: &AppHandle) -> sw_local_llm::SidecarPaths {
    let dir = llama_dir(app);
    sw_local_llm::SidecarPaths {
        server_bin: dir.join(LLAMA_SERVER_BIN),
        lib_dir: dir,
    }
}

fn info_from_entry(entry: &ModelEntry) -> LlmModelInfo {
    LlmModelInfo {
        id: entry.id.clone(),
        repo_id: entry.repo_id.clone(),
        label: entry.label.clone(),
        ram_hint_gb: entry.ram_hint_gb,
        recommended: entry.recommended,
        kind: kind_label(&entry.kind),
    }
}

fn base_dir() -> PathBuf {
    sw_local_llm::default_llm_dir()
}

fn prefs_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.stagewhisper.app")
        .join("local_llm_prefs.json")
}

pub fn load_prefs() -> LocalLlmPrefs {
    std::fs::read_to_string(prefs_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<LocalLlmPrefs>(&raw).ok())
        .unwrap_or_default()
}

fn save_prefs(prefs: &LocalLlmPrefs) -> Result<(), String> {
    let path = prefs_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_string_pretty(prefs).map_err(|e| e.to_string())?;
    std::fs::write(&path, raw).map_err(|e| e.to_string())
}

fn selected_entry(app: &AppHandle) -> ModelEntry {
    let data = app.state::<Mutex<AppState>>();
    let selected_id = data
        .lock()
        .ok()
        .map(|s| s.local_llm_prefs.selected_model_id.clone())
        .unwrap_or_else(|| sw_local_llm::DEFAULT_MODEL_ID.to_string());
    sw_local_llm::resolve(&selected_id).unwrap_or_else(sw_local_llm::default_entry)
}

#[tauri::command]
pub fn list_local_llm_models() -> Vec<LlmModelInfo> {
    sw_local_llm::curated()
        .iter()
        .map(info_from_entry)
        .collect()
}

#[tauri::command]
pub fn get_local_llm_status(
    app: AppHandle,
    data: State<'_, Mutex<AppState>>,
) -> Result<LlmStatusPayload, String> {
    let prefs = {
        let state = data.lock().map_err(|_| "state poisoned".to_string())?;
        state.local_llm_prefs.clone()
    };
    let entry =
        sw_local_llm::resolve(&prefs.selected_model_id).unwrap_or_else(sw_local_llm::default_entry);
    let dir = base_dir();
    let ready = sw_local_llm::model_ready(&dir, &entry);
    let exists = sw_local_llm::model_exists(&dir, &entry);

    let mut changed = false;
    if let Ok(mut state) = data.lock() {
        changed = state.local_llm_ready != ready;
        state.local_llm_ready = ready;
    }
    if changed {
        let _ = app.emit(STATUS_CHANGED_EVENT, ());
        emit_engine_readiness(&app);
    }

    Ok(LlmStatusPayload {
        ready,
        exists,
        model_dir: sw_local_llm::model_dir(&dir, &entry).display().to_string(),
        selected_id: entry.id.clone(),
        label: entry.label.clone(),
        primary_responder: prefs.primary_responder,
    })
}

#[tauri::command]
pub async fn download_local_llm_model(
    app: AppHandle,
    downloads: State<'_, LocalLlmDownloads>,
    model_id_or_repo: String,
    hf_token: Option<String>,
) -> Result<(), String> {
    let entry = sw_local_llm::resolve(&model_id_or_repo)
        .ok_or_else(|| format!("unknown model: {model_id_or_repo}"))?;
    let dir = base_dir();

    if sw_local_llm::model_ready(&dir, &entry) {
        finalize_download(&app, &entry);
        let _ = app.emit(DOWNLOAD_COMPLETE_EVENT, ());
        return Ok(());
    }

    let cancel = downloads.cancel.clone();
    cancel.store(false, Ordering::Relaxed);

    let app_for_progress = app.clone();
    let result =
        sw_local_llm::download_model_files(&dir, &entry, hf_token, &cancel, move |progress| {
            let _ = app_for_progress.emit(
                DOWNLOAD_PROGRESS_EVENT,
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
            finalize_download(&app, &entry);
            let _ = app.emit(DOWNLOAD_COMPLETE_EVENT, ());
            emit_engine_readiness(&app);
            Ok(())
        }
        Err(LlmError::Cancelled) => {
            let _ = sw_local_llm::delete_model(&dir, &entry).await;
            let _ = app.emit(DOWNLOAD_CANCELLED_EVENT, ());
            emit_engine_readiness(&app);
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            let _ = app.emit(DOWNLOAD_ERROR_EVENT, &msg);
            Err(msg)
        }
    }
}

#[tauri::command]
pub fn cancel_local_llm_download(downloads: State<'_, LocalLlmDownloads>) {
    downloads.cancel.store(true, Ordering::Relaxed);
}

fn is_curated(entry: &ModelEntry) -> bool {
    sw_local_llm::curated().iter().any(|m| m.id == entry.id)
}

fn finalize_download(app: &AppHandle, entry: &ModelEntry) {
    if is_curated(entry) {
        mark_ready_if_selected(app, entry);
        return;
    }
    let data = app.state::<Mutex<AppState>>();
    let prefs = {
        let Ok(mut state) = data.lock() else {
            return;
        };
        state.local_llm_prefs.selected_model_id = entry.id.clone();
        state.local_llm_ready = true;
        state.local_llm_prefs.clone()
    };
    let _ = save_prefs(&prefs);
}

fn mark_ready_if_selected(app: &AppHandle, entry: &ModelEntry) {
    let data = app.state::<Mutex<AppState>>();
    let Ok(mut state) = data.lock() else {
        return;
    };
    if state.local_llm_prefs.selected_model_id == entry.id
        || state.local_llm_prefs.selected_model_id == entry.repo_id
    {
        state.local_llm_ready = true;
    }
}

#[tauri::command]
pub async fn delete_local_llm_model(
    app: AppHandle,
    model_id_or_repo: String,
) -> Result<(), String> {
    let entry = sw_local_llm::resolve(&model_id_or_repo)
        .ok_or_else(|| format!("unknown model: {model_id_or_repo}"))?;
    sw_local_llm::delete_model(&base_dir(), &entry)
        .await
        .map_err(|e| e.to_string())?;

    let runtime = app.state::<LocalLlmRuntime>();
    runtime.unload_if(&entry.id).await;

    let prefs = {
        let data = app.state::<Mutex<AppState>>();
        let mut state = data.lock().map_err(|_| "state poisoned".to_string())?;
        let was_selected = state.local_llm_prefs.selected_model_id == entry.id
            || state.local_llm_prefs.selected_model_id == entry.repo_id;
        if was_selected {
            if entry.source == sw_local_llm::ModelSource::Local {
                state.local_llm_prefs.selected_model_id =
                    sw_local_llm::DEFAULT_MODEL_ID.to_string();
                let default_entry = sw_local_llm::resolve(sw_local_llm::DEFAULT_MODEL_ID)
                    .unwrap_or_else(sw_local_llm::default_entry);
                state.local_llm_ready = sw_local_llm::model_ready(&base_dir(), &default_entry);
            } else {
                state.local_llm_ready = false;
            }
        }
        state.local_llm_prefs.clone()
    };
    let _ = save_prefs(&prefs);
    let _ = app.emit(STATUS_CHANGED_EVENT, ());
    emit_engine_readiness(&app);
    Ok(())
}

#[tauri::command]
pub fn set_local_llm_model(
    app: AppHandle,
    data: State<'_, Mutex<AppState>>,
    model_id_or_repo: String,
) -> Result<(), String> {
    let entry = sw_local_llm::resolve(&model_id_or_repo)
        .ok_or_else(|| format!("unknown model: {model_id_or_repo}"))?;
    let ready = sw_local_llm::model_ready(&base_dir(), &entry);

    let prefs = {
        let mut state = data.lock().map_err(|_| "state poisoned".to_string())?;
        state.local_llm_prefs.selected_model_id = entry.id.clone();
        state.local_llm_ready = ready;
        state.local_llm_prefs.clone()
    };
    save_prefs(&prefs)?;
    let _ = app.emit(STATUS_CHANGED_EVENT, ());
    emit_engine_readiness(&app);
    Ok(())
}

fn status_payload(app: &AppHandle) -> Result<LlmStatusPayload, String> {
    let data = app.state::<Mutex<AppState>>();
    let prefs = data
        .lock()
        .map_err(|_| "state poisoned".to_string())?
        .local_llm_prefs
        .clone();
    let entry =
        sw_local_llm::resolve(&prefs.selected_model_id).unwrap_or_else(sw_local_llm::default_entry);
    let dir = base_dir();
    let ready = sw_local_llm::model_ready(&dir, &entry);
    let exists = sw_local_llm::model_exists(&dir, &entry);
    let mut changed = false;
    if let Ok(mut state) = data.lock() {
        changed = state.local_llm_ready != ready;
        state.local_llm_ready = ready;
    }
    if changed {
        let _ = app.emit(STATUS_CHANGED_EVENT, ());
        emit_engine_readiness(app);
    }
    Ok(LlmStatusPayload {
        ready,
        exists,
        model_dir: sw_local_llm::model_dir(&dir, &entry).display().to_string(),
        selected_id: entry.id.clone(),
        label: entry.label.clone(),
        primary_responder: prefs.primary_responder,
    })
}

fn select_local_path(app: &AppHandle, path: &Path) -> Result<LlmStatusPayload, String> {
    let entry = sw_local_llm::resolve(&path.display().to_string()).ok_or_else(|| {
        "That folder has no model we can run. Look for one with a .gguf file inside.".to_string()
    })?;
    let ready = sw_local_llm::model_ready(&base_dir(), &entry);
    let prefs = {
        let data = app.state::<Mutex<AppState>>();
        let mut state = data.lock().map_err(|_| "state poisoned".to_string())?;
        state.local_llm_prefs.selected_model_id = entry.id.clone();
        state.local_llm_ready = ready;
        state.local_llm_prefs.clone()
    };
    save_prefs(&prefs)?;
    let _ = app.emit(STATUS_CHANGED_EVENT, ());
    emit_engine_readiness(app);
    status_payload(app)
}

#[tauri::command]
pub async fn use_local_llm_folder(app: AppHandle) -> Result<Option<LlmStatusPayload>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |choice| {
        let _ = tx.send(choice);
    });
    let Some(file_path) = rx.await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let path = file_path.into_path().map_err(|e| e.to_string())?;
    select_local_path(&app, &path).map(Some)
}

#[tauri::command]
pub fn use_hf_cache_model(
    app: AppHandle,
    repo_id: String,
) -> Result<Option<LlmStatusPayload>, String> {
    match sw_local_llm::hf_cache_snapshot(&repo_id) {
        Some(path) => select_local_path(&app, &path).map(Some),
        None => Ok(None),
    }
}

#[tauri::command]
pub fn get_responder_preference(data: State<'_, Mutex<AppState>>) -> Result<String, String> {
    let state = data.lock().map_err(|_| "state poisoned".to_string())?;
    Ok(state.local_llm_prefs.primary_responder.clone())
}

#[tauri::command]
pub fn set_responder_preference(
    app: AppHandle,
    data: State<'_, Mutex<AppState>>,
    preference: String,
) -> Result<(), String> {
    let normalized = if preference == "local" {
        "local"
    } else {
        "external"
    };
    let prefs = {
        let mut state = data.lock().map_err(|_| "state poisoned".to_string())?;
        state.local_llm_prefs.primary_responder = normalized.to_string();
        state.local_llm_prefs.clone()
    };
    save_prefs(&prefs)?;
    let _ = app.emit(RESPONDER_PREFERENCE_EVENT, normalized);
    Ok(())
}

#[derive(Default)]
struct LoadedEngine {
    model_id: Option<String>,
    engine: Option<sw_local_llm::LocalLlmEngine>,
}

#[derive(Default)]
pub struct LocalLlmRuntime {
    inner: tokio::sync::Mutex<LoadedEngine>,
}

type QueuedFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;
type TurnFailureCallback = Box<dyn FnOnce(String) + Send>;

struct TurnFailureGuard {
    on_incomplete: Option<TurnFailureCallback>,
}

impl TurnFailureGuard {
    fn new(on_incomplete: impl FnOnce(String) + Send + 'static) -> Self {
        Self {
            on_incomplete: Some(Box::new(on_incomplete)),
        }
    }

    fn disarm(mut self) {
        self.on_incomplete = None;
    }

    fn fire(mut self, reason: String) {
        if let Some(on_incomplete) = self.on_incomplete.take() {
            on_incomplete(reason);
        }
    }
}

impl Drop for TurnFailureGuard {
    fn drop(&mut self) {
        if let Some(on_incomplete) = self.on_incomplete.take() {
            on_incomplete("the queued turn ended without completing".to_string());
        }
    }
}

struct QueuedTurn {
    guard: TurnFailureGuard,
    future: QueuedFuture,
}

fn incomplete_reason(err: &tauri::Error) -> String {
    match err {
        tauri::Error::JoinError(join_err) if join_err.is_panic() => {
            "the queued turn panicked".to_string()
        }
        tauri::Error::JoinError(join_err) if join_err.is_cancelled() => {
            "the queued turn was cancelled".to_string()
        }
        other => format!("the queued turn ended unexpectedly: {other}"),
    }
}

#[derive(Debug)]
pub struct LocalTurnQueueClosed;

pub struct LocalTurnQueue {
    tx: tokio::sync::mpsc::UnboundedSender<QueuedTurn>,
}

impl LocalTurnQueue {
    pub fn new() -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<QueuedTurn>();
        tauri::async_runtime::spawn(async move {
            while let Some(QueuedTurn { guard, future }) = rx.recv().await {
                match tauri::async_runtime::spawn(future).await {
                    Ok(()) => guard.disarm(),
                    Err(err) => guard.fire(incomplete_reason(&err)),
                }
            }
        });
        Self { tx }
    }

    pub fn enqueue(
        &self,
        on_incomplete: impl FnOnce(String) + Send + 'static,
        turn: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<(), LocalTurnQueueClosed> {
        let item = QueuedTurn {
            guard: TurnFailureGuard::new(on_incomplete),
            future: Box::pin(turn),
        };
        self.tx.send(item).map_err(|err| {
            err.0.guard.disarm();
            LocalTurnQueueClosed
        })
    }

    pub fn enqueue_for(
        &self,
        _session_id: &str,
        on_incomplete: impl FnOnce(String) + Send + 'static,
        turn: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<(), LocalTurnQueueClosed> {
        self.enqueue(on_incomplete, turn)
    }
}

impl Default for LocalTurnQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalLlmRuntime {
    pub async fn generate<F>(
        &self,
        sidecar: &sw_local_llm::SidecarPaths,
        entry: &ModelEntry,
        system: Option<&str>,
        history: &[ChatMessage],
        params: &InferenceParams,
        on_token: F,
    ) -> Result<String, LlmError>
    where
        F: FnMut(GenerationChunk) + Send,
    {
        let mut on_token = on_token;
        let mut guard = self.inner.lock().await;

        let alive = match guard.engine.as_mut() {
            Some(engine) => engine.is_alive(),
            None => false,
        };
        if !alive || guard.model_id.as_deref() != Some(entry.id.as_str()) {
            guard.engine = None;
            guard.model_id = None;
            let dir = sw_local_llm::model_dir(&sw_local_llm::default_llm_dir(), entry);
            let engine = sw_local_llm::LocalLlmEngine::load(sidecar, &dir, entry).await?;
            guard.engine = Some(engine);
            guard.model_id = Some(entry.id.clone());
        }

        let emitted = std::sync::atomic::AtomicBool::new(false);
        let first = {
            let engine = guard
                .engine
                .as_ref()
                .ok_or_else(|| LlmError::Load("engine not loaded".to_string()))?;
            let mut tracked = |chunk: GenerationChunk| {
                if !chunk.done && !chunk.text.is_empty() {
                    emitted.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                on_token(chunk);
            };
            engine
                .infer_messages(system, history, params, &mut tracked)
                .await
        };

        match first {
            Ok(text) => Ok(text),
            Err(err) => {
                if matches!(err, LlmError::Timeout(_)) {
                    guard.engine = None;
                    guard.model_id = None;
                    return Err(err);
                }
                let retryable = matches!(err, LlmError::Inference(_))
                    && !emitted.load(std::sync::atomic::Ordering::Relaxed);
                if !retryable {
                    return Err(err);
                }
                guard.engine = None;
                guard.model_id = None;
                let dir = sw_local_llm::model_dir(&sw_local_llm::default_llm_dir(), entry);
                let engine = sw_local_llm::LocalLlmEngine::load(sidecar, &dir, entry).await?;
                guard.engine = Some(engine);
                guard.model_id = Some(entry.id.clone());
                let engine = guard
                    .engine
                    .as_ref()
                    .ok_or_else(|| LlmError::Load("engine not loaded".to_string()))?;
                engine
                    .infer_messages(system, history, params, &mut on_token)
                    .await
            }
        }
    }

    pub async fn unload_if(&self, model_id: &str) {
        let mut guard = self.inner.lock().await;
        if guard.model_id.as_deref() == Some(model_id) {
            guard.engine = None;
            guard.model_id = None;
        }
    }
}

pub fn local_ready(app: &AppHandle) -> bool {
    let data = app.state::<Mutex<AppState>>();
    data.lock().map(|s| s.local_llm_ready).unwrap_or(false)
}

pub fn prefers_local(app: &AppHandle) -> bool {
    let data = app.state::<Mutex<AppState>>();
    data.lock()
        .map(|s| s.local_llm_prefs.prefers_local())
        .unwrap_or(false)
}

pub fn stt_ready() -> bool {
    sw_audio_recording::download::models_ready(&sw_audio_recording::download::default_model_dir())
}

pub fn engine_ready(app: &AppHandle) -> bool {
    let relay = app
        .state::<Arc<crate::relay_settings::RelaySettingsStore>>()
        .snapshot();
    crate::state::session::can_listen(&relay, local_ready(app), prefers_local(app), stt_ready())
}

pub fn emit_engine_readiness(app: &AppHandle) {
    let _ = app.emit(ENGINE_READINESS_EVENT, engine_ready(app));
}

pub async fn generate_reply(
    app: &AppHandle,
    system: Option<&str>,
    prompt: &str,
) -> Result<String, String> {
    generate_reply_messages(app, system, &[ChatMessage::user(prompt)]).await
}

pub async fn generate_reply_messages(
    app: &AppHandle,
    system: Option<&str>,
    history: &[ChatMessage],
) -> Result<String, String> {
    let entry = selected_entry(app);
    let sidecar = sidecar_paths(app);
    let runtime = app.state::<LocalLlmRuntime>();
    let params = InferenceParams::default();
    runtime
        .generate(&sidecar, &entry, system, history, &params, |_chunk| {})
        .await
        .map_err(|e| e.to_string())
}

pub async fn generate_reply_streaming(
    app: &AppHandle,
    system: Option<&str>,
    prompt: &str,
    on_token: impl FnMut(GenerationChunk) + Send,
) -> Result<String, String> {
    generate_reply_messages_streaming(app, system, &[ChatMessage::user(prompt)], on_token).await
}

pub async fn generate_reply_messages_streaming(
    app: &AppHandle,
    system: Option<&str>,
    history: &[ChatMessage],
    on_token: impl FnMut(GenerationChunk) + Send,
) -> Result<String, String> {
    let entry = selected_entry(app);
    let sidecar = sidecar_paths(app);
    let runtime = app.state::<LocalLlmRuntime>();
    let params = InferenceParams::default();
    runtime
        .generate(&sidecar, &entry, system, history, &params, on_token)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "local_llm_queue_tests.rs"]
mod queue_tests;
