use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    sync::mpsc,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use serde_json::json;
use sw_audio_recording::pipeline::{LocalPipeline, PipelineEvent};
use sw_audio_recording::aec::StreamingEchoCanceller;
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
#[cfg(feature = "diarization")]
const MAX_OTHERS_SAMPLES: usize = 16_000 * 30;

#[derive(Default)]
struct EchoAudioCollector {
    #[cfg(feature = "diarization")]
    others_audio: Vec<f32>,
    #[cfg(feature = "diarization")]
    pending_utterances: Vec<(usize, Vec<f32>)>,
    #[cfg(feature = "diarization")]
    others_speech_active: bool,
}

impl EchoAudioCollector {
    fn new() -> Self {
        Self::default()
    }

    #[cfg(feature = "diarization")]
    fn push_others(&mut self, chunk: &[i16]) {
        self.others_audio
            .extend(chunk.iter().map(|s| *s as f32 / 32768.0));
        if self.others_audio.len() > MAX_OTHERS_SAMPLES {
            let excess = self.others_audio.len() - MAX_OTHERS_SAMPLES;
            self.others_audio.drain(..excess);
        }
    }

    #[cfg(feature = "diarization")]
    fn snapshot_others(&mut self, index: usize) {
        let samples = std::mem::take(&mut self.others_audio);
        if !samples.is_empty() {
            self.pending_utterances.push((index, samples));
        }
    }

}

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
            let mut accumulator = TranscriptAccumulator::new();
            let mut collector = EchoAudioCollector::new();
            let mut panic_retries: u32 = 0;
            let capture_session = LocalCaptureSession {
                started_at_ms: session_started_at_ms,
                session_id,
            };
            let result = loop {
                let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_local_capture(
                        &app_for_thread,
                        &stop_rx,
                        &ready_tx,
                        &mut startup_notified,
                        &capture_session,
                        &mut accumulator,
                        &mut collector,
                    )
                }));
                match attempt {
                    Ok(result) => break result,
                    Err(panic) => {
                        let message = crate::audio::panic_message(panic.as_ref());
                        eprintln!("[audio-local] capture thread panicked: {message}");
                        panic_retries += 1;
                        if panic_retries > crate::audio::CAPTURE_PANIC_MAX_RETRIES {
                            break Err(format!("local capture crashed repeatedly: {message}"));
                        }
                        eprintln!(
                            "[audio-local] resuming capture after panic (attempt {panic_retries}/{})",
                            crate::audio::CAPTURE_PANIC_MAX_RETRIES
                        );
                        thread::sleep(crate::audio::CAPTURE_PANIC_RETRY_DELAY);
                    }
                }
            };
            if let Err(err) = result {
                if !startup_notified {
                    let _ = ready_tx.send(Err(err.clone()));
                }
                eprintln!("[audio-local] error: {err}");
                let _ = app_for_thread.emit("pipeline-load-error", err);
                if !accumulator.is_empty() {
                    crate::notes::finalize_session(
                        app_for_thread.clone(),
                        session_id,
                        accumulator.into_segments(),
                        session_started_at_ms,
                        now_ms(),
                    );
                }
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

fn start_system_stream(
    audio_tx: crossbeam_channel::Sender<AudioFrame>,
    dropped_chunks: Arc<AtomicUsize>,
) -> Result<sw_audio_recording::RUHear, String> {
    let mut stream = build_system_output_stream_streaming(move |chunk: Vec<i16>| {
        if audio_tx
            .try_send(AudioFrame::System(chunk, Instant::now()))
            .is_err()
        {
            let dropped = dropped_chunks.fetch_add(1, Ordering::Relaxed) + 1;
            if dropped % 50 == 1 {
                eprintln!("[audio-local] system capture queue full; dropped {dropped} chunks");
            }
        }
    })
    .map_err(|err| format!("failed to build output stream: {err}"))?;

    stream
        .start()
        .map_err(|err| format!("failed to start output stream: {err}"))?;

    Ok(stream)
}

struct LocalCaptureSession {
    started_at_ms: u64,
    session_id: Uuid,
}

fn run_local_capture(
    app: &AppHandle,
    stop_rx: &mpsc::Receiver<AudioStopReason>,
    ready_tx: &mpsc::Sender<Result<(), String>>,
    startup_notified: &mut bool,
    session: &LocalCaptureSession,
    accumulator: &mut TranscriptAccumulator,
    collector: &mut EchoAudioCollector,
) -> Result<(), String> {
    let session_started_at_ms = session.started_at_ms;
    let session_id = session.session_id;
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

    let mut stream = start_system_stream(audio_tx.clone(), Arc::clone(&dropped_chunks))?;

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

    let mut echo_canceller = if mic_pipeline.is_some() {
        StreamingEchoCanceller::new()
            .map_err(|e| eprintln!("[audio-local] echo canceller unavailable: {e}"))
            .ok()
    } else {
        None
    };

    let dropped_mic = Arc::new(AtomicUsize::new(0));
    let _mic_stream = match (mic_pipeline.is_some(), mic_audio_tx) {
        (true, Some(tx)) => {
            let dropped_mic_cb = Arc::clone(&dropped_mic);
            match build_mic_input_stream(move |chunk: Vec<i16>| {
                if tx
                    .try_send(AudioFrame::Mic(chunk, Instant::now()))
                    .is_err()
                {
                    let dropped = dropped_mic_cb.fetch_add(1, Ordering::Relaxed) + 1;
                    if dropped % 50 == 1 {
                        eprintln!("[audio-local] microphone capture queue full; dropped {dropped} chunks");
                    }
                }
            }) {
                Ok(stream) => Some(stream),
                Err(e) => {
                    eprintln!("[audio-local] mic stream unavailable: {e}");
                    mic_pipeline = None;
                    None
                }
            }
        }
        _ => None,
    };

    if let Some(ref win) = session_window {
        let _ = win.emit("pipeline-loading", false);
    }
    let _ = app.emit("pipeline-mode-changed", "local");

    let mut last_system_audio = Instant::now();
    let mut last_any_audio = Instant::now();
    let mut stall_restarts: u32 = 0;
    let mut system_capture_abandoned = false;

    let reason = loop {
        match stop_rx.try_recv() {
            Ok(reason) => {
                eprintln!("[audio-local] stop signal received (reason={reason:?})");
                break Some(reason);
            }
            Err(mpsc::TryRecvError::Disconnected) => break None,
            Err(mpsc::TryRecvError::Empty) => {}
        }

        match audio_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => match frame {
                AudioFrame::System(chunk, captured_at) => {
                    last_system_audio = Instant::now();
                    last_any_audio = Instant::now();
                    stall_restarts = 0;
                    if system_capture_abandoned {
                        system_capture_abandoned = false;
                        eprintln!("[audio-local] system audio recovered; resuming stall monitoring");
                    }
                    if let Some(ref mut ec) = echo_canceller {
                        ec.push_far_end(&chunk, captured_at);
                    }
                    let events = pipeline.process_audio_chunk(&chunk);
                    dispatch_events(
                        app,
                        &events,
                        accumulator,
                        collector,
                        TranscriptSource::Others,
                    );
                    #[cfg(feature = "diarization")]
                    if collector.others_speech_active {
                        collector.push_others(&chunk);
                    }
                }
                AudioFrame::Mic(chunk, captured_at) => {
                    last_any_audio = Instant::now();
                    if let Some(ref mut mic) = mic_pipeline {
                        let near = match echo_canceller {
                            Some(ref mut ec) => {
                                ec.push_near_end(&chunk, captured_at);
                                ec.drain_cleaned()
                            }
                            None => chunk,
                        };
                        if !near.is_empty() {
                            let events = mic.process_audio_chunk(&near);
                            dispatch_events(
                                app,
                                &events,
                                accumulator,
                                collector,
                                TranscriptSource::You,
                            );
                        }
                    }
                }
            },
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break None,
        }

        if system_capture_abandoned {
            if last_any_audio.elapsed() >= crate::audio::SYSTEM_AUDIO_STALL_TIMEOUT {
                eprintln!(
                    "[audio-local] audio fully stalled after system capture was abandoned; ending session"
                );
                let _ = app.emit(
                    "pipeline-load-error",
                    "Audio capture stopped and could not be restarted. The session was ended and your transcript saved.",
                );
                break Some(AudioStopReason::Complete);
            }
        } else if last_system_audio.elapsed() >= crate::audio::SYSTEM_AUDIO_STALL_TIMEOUT {
            if stall_restarts >= crate::audio::STALL_RESTART_MAX_ATTEMPTS {
                if last_any_audio.elapsed() >= crate::audio::SYSTEM_AUDIO_STALL_TIMEOUT {
                    eprintln!(
                        "[audio-local] system audio stalled; ending session after {stall_restarts} failed restarts"
                    );
                    let _ = app.emit(
                        "pipeline-load-error",
                        "System audio capture stopped and could not be restarted. The session was ended and your transcript saved.",
                    );
                    break Some(AudioStopReason::Complete);
                }
                system_capture_abandoned = true;
                eprintln!(
                    "[audio-local] system audio stalled after {stall_restarts} failed restarts; microphone still active — continuing mic-only"
                );
                let _ = app.emit(
                    "pipeline-load-error",
                    "System audio capture stopped and could not be restarted. The session continues with your microphone only.",
                );
            } else {
                stall_restarts += 1;
                eprintln!(
                    "[audio-local] no system audio for {}s; restarting capture stream (attempt {stall_restarts}/{})",
                    crate::audio::SYSTEM_AUDIO_STALL_TIMEOUT.as_secs(),
                    crate::audio::STALL_RESTART_MAX_ATTEMPTS
                );
                let _ = stream.stop();
                match start_system_stream(audio_tx.clone(), Arc::clone(&dropped_chunks)) {
                    Ok(new_stream) => stream = new_stream,
                    Err(err) => {
                        eprintln!("[audio-local] capture stream restart failed: {err}")
                    }
                }
                last_system_audio = Instant::now();
            }
        }
    };

    let _ = stream.stop();
    let Some(reason) = reason else {
        return Ok(());
    };
    while let Ok(frame) = audio_rx.try_recv() {
        match frame {
            AudioFrame::System(chunk, captured_at) => {
                if let Some(ref mut ec) = echo_canceller {
                    ec.push_far_end(&chunk, captured_at);
                }
                let events = pipeline.process_audio_chunk(&chunk);
                dispatch_events(
                    app,
                    &events,
                    accumulator,
                    collector,
                    TranscriptSource::Others,
                );
                #[cfg(feature = "diarization")]
                if collector.others_speech_active {
                    collector.push_others(&chunk);
                }
            }
            AudioFrame::Mic(chunk, captured_at) => {
                if let Some(ref mut mic) = mic_pipeline {
                    let near = match echo_canceller {
                        Some(ref mut ec) => {
                            ec.push_near_end(&chunk, captured_at);
                            ec.drain_cleaned()
                        }
                        None => chunk,
                    };
                    if !near.is_empty() {
                        let events = mic.process_audio_chunk(&near);
                        dispatch_events(
                            app,
                            &events,
                            accumulator,
                            collector,
                            TranscriptSource::You,
                        );
                    }
                }
            }
        }
    }
    let sys_flush_events = pipeline.flush();
    dispatch_events(
        app,
        &sys_flush_events,
        accumulator,
        collector,
        TranscriptSource::Others,
    );

    if let Some(ref mut mic) = mic_pipeline {
        if let Some(ref mut ec) = echo_canceller {
            let near = ec.drain_remaining();
            if !near.is_empty() {
                let events = mic.process_audio_chunk(&near);
                dispatch_events(
                    app,
                    &events,
                    accumulator,
                    collector,
                    TranscriptSource::You,
                );
            }
        }
        let mic_flush_events = mic.flush();
        dispatch_events(
            app,
            &mic_flush_events,
            accumulator,
            collector,
            TranscriptSource::You,
        );
    }
    if matches!(reason, AudioStopReason::Complete) && !accumulator.is_empty() {
        #[allow(unused_mut)]
        let mut segments =
            std::mem::replace(accumulator, TranscriptAccumulator::new()).into_segments();
        #[cfg(feature = "diarization")]
        {
            if let Err(err) = crate::ensure_session_storage(app) {
                eprintln!("[audio-local] session storage unavailable for diarization: {err}");
            }
            relabel_segments(app, &mut segments, collector);
        }
        crate::notes::finalize_session(
            app.clone(),
            session_id,
            segments,
            session_started_at_ms,
            now_ms(),
        );
    }
    Ok(())
}

