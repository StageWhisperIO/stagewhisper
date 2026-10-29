mod align;
mod dump;
mod echo_path;
mod model;
mod onnx;

use crate::reference_gate::ReferenceGate;
use dump::AecDump;
use std::time::{Duration, Instant};

pub(crate) use align::CircularBuffer;
use align::{StreamClock, TimelineAligner, FAR_WAIT_LAG_SAMPLES, SAMPLE_RATE};
use echo_path::{EchoPathDetector, FAR_ACTIVE_RMS};
pub use echo_path::{EchoPathPrior, EchoRegime};
pub use model::BLOCK_SHIFT;
pub use onnx::Aec;

type AecResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const BYPASS_LOG_INTERVAL_SAMPLES: u64 = 16000;
const FAR_LIVENESS_TIMEOUT: Duration = Duration::from_millis(400);
const FAR_SILENCE_SKIP_BLOCKS: u32 = 125;

fn sanitized(samples: &[f32]) -> Vec<f32> {
    samples
        .iter()
        .map(|&v| if v.is_finite() { v } else { 0.0 })
        .collect()
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|&v| v * v).sum();
    (sum / samples.len() as f32).sqrt()
}

pub struct StreamingEchoCanceller {
    aec: Aec,
    aligner: TimelineAligner,
    epoch: Option<Instant>,
    near_clock: StreamClock,
    far_clock: StreamClock,
    bypassed_reported: u64,
    last_far_at: Option<Instant>,
    last_near_at: Option<Instant>,
    aec_failed: bool,
    detector: EchoPathDetector,
    prior: EchoPathPrior,
    regime: EchoRegime,
    far_silent_blocks: u32,
    silence_skipping: bool,
    inference_blocks: u64,
    reference_gate: ReferenceGate,
    dump: Option<AecDump>,
}

impl StreamingEchoCanceller {
    pub fn new() -> AecResult<Self> {
        Self::with_prior(EchoPathPrior::Unknown)
    }

    pub fn with_prior(prior: EchoPathPrior) -> AecResult<Self> {
        let detector = EchoPathDetector::new(prior);
        let regime = detector.regime();
        Ok(Self {
            aec: Aec::new()?,
            aligner: TimelineAligner::new(),
            epoch: None,
            near_clock: StreamClock::new(),
            far_clock: StreamClock::new(),
            bypassed_reported: 0,
            last_far_at: None,
            last_near_at: None,
            aec_failed: false,
            detector,
            prior,
            regime,
            far_silent_blocks: 0,
            silence_skipping: false,
            inference_blocks: 0,
            reference_gate: ReferenceGate::new(),
            dump: AecDump::from_env(),
        })
    }

    pub fn on_route_change(&mut self, prior: EchoPathPrior) {
        log::info!(
            "[aec] output route changed (prior {:?} -> {:?}); resetting echo path state",
            self.prior,
            prior
        );
        self.prior = prior;
        self.detector.reset(prior);
        self.regime = self.detector.regime();
        self.aec.reset();
        self.far_silent_blocks = 0;
        self.silence_skipping = false;
        self.reference_gate = ReferenceGate::new();
    }

    pub fn regime(&self) -> EchoRegime {
        self.regime
    }

    pub fn inference_blocks(&self) -> u64 {
        self.inference_blocks
    }

    fn far_is_live(&self) -> bool {
        match (self.last_far_at, self.last_near_at) {
            (Some(far), Some(near)) => near.saturating_duration_since(far) < FAR_LIVENESS_TIMEOUT,
            _ => false,
        }
    }

    pub fn push_far_end(&mut self, chunk: &[i16], captured_at: Instant) {
        self.last_far_at = Some(captured_at);
        let epoch = *self.epoch.get_or_insert(captured_at);
        let start = self.far_clock.advance(epoch, captured_at, chunk.len());
        let samples: Vec<f32> = chunk.iter().map(|&s| s as f32 / 32768.0).collect();
        self.aligner.push_far(start, &samples);
    }

    pub fn push_near_end(&mut self, chunk: &[i16], captured_at: Instant) {
        self.last_near_at = Some(captured_at);
        let epoch = *self.epoch.get_or_insert(captured_at);
        let start = self.near_clock.advance(epoch, captured_at, chunk.len());
        let samples: Vec<f32> = chunk.iter().map(|&s| s as f32 / 32768.0).collect();
        self.aligner.push_near(start, &samples);
    }

    pub fn drain_cleaned(&mut self) -> Vec<i16> {
        self.run(false)
    }

    pub fn drain_remaining(&mut self) -> Vec<i16> {
        self.run(true)
    }

