use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use sw_audio_recording::download::default_model_dir;
use sw_audio_recording::pipeline::{LocalPipeline, PipelineEvent};
use sw_audio_recording::vad::VadConfig;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let wav_path = PathBuf::from(
        args.next()
            .expect("usage: stt_latency <wav-16k-mono> [speed] [silence-ms]"),
    );
    let speed: f32 = args.next().map(|s| s.parse().unwrap()).unwrap_or(1.0);
    let silence_ms: Option<u64> = args.next().map(|s| s.parse().unwrap());

    let mut reader = hound::WavReader::open(&wav_path)?;
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 16000, "expected 16 kHz wav");
    assert_eq!(spec.channels, 1, "expected mono wav");
    let samples: Vec<i16> = reader.samples::<i16>().collect::<Result<_, _>>()?;
    eprintln!(
        "[stt-harness] {} samples ({:.1}s) speed {speed}x",
        samples.len(),
        samples.len() as f32 / 16000.0
    );

    let cancelled = AtomicBool::new(false);
    let load_started = Instant::now();
    let mut pipeline = LocalPipeline::load(&default_model_dir(), true, false, &cancelled)?;
    eprintln!(
        "[stt-harness] pipeline loaded in {:.1}s",
        load_started.elapsed().as_secs_f32()
    );
    if let Some(ms) = silence_ms {
        pipeline.set_vad_config(VadConfig {
            silence_ms: ms,
            ..VadConfig::default()
        });
        eprintln!("[stt-harness] vad silence_ms override: {ms}");
    }

    let chunk = 1600;
    let started = Instant::now();
    for (i, block) in samples.chunks(chunk).enumerate() {
        let audio_pos_s = (i * chunk) as f32 / 16000.0;
        let deadline = started + Duration::from_secs_f32(audio_pos_s / speed);
        if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        for event in pipeline.process_audio_chunk(block) {
            match event {
                PipelineEvent::Transcript { text, is_final } => {
                    eprintln!(
                        "[stt-harness] transcript final={is_final} audio_pos={audio_pos_s:.2}s wall={:.2}s chars={} text={:?}",
                        started.elapsed().as_secs_f32(),
                        text.len(),
                        text
                    );
                }
                PipelineEvent::SpeechStart => {
                    eprintln!("[stt-harness] speech_start audio_pos={audio_pos_s:.2}s");
                }
                PipelineEvent::SpeechEnd => {
                    eprintln!("[stt-harness] speech_end audio_pos={audio_pos_s:.2}s");
                }
            }
        }
    }
    for event in pipeline.flush() {
        if let PipelineEvent::Transcript { text, is_final } = event {
            eprintln!(
                "[stt-harness] flush transcript final={is_final} chars={} text={:?}",
                text.len(),
                text
            );
        }
    }
    Ok(())
}
