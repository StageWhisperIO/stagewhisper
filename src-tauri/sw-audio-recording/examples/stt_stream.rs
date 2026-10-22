use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sw_audio_recording::download::default_model_dir;
use sw_audio_recording::pipeline::{LocalPipeline, PipelineEvent};

const SAMPLE_RATE: f64 = 16000.0;
const BLOCK_SAMPLES: usize = 512;

fn emit(out: &mut impl Write, line: Value) -> std::io::Result<()> {
    writeln!(out, "{line}")?;
    out.flush()
}

fn report(
    out: &mut impl Write,
    events: Vec<PipelineEvent>,
    samples_seen: usize,
) -> std::io::Result<()> {
    let position = samples_seen as f64 / SAMPLE_RATE;
    for event in events {
        let line = match event {
            PipelineEvent::SpeechStart => json!({"type": "speech_start", "position": position}),
            PipelineEvent::SpeechEnd => json!({"type": "speech_end", "position": position}),
            PipelineEvent::Transcript { text, is_final } => json!({
                "type": "transcript",
                "final": is_final,
                "text": text,
                "position": position,
            }),
        };
        emit(out, line)?;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let model_dir = std::env::var_os("SW_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(default_model_dir);
    let use_accel = std::env::var_os("SW_STT_CPU").is_none();
    let cancelled = AtomicBool::new(false);
    let loading = Instant::now();
    let mut pipeline = LocalPipeline::load(&model_dir, use_accel, false, &cancelled)?;
    let mut out = std::io::stdout().lock();
    emit(
        &mut out,
        json!({"type": "ready", "load_seconds": loading.elapsed().as_secs_f64()}),
    )?;

    let mut input = std::io::stdin().lock();
    let mut bytes = vec![0u8; BLOCK_SAMPLES * 2];
    let mut samples_seen = 0usize;
    let mut compute = Duration::ZERO;
    loop {
        match input.read_exact(&mut bytes) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.into()),
        }
        let block: Vec<i16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| i16::from_le_bytes(*pair))
            .collect();
        samples_seen += block.len();
        let started = Instant::now();
        let events = pipeline.process_audio_chunk(&block);
        compute += started.elapsed();
        report(&mut out, events, samples_seen)?;
    }

    let started = Instant::now();
    let events = pipeline.flush();
    compute += started.elapsed();
    report(&mut out, events, samples_seen)?;
    emit(
        &mut out,
        json!({"type": "done", "compute_seconds": compute.as_secs_f64(), "accelerated": use_accel}),
    )?;
    Ok(())
}