    fn run(&mut self, flush: bool) -> Vec<i16> {
        let wait_for_far = self.far_is_live();
        let mut out = Vec::new();
        while let Some((near, far)) = self.aligner.take(flush, wait_for_far) {
            let cleaned = self.process_aligned(&near, &far);
            out.extend(
                cleaned
                    .iter()
                    .map(|&x| (x.clamp(-1.0, 1.0) * 32767.0) as i16),
            );
        }
        self.report_bypassed();
        out
    }

    fn report_bypassed(&mut self) {
        let pending = self.aligner.bypassed - self.bypassed_reported;
        if pending >= BYPASS_LOG_INTERVAL_SAMPLES {
            log::warn!(
                "[aec] {} ms of microphone audio processed without aligned far-end reference; echo cancellation bypassed (system-output capture lagging > {} ms)",
                pending * 1000 / SAMPLE_RATE as u64,
                FAR_WAIT_LAG_SAMPLES as u64 * 1000 / SAMPLE_RATE as u64,
            );
            self.bypassed_reported = self.aligner.bypassed;
        }
    }

    fn observe_blocks(&mut self, near: &[f32], far: &[f32]) -> bool {
        let mut any_far_active = false;
        for (near_block, far_block) in near
            .chunks_exact(BLOCK_SHIFT)
            .zip(far.chunks_exact(BLOCK_SHIFT))
        {
            let far_rms = if far_block.iter().all(|v| v.is_finite()) {
                let r = rms(far_block);
                if r >= FAR_ACTIVE_RMS {
                    any_far_active = true;
                }
                Some(r)
            } else {
                None
            };
            self.detector.push_block(rms(near_block), far_rms);
        }
        any_far_active
    }

    fn sync_regime(&mut self) {
        let regime = self.detector.regime();
        if regime != self.regime {
            self.regime = regime;
            self.aec.reset();
            self.far_silent_blocks = 0;
            self.silence_skipping = false;
        }
    }

    fn far_silence_allows_inference(&mut self, far_active: bool, blocks: u32) -> bool {
        if far_active {
            if self.silence_skipping {
                self.aec.reset();
                self.silence_skipping = false;
            }
            self.far_silent_blocks = 0;
            return true;
        }
        self.far_silent_blocks = self.far_silent_blocks.saturating_add(blocks);
        if self.far_silent_blocks >= FAR_SILENCE_SKIP_BLOCKS {
            self.silence_skipping = true;
        }
        !self.silence_skipping
    }

