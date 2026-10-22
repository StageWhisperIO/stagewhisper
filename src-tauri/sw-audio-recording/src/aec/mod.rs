mod model;
mod onnx;

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub use model::BLOCK_SHIFT;
pub use onnx::Aec;

type AecResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const SAMPLE_RATE: f64 = 16000.0;
const MAX_FIFO_SAMPLES: usize = 16000 * 4;
const FAR_WAIT_LAG_SAMPLES: i64 = 8000;
const BYPASS_LOG_INTERVAL_SAMPLES: u64 = 16000;
const FAR_LIVENESS_TIMEOUT: Duration = Duration::from_millis(400);
const RESYNC_THRESHOLD_SAMPLES: i64 = 1600;

pub(crate) struct CircularBuffer {
    buffer: Vec<f32>,
    block_len: usize,
    block_shift: usize,
}

impl CircularBuffer {
    fn new(block_len: usize, block_shift: usize) -> Self {
        Self {
            buffer: vec![0.0f32; block_len],
            block_len,
            block_shift,
        }
    }

    fn push_chunk(&mut self, chunk: &[f32]) {
        let keep = self.block_len - self.block_shift;
        self.buffer.copy_within(self.block_shift.., 0);
        let copy_len = chunk.len().min(self.block_shift);
        self.buffer[keep..keep + copy_len].copy_from_slice(&chunk[..copy_len]);

        if copy_len < self.block_shift {
            self.buffer[keep + copy_len..].fill(0.0);
        }
    }

    fn shift_and_accumulate(&mut self, data: &[f32]) {
        let keep = self.block_len - self.block_shift;
        self.buffer.copy_within(self.block_shift.., 0);
        self.buffer[keep..].fill(0.0);

        for (d, &val) in self.buffer.iter_mut().zip(data.iter()) {
            *d += val;
        }
    }

    fn data(&self) -> &[f32] {
        &self.buffer
    }
}

struct TimelineStream {
    samples: VecDeque<f32>,
    base: i64,
}

impl TimelineStream {
    fn new() -> Self {
        Self {
            samples: VecDeque::new(),
            base: 0,
        }
    }

    fn end(&self) -> i64 {
        self.base + self.samples.len() as i64
    }

    fn place(&mut self, start: i64, samples: &[f32], cursor: i64) {
        let stop = start + samples.len() as i64;
        if stop <= cursor {
            return;
        }
        let (start, samples) = if start < cursor {
            (cursor, &samples[(cursor - start) as usize..])
        } else {
            (start, samples)
        };

        if self.samples.is_empty() {
            self.base = start;
            self.samples.extend(samples.iter().copied());
        } else if start >= self.end() {
            let gap = (start - self.end()) as usize;
            if gap > MAX_FIFO_SAMPLES {
                self.samples.clear();
                self.base = start;
            } else {
                self.samples.extend(std::iter::repeat_n(f32::NAN, gap));
            }
            self.samples.extend(samples.iter().copied());
        } else {
            let have = (self.end() - start) as usize;
            if have < samples.len() {
                self.samples.extend(samples[have..].iter().copied());
            }
        }

        if self.samples.len() > 2 * MAX_FIFO_SAMPLES {
            let overflow = self.samples.len() - MAX_FIFO_SAMPLES;
            self.samples.drain(..overflow);
            self.base += overflow as i64;
        }
    }

    fn read(&self, from: i64, to: i64) -> Vec<f32> {
        let mut out = vec![f32::NAN; (to - from) as usize];
        let lo = from.max(self.base);
        let hi = to.min(self.end());
        for pos in lo..hi {
            out[(pos - from) as usize] = self.samples[(pos - self.base) as usize];
        }
        out
    }

    fn trim_below(&mut self, cursor: i64) {
        if self.base < cursor {
            let drop = ((cursor - self.base) as usize).min(self.samples.len());
            self.samples.drain(..drop);
            self.base += drop as i64;
        }
    }
}

struct TimelineAligner {
    near: TimelineStream,
    far: TimelineStream,
    cursor: i64,
    started: bool,
    bypassed: u64,
}

impl TimelineAligner {
    fn new() -> Self {
        Self {
            near: TimelineStream::new(),
            far: TimelineStream::new(),
            cursor: 0,
            started: false,
            bypassed: 0,
        }
    }

    fn push_near(&mut self, start: i64, samples: &[f32]) {
        if !self.started {
            self.started = true;
            self.cursor = start;
        }
        self.near.place(start, samples, self.cursor);
    }

    fn push_far(&mut self, start: i64, samples: &[f32]) {
        self.far.place(start, samples, self.cursor);
    }