fn dispatch_events(
    app: &AppHandle,
    events: &[PipelineEvent],
    accumulator: &mut TranscriptAccumulator,
    #[allow(unused_variables)] collector: &mut EchoAudioCollector,
    source: TranscriptSource,
) {
    let session_window = app.get_webview_window(SESSION_LABEL);
    for event in events {
        match event {
            PipelineEvent::SpeechStart => {
                #[cfg(feature = "diarization")]
                if matches!(source, TranscriptSource::Others) {
                    collector.others_speech_active = true;
                }
                if let Some(ref win) = session_window {
                    let _ = win.emit("vad-status", "speech-start");
                }
            }
            PipelineEvent::SpeechEnd => {
                #[cfg(feature = "diarization")]
                if matches!(source, TranscriptSource::Others) {
                    collector.others_speech_active = false;
                }
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

                #[cfg(feature = "diarization")]
                let len_before = accumulator.segments().len();
                accumulator.push_final(source, text);
                #[cfg(feature = "diarization")]
                if accumulator.segments().len() > len_before {
                    let index = accumulator.segments().len() - 1;
                    match source {
                        TranscriptSource::Others => collector.snapshot_others(index),
                        TranscriptSource::You => {}
                    }
                }
            }
        }
    }
}

#[cfg(feature = "diarization")]
fn relabel_segments(
    app: &AppHandle,
    segments: &mut Vec<sw_notes::accumulate::TranscriptSegment>,
    collector: &mut EchoAudioCollector,
) {
    let pending_utterances = std::mem::take(&mut collector.pending_utterances);

    if pending_utterances.is_empty() {
        return;
    }
    let mut diarizer = match crate::diarization::open_diarizer(app) {
        Ok(diarizer) => diarizer,
        Err(err) => {
            eprintln!("[audio-local] diarization unavailable: {err}");
            return;
        }
    };
    let utterances: Vec<sw_audio_recording::diarization::Utterance> = pending_utterances
        .iter()
        .map(|(index, samples)| sw_audio_recording::diarization::Utterance {
            index: *index,
            samples: samples.clone(),
        })
        .collect();
    let assignments = match diarizer.assign_speakers(&utterances) {
        Ok(assignments) => assignments,
        Err(err) => {
            eprintln!("[audio-local] speaker assignment failed: {err}");
            return;
        }
    };
    apply_assignments(segments, assignments);
}

#[cfg(feature = "diarization")]
fn apply_assignments(
    segments: &mut [sw_notes::accumulate::TranscriptSegment],
    assignments: Vec<sw_audio_recording::diarization::SpeakerAssignment>,
) {
    for assignment in assignments {
        if let Some(segment) = segments.get_mut(assignment.index) {
            segment.speaker_id = Some(assignment.speaker_id);
            segment.speaker_label = assignment.speaker_label;
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