    fn process_aligned(&mut self, near: &[f32], far: &[f32]) -> Vec<f32> {
        let mut out = sanitized(near);
        let full = near.len() - near.len() % BLOCK_SHIFT;
        if full > 0 {
            let far_active = self.observe_blocks(&out[..full], &far[..full]);
            self.sync_regime();
            let inference = self.regime == EchoRegime::Engaged
                && self.far_silence_allows_inference(far_active, (full / BLOCK_SHIFT) as u32)
                && !self.aec_failed;
            if inference {
                let near_clean = out[..full].to_vec();
                let far_clean = sanitized(&far[..full]);
                match self.aec.process_streaming(&near_clean, &far_clean) {
                    Ok(processed) => {
                        for i in 0..full {
                            if far[i].is_finite() {
                                out[i] = processed[i];
                            }
                        }
                        self.inference_blocks += (full / BLOCK_SHIFT) as u64;
                    }
                    Err(e) => {
                        log::error!(
                            "[aec] inference failed ({e}); disabling echo cancellation for this session and passing microphone through raw"
                        );
                        self.aec_failed = true;
                    }
                }
            }
        }
        self.reference_gate.process(&mut out, far);
        if let Some(dump) = self.dump.as_mut() {
            dump.write(near, far, &out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const CHUNK: usize = BLOCK_SHIFT;

    fn tone_chunk(index: usize, amp: f32, freq: f32) -> Vec<i16> {
        (0..CHUNK)
            .map(|j| {
                let t = (index * CHUNK + j) as f32 / SAMPLE_RATE as f32;
                (amp * (2.0 * std::f32::consts::PI * freq * t).sin() * 32767.0) as i16
            })
            .collect()
    }

    fn at(base: Instant, index: usize) -> Instant {
        base + Duration::from_secs_f64((index * CHUNK) as f64 / SAMPLE_RATE)
    }

    fn run_session(
        ec: &mut StreamingEchoCanceller,
        chunks: usize,
        far_amp: f32,
        near_amp: f32,
    ) -> (Vec<i16>, Vec<i16>) {
        let base = Instant::now();
        let mut input = Vec::new();
        let mut output = Vec::new();
        for i in 0..chunks {
            let burst_on = (i / 40) % 2 == 0;
            let far = tone_chunk(i, if burst_on { far_amp } else { 0.0 }, 440.0);
            let near = tone_chunk(i, near_amp, 220.0);
            ec.push_far_end(&far, at(base, i));
            ec.push_near_end(&near, at(base, i));
            input.extend(near);
            output.extend(ec.drain_cleaned());
        }
        output.extend(ec.drain_remaining());
        (input, output)
    }

    #[test]
    fn canceller_passes_mic_through_when_no_far_end() {
        let mut ec = StreamingEchoCanceller::new().expect("failed to build AEC");
        let base = Instant::now();
        let chunks = 8;

        let mut input: Vec<i16> = Vec::new();
        let mut output: Vec<i16> = Vec::new();
        for i in 0..chunks {
            let chunk: Vec<i16> = (0..CHUNK)
                .map(|j| (((i * CHUNK + j) % 200) as i16) - 100)
                .collect();
            ec.push_near_end(&chunk, at(base, i));
            output.extend(ec.drain_cleaned());
            input.extend(chunk);
        }
        output.extend(ec.drain_remaining());

        assert_eq!(output.len(), input.len());
        for (o, i) in output.iter().zip(input.iter()) {
            assert!(
                (*o as i32 - *i as i32).abs() <= 1,
                "passthrough altered sample: {o} vs {i}"
            );
        }
    }

    #[test]
    fn clear_prior_starts_bypassed_and_passes_mic_through() {
        let mut ec = StreamingEchoCanceller::with_prior(EchoPathPrior::Clear).unwrap();
        assert_eq!(ec.regime(), EchoRegime::Bypassed);

        let (input, output) = run_session(&mut ec, 60, 0.3, 0.2);

        assert_eq!(ec.inference_blocks(), 0);
        assert_eq!(output.len(), input.len());
        for (o, i) in output.iter().zip(input.iter()) {
            assert!((*o as i32 - *i as i32).abs() <= 1);
        }
    }

    #[test]
    fn echo_prone_prior_runs_inference_while_far_active() {
        let mut ec = StreamingEchoCanceller::with_prior(EchoPathPrior::EchoProne).unwrap();
        assert_eq!(ec.regime(), EchoRegime::Engaged);

        run_session(&mut ec, 60, 0.3, 0.2);

        assert!(ec.inference_blocks() > 0);
    }

    #[test]
    fn far_silence_gate_stops_inference_after_hangover() {
        let mut ec = StreamingEchoCanceller::with_prior(EchoPathPrior::EchoProne).unwrap();
        let base = Instant::now();
        let chunks = 500;

        for i in 0..chunks {
            let far = vec![0i16; CHUNK];
            let near = tone_chunk(i, 0.2, 220.0);
            ec.push_far_end(&far, at(base, i));
            ec.push_near_end(&near, at(base, i));
            ec.drain_cleaned();
        }
        ec.drain_remaining();

        assert_eq!(ec.regime(), EchoRegime::Engaged);
        let inferred = ec.inference_blocks();
        assert!(inferred > 0, "inference never ran before the gate closed");
        assert!(
            inferred <= 2 * FAR_SILENCE_SKIP_BLOCKS as u64,
            "silence gate failed to stop inference: {inferred} blocks"
        );
    }

    #[test]
    fn route_change_to_clear_prior_stops_inference() {
        let mut ec = StreamingEchoCanceller::with_prior(EchoPathPrior::EchoProne).unwrap();
        run_session(&mut ec, 60, 0.3, 0.2);
        let before = ec.inference_blocks();
        assert!(before > 0);

        ec.on_route_change(EchoPathPrior::Clear);
        assert_eq!(ec.regime(), EchoRegime::Bypassed);

        run_session(&mut ec, 60, 0.3, 0.2);
        assert_eq!(ec.inference_blocks(), before);
    }

    #[test]
    fn same_prior_route_change_resets_learned_regime() {
        let mut ec = StreamingEchoCanceller::with_prior(EchoPathPrior::Unknown).unwrap();
        let base = Instant::now();
        for i in 0..2500 {
            let burst_on = (i / 40) % 2 == 0;
            let far = tone_chunk(i, if burst_on { 0.3 } else { 0.0 }, 440.0);
            ec.push_far_end(&far, at(base, i));
            ec.push_near_end(&vec![0i16; CHUNK], at(base, i));
            ec.drain_cleaned();
        }
        ec.drain_remaining();
        assert_eq!(ec.regime(), EchoRegime::Bypassed);

        ec.on_route_change(EchoPathPrior::Unknown);
        assert_eq!(ec.regime(), EchoRegime::Engaged);
    }
}