    fn take(&mut self, flush: bool, wait_for_far: bool) -> Option<(Vec<f32>, Vec<f32>)> {
        if !self.started {
            return None;
        }
        let near_end = self.near.end();
        let far_end = self.far.end();
        let mut to = near_end;
        if !flush && wait_for_far && far_end < near_end {
            to = far_end.max(near_end - FAR_WAIT_LAG_SAMPLES);
        }
        if to <= self.cursor {
            return None;
        }
        let mut span = (to - self.cursor) as usize;
        if !flush {
            span -= span % BLOCK_SHIFT;
        }
        if span == 0 {
            return None;
        }
        let from = self.cursor;
        let to = from + span as i64;
        let near = self.near.read(from, to);
        let far = self.far.read(from, to);
        let covered = far.iter().filter(|v| v.is_finite()).count();
        self.bypassed += (span - covered) as u64;
        self.cursor = to;
        self.near.trim_below(self.cursor);
        self.far.trim_below(self.cursor);
        Some((near, far))
    }
}

fn sanitized(samples: &[f32]) -> Vec<f32> {
    samples
        .iter()
        .map(|&v| if v.is_finite() { v } else { 0.0 })
        .collect()
}

fn offset_samples(epoch: Instant, captured_at: Instant) -> i64 {
    if captured_at >= epoch {
        (captured_at.duration_since(epoch).as_secs_f64() * SAMPLE_RATE).round() as i64
    } else {
        -((epoch.duration_since(captured_at).as_secs_f64() * SAMPLE_RATE).round() as i64)
    }
}

struct StreamClock {
    started: bool,
    next_pos: i64,
}

impl StreamClock {
    fn new() -> Self {
        Self {
            started: false,
            next_pos: 0,
        }
    }

    fn advance(&mut self, epoch: Instant, captured_at: Instant, len: usize) -> i64 {
        let expected = offset_samples(epoch, captured_at) - len as i64;
        let start = if !self.started || (expected - self.next_pos).abs() > RESYNC_THRESHOLD_SAMPLES
        {
            self.started = true;
            expected
        } else {
            self.next_pos
        };
        self.next_pos = start + len as i64;
        start
    }
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
    #[cfg(feature = "reference-gate")]
    reference_gate: crate::reference_gate::ReferenceGate,
}

