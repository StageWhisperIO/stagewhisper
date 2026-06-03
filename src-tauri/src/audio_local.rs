use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    sync::mpsc,
    sync::Arc,
    thread,
    time::Duration,
};

use serde_json::json;
use sw_audio_recording::pipeline::{LocalPipeline, PipelineEvent};
use sw_audio_recording::{build_mic_input_stream, build_system_output_stream_streaming};
use sw_notes::{TranscriptAccumulator, TranscriptSource};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::panels::SESSION_LABEL;
use crate::relay::RelayClient;
use crate::state::app_state::{AppState, PipelineMode};
use crate::{
    app_settings::AppSettingsStore,
    audio::{reset_session_state_keep_panel, AudioFrame, AudioStopReason, AudioStopSignal},
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const PIPELINE_LOAD_TIMEOUT: Duration = Duration::from_secs(120);
const AUDIO_BUFFER_CAPACITY: usize = 8192;

static PIPELINE_LOAD_ACTIVE: AtomicBool = AtomicBool::new(false);

struct PipelineLoadGuard;
impl PipelineLoadGuard {
    fn acquire() -> Option<Self> {
        PIPELINE_LOAD_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self)
    }
}
impl Drop for PipelineLoadGuard {
    fn drop(&mut self) {
        PIPELINE_LOAD_ACTIVE.store(false, Ordering::Release);
    }
}

pub fn start_local_capture(app: AppHandle) -> Result<AudioStopSignal, String> {
    let (stop_tx, stop_rx) = mpsc::channel::<AudioStopReason>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let app_for_thread = app.clone();
    let session_started_at_ms = now_ms();

    let session_id = {
        let relay = app.state::<Arc<RwLock<RelayClient>>>().inner().clone();
        tauri::async_runtime::block_on(async move { relay.write().await.reset_session() })
    };
    let session_id_str = session_id.to_string();

    {
        let data = app.state::<std::sync::Mutex<AppState>>();
        let mut state = data.lock().unwrap();
        state.pipeline_mode = PipelineMode::Local;
        state.session_id = Some(session_id_str.clone());
    }

    let _ = app.emit("current-session-changed", Some(session_id_str));

    thread::Builder::new()
        .name("sw-local-capture".to_string())
        .spawn(move || {
            let mut startup_notified = false;
            let result = run_local_capture(
                &app_for_thread,
                &stop_rx,
                &ready_tx,
                &mut startup_notified,
                session_started_at_ms,
                session_id,
            );
            if let Err(err) = result {
                if !startup_notified {
                    let _ = ready_tx.send(Err(err.clone()));
                }
                eprintln!("[audio-local] error: {err}");
                let _ = app_for_thread.emit("pipeline-load-error", err);
            }
            reset_session_state_keep_panel(&app_for_thread);
        })
        .map_err(|err| format!("failed to spawn local capture thread: {err}"))?;

    match ready_rx.recv_timeout(STARTUP_TIMEOUT) {
        Ok(Ok(())) => Ok(stop_tx),
        Ok(Err(err)) => Err(err),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let _ = stop_tx.send(AudioStopReason::Complete);
            Err("timed out while starting local capture".to_string())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("local capture thread exited before startup confirmation".to_string())
        }
    }
}

fn run_local_capture(
    app: &AppHandle,
    stop_rx: &mpsc::Receiver<AudioStopReason>,
    ready_tx: &mpsc::Sender<Result<(), String>>,
    startup_notified: &mut bool,
    session_started_at_ms: u64,
    session_id: Uuid,
) -> Result<(), String> {
    let model_dir = sw_audio_recording::download::default_model_dir();
    let session_window = app.get_webview_window(SESSION_LABEL);

    if let Some(ref win) = session_window {
        let _ = win.emit("pipeline-loading", true);
    }

    let load_guard = PipelineLoadGuard::acquire()
        .ok_or_else(|| "local pipeline load already in progress".to_string())?;

    let mic_enabled = app.state::<Arc<AppSettingsStore>>().snapshot().mic_enabled;
    let (audio_tx, audio_rx) = crossbeam_channel::bounded::<AudioFrame>(AUDIO_BUFFER_CAPACITY);
    let mic_audio_tx = if mic_enabled {
        Some(audio_tx.clone())
    } else {
        None
    };
    let dropped_chunks = Arc::new(AtomicUsize::new(0));
    let dropped_chunks_cb = Arc::clone(&dropped_chunks);

    let mut stream = build_system_output_stream_streaming(move |chunk: Vec<i16>| {
        if audio_tx.try_send(AudioFrame::System(chunk)).is_err() {
            dropped_chunks_cb.fetch_add(1, Ordering::Relaxed);
        }
    })
    .map_err(|err| format!("failed to build output stream: {err}"))?;

    stream
        .start()
        .map_err(|err| format!("failed to start output stream: {err}"))?;

    let _ = ready_tx.send(Ok(()));
    *startup_notified = true;

    let (load_tx, load_rx) = std::sync::mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_clone = Arc::clone(&cancelled);
    let model_dir_clone = model_dir.clone();
    thread::spawn(move || {
        let _guard = load_guard;
        let result = LocalPipeline::load(&model_dir_clone, true, false, &cancelled_clone);
        let _ = load_tx.send(result);
    });

    let load_deadline = std::time::Instant::now() + PIPELINE_LOAD_TIMEOUT;
    let mut pipeline = loop {
        if let Ok(reason) = stop_rx.try_recv() {
            cancelled.store(true, Ordering::Release);
            let _ = stream.stop();
            if let Some(ref win) = session_window {
                let _ = win.emit("pipeline-loading", false);
            }
            eprintln!("[audio-local] stop during load (reason={reason:?})");
            return Ok(());
        }

        let remaining = load_deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            cancelled.store(true, Ordering::Release);
            return Err(format!(
                "local pipeline load timed out after {}s",
                PIPELINE_LOAD_TIMEOUT.as_secs()
            ));
        }

        let poll_interval = remaining.min(Duration::from_millis(200));
        match load_rx.recv_timeout(poll_interval) {
            Ok(Ok(pipeline)) => break pipeline,
            Ok(Err(e)) => return Err(format!("pipeline load failed: {e}")),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err("pipeline load thread terminated unexpectedly".to_string());
            }
        }
    };

    let mut mic_pipeline = if mic_enabled {
        match LocalPipeline::load_with_shared_model(
            &model_dir,
            pipeline.shared_model(),
            false,
            &cancelled,
        ) {
            Ok(pipeline) => Some(pipeline),
            Err(e) => {
                eprintln!("[audio-local] mic pipeline load failed: {e}");
                None
            }
        }
    } else {
        None
    };

    let _mic_stream = match (mic_pipeline.is_some(), mic_audio_tx) {
        (true, Some(tx)) => match build_mic_input_stream(move |chunk: Vec<i16>| {
            let _ = tx.try_send(AudioFrame::Mic(chunk));
        }) {
            Ok(stream) => Some(stream),
            Err(e) => {
                eprintln!("[audio-local] mic stream unavailable: {e}");
                mic_pipeline = None;
                None
            }
        },
        _ => None,
    };

    if let Some(ref win) = session_window {
        let _ = win.emit("pipeline-loading", false);
    }
    let _ = app.emit("pipeline-mode-changed", "local");

    let mut accumulator = TranscriptAccumulator::new();

    loop {
        match stop_rx.try_recv() {
            Ok(reason) => {
                eprintln!("[audio-local] stop signal received (reason={reason:?})");
                let _ = stream.stop();
                while let Ok(frame) = audio_rx.try_recv() {
                    match frame {
                        AudioFrame::System(chunk) => {
                            let events = pipeline.process_audio_chunk(&chunk);
                            dispatch_events(
                                app,
                                &events,
                                &mut accumulator,
                                TranscriptSource::Others,
                            );
                        }
                        AudioFrame::Mic(chunk) => {
                            if let Some(ref mut mic) = mic_pipeline {
                                let events = mic.process_audio_chunk(&chunk);
                                dispatch_events(
                                    app,
                                    &events,
                                    &mut accumulator,
                                    TranscriptSource::You,
                                );
                            }
                        }
                    }
                }
                let sys_flush_events = pipeline.flush();
                dispatch_events(
                    app,
                    &sys_flush_events,
                    &mut accumulator,
                    TranscriptSource::Others,
                );

                if let Some(ref mut mic) = mic_pipeline {
                    let mic_flush_events = mic.flush();
                    dispatch_events(
                        app,
                        &mic_flush_events,
                        &mut accumulator,
                        TranscriptSource::You,
                    );
                }
                if matches!(reason, AudioStopReason::Complete) && !accumulator.is_empty() {
                    crate::notes::finalize_session(
                        app.clone(),
                        session_id,
                        accumulator.into_segments(),
                        session_started_at_ms,
                        now_ms(),
                    );
                }
                return Ok(());
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                let _ = stream.stop();
                return Ok(());
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }

        match audio_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => match frame {
                AudioFrame::System(chunk) => {
                    let events = pipeline.process_audio_chunk(&chunk);
                    dispatch_events(app, &events, &mut accumulator, TranscriptSource::Others);
                }
                AudioFrame::Mic(chunk) => {
                    if let Some(ref mut mic) = mic_pipeline {
                        let events = mic.process_audio_chunk(&chunk);
                        dispatch_events(app, &events, &mut accumulator, TranscriptSource::You);
                    }
                }
            },
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                let _ = stream.stop();
                return Ok(());
            }
        }
    }
}

fn dispatch_events(
    app: &AppHandle,
    events: &[PipelineEvent],
    accumulator: &mut TranscriptAccumulator,
    source: TranscriptSource,
) {
    let session_window = app.get_webview_window(SESSION_LABEL);
    for event in events {
        match event {
            PipelineEvent::SpeechStart => {
                if let Some(ref win) = session_window {
                    let _ = win.emit("vad-status", "speech-start");
                }
            }
            PipelineEvent::SpeechEnd => {
                if let Some(ref win) = session_window {
                    let _ = win.emit("vad-status", "speech-end");
                }
            }
            PipelineEvent::Transcript { text, is_final } => {
                if let Some(ref win) = session_window {
                    if *is_final {
                        let _ = win.emit(
                            "transcript-updated",
                            json!({
                                "kind": "input",
                                "text": "",
                                "finished": true,
                                "source": source,
                            }),
                        );
                    } else {
                        let _ = win.emit(
                            "transcript-updated",
                            json!({
                                "kind": "input",
                                "text": text,
                                "finished": false,
                                "source": source,
                            }),
                        );
                    }
                }

                if !*is_final {
                    continue;
                }

                accumulator.push_final(source, text);
            }
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