impl StreamingEchoCanceller {
    pub fn new() -> AecResult<Self> {
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
            #[cfg(feature = "reference-gate")]
            reference_gate: crate::reference_gate::ReferenceGate::new(),
        })
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

    fn process_aligned(&mut self, near: &[f32], far: &[f32]) -> Vec<f32> {
        let mut out = sanitized(near);
        let full = near.len() - near.len() % BLOCK_SHIFT;
        if full > 0 && !self.aec_failed {
            let near_clean = sanitized(&near[..full]);
            let far_clean = sanitized(&far[..full]);
            match self.aec.process_streaming(&near_clean, &far_clean) {
                Ok(processed) => {
                    for i in 0..full {
                        if far[i].is_finite() {
                            out[i] = processed[i];
                        }
                    }
                }
                Err(e) => {
                    log::error!(
                        "[aec] inference failed ({e}); disabling echo cancellation for this session and passing microphone through raw"
                    );
                    self.aec_failed = true;
                }
            }
        }
        #[cfg(feature = "reference-gate")]
        self.reference_gate.process(&mut out, far);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ramp(start: i64, len: usize) -> Vec<f32> {
        (0..len).map(|i| (start + i as i64) as f32).collect()
    }

    fn drain_all(aligner: &mut TimelineAligner) -> usize {
        let mut emitted = 0;
        while let Some((near, _)) = aligner.take(false, true) {
            emitted += near.len();
        }
        if let Some((near, _)) = aligner.take(true, true) {
            emitted += near.len();
        }
        emitted
    }

    #[test]
    fn mic_is_never_dropped_without_far_end() {
        let mut aligner = TimelineAligner::new();
        let total = BLOCK_SHIFT * 20;
        aligner.push_near(0, &ramp(0, total));

        let mut emitted = 0usize;
        while let Some((near, _far)) = aligner.take(false, true) {
            emitted += near.len();
        }
        if let Some((near, _far)) = aligner.take(true, true) {
            emitted += near.len();
        }

        assert_eq!(emitted, total);
    }

    #[test]
    fn future_far_end_is_kept_not_discarded() {
        let mut aligner = TimelineAligner::new();
        let lead = FAR_WAIT_LAG_SAMPLES as usize + BLOCK_SHIFT * 40;
        let m = BLOCK_SHIFT * 4;

        aligner.push_near(0, &ramp(0, lead));
        while aligner.take(false, true).is_some() {}
        let cursor = aligner.cursor;
        assert!(cursor > 0);

        let far = ramp(7, m);
        aligner.push_far(cursor, &far);
        aligner.push_near(lead as i64, &ramp(0, m));

        let mut aligned_far = Vec::new();
        while let Some((_near, f)) = aligner.take(false, true) {
            aligned_far.extend(f);
        }

        assert!(aligned_far.len() >= m);
        assert_eq!(&aligned_far[..m], &far[..]);
    }

    #[test]
    fn stale_far_end_is_discarded() {
        let mut aligner = TimelineAligner::new();
        let span = FAR_WAIT_LAG_SAMPLES as usize + BLOCK_SHIFT * 40;

        aligner.push_near(0, &ramp(0, span));
        while aligner.take(false, true).is_some() {}
        let cursor = aligner.cursor;
        assert!(cursor > 0);

        aligner.push_far(0, &ramp(0, cursor as usize));
        aligner.push_near(span as i64, &ramp(0, span));

        let mut aligned_far = Vec::new();
        while let Some((_near, f)) = aligner.take(false, true) {
            aligned_far.extend(f);
        }

        assert!(aligned_far.iter().all(|&v| !v.is_finite()));
    }

    #[test]
    fn stream_clock_advances_by_sample_count_ignoring_jitter() {
        let epoch = Instant::now();
        let mut clock = StreamClock::new();
        let len = 100;

        let p0 = clock.advance(epoch, epoch + Duration::from_millis(10), len);
        let p1 = clock.advance(epoch, epoch + Duration::from_millis(35), len);

        assert_eq!(p1, p0 + len as i64);
    }

    #[test]
    fn stream_clock_resyncs_on_large_gap() {
        let epoch = Instant::now();
        let mut clock = StreamClock::new();
        let len = 100;

        let p0 = clock.advance(epoch, epoch + Duration::from_millis(10), len);
        let p1 = clock.advance(epoch, epoch + Duration::from_millis(510), len);

        assert!(p1 > p0 + len as i64 * 2);
    }

    #[test]
    fn offset_is_signed_for_out_of_order_frames() {
        let base = Instant::now();
        let earlier = base + Duration::from_millis(10);
        let later = base + Duration::from_millis(20);

        assert_eq!(offset_samples(later, later), 0);
        assert_eq!(offset_samples(later, earlier), -160);
        assert_eq!(offset_samples(earlier, later), 160);
    }

    #[test]
    fn near_captured_before_epoch_is_preserved() {
        let mut aligner = TimelineAligner::new();
        let len = BLOCK_SHIFT * 4;

        aligner.push_near(-(len as i64), &ramp(0, len));
        aligner.push_near(0, &ramp(0, len));

        assert_eq!(drain_all(&mut aligner), 2 * len);
    }

    #[test]
    fn far_end_within_grace_is_waited_for_and_used() {
        let mut aligner = TimelineAligner::new();
        let n = BLOCK_SHIFT * 4;
        assert!((n as i64) < FAR_WAIT_LAG_SAMPLES);

        aligner.push_near(0, &ramp(0, n));
        assert!(aligner.take(false, true).is_none());
        assert_eq!(aligner.bypassed, 0);

        let far = ramp(1, n);
        aligner.push_far(0, &far);
        let mut aligned_far = Vec::new();
        while let Some((_near, f)) = aligner.take(false, true) {
            aligned_far.extend(f);
        }

        assert_eq!(aligner.bypassed, 0);
        assert_eq!(aligned_far, far);
    }

    #[test]
    fn missing_far_end_beyond_grace_is_counted_as_bypass() {
        let mut aligner = TimelineAligner::new();
        let n = FAR_WAIT_LAG_SAMPLES as usize + BLOCK_SHIFT * 40;

        aligner.push_near(0, &ramp(0, n));
        while aligner.take(false, true).is_some() {}

        assert!(aligner.bypassed > 0);
    }

    #[test]
    fn dropped_far_end_gap_is_counted_as_bypass() {
        let mut aligner = TimelineAligner::new();
        let block = BLOCK_SHIFT;

        aligner.push_near(0, &ramp(0, block * 30));
        aligner.push_far(0, &ramp(1, block * 5));
        aligner.push_far((block * 10) as i64, &ramp(1, block * 5));

        let mut aligned_far = Vec::new();
        while let Some((_near, f)) = aligner.take(false, true) {
            aligned_far.extend(f);
        }

        assert_eq!(aligner.bypassed, (block * 5) as u64);
        assert!(aligned_far[..block * 5].iter().all(|&v| v.is_finite()));
        assert!(aligned_far[block * 5..block * 10]
            .iter()
            .all(|&v| !v.is_finite()));
    }

    #[test]
    fn canceller_passes_mic_through_when_no_far_end() {
        let mut ec = StreamingEchoCanceller::new().expect("failed to build AEC");
        let base = Instant::now();
        let block = BLOCK_SHIFT;
        let chunks = 8;

        let mut input: Vec<i16> = Vec::new();
        let mut output: Vec<i16> = Vec::new();
        for i in 0..chunks {
            let chunk: Vec<i16> = (0..block)
                .map(|j| (((i * block + j) % 200) as i16) - 100)
                .collect();
            let captured_at = base + Duration::from_secs_f64((i * block) as f64 / SAMPLE_RATE);
            ec.push_near_end(&chunk, captured_at);
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
    fn dead_far_end_passes_mic_through_without_waiting() {
        let mut aligner = TimelineAligner::new();
        let n = BLOCK_SHIFT * 4;
        assert!((n as i64) < FAR_WAIT_LAG_SAMPLES);

        aligner.push_near(0, &ramp(0, n));

        let mut emitted = 0;
        while let Some((near, _)) = aligner.take(false, false) {
            emitted += near.len();
        }

        assert_eq!(emitted, n);
    }
}
