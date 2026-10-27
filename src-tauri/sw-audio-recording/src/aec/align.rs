use std::collections::VecDeque;
use std::time::Instant;

use super::BLOCK_SHIFT;

pub(crate) const SAMPLE_RATE: f64 = 16000.0;
pub(crate) const FAR_WAIT_LAG_SAMPLES: i64 = 8000;
const MAX_FIFO_SAMPLES: usize = 16000 * 4;
const RESYNC_THRESHOLD_SAMPLES: i64 = 1600;

pub(crate) struct CircularBuffer {
    buffer: Vec<f32>,
    block_len: usize,
    block_shift: usize,
}

impl CircularBuffer {
    pub(crate) fn new(block_len: usize, block_shift: usize) -> Self {
        Self {
            buffer: vec![0.0f32; block_len],
            block_len,
            block_shift,
        }
    }

    pub(crate) fn push_chunk(&mut self, chunk: &[f32]) {
        let keep = self.block_len - self.block_shift;
        self.buffer.copy_within(self.block_shift.., 0);
        let copy_len = chunk.len().min(self.block_shift);
        self.buffer[keep..keep + copy_len].copy_from_slice(&chunk[..copy_len]);

        if copy_len < self.block_shift {
            self.buffer[keep + copy_len..].fill(0.0);
        }
    }

    pub(crate) fn shift_and_accumulate(&mut self, data: &[f32]) {
        let keep = self.block_len - self.block_shift;
        self.buffer.copy_within(self.block_shift.., 0);
        self.buffer[keep..].fill(0.0);

        for (d, &val) in self.buffer.iter_mut().zip(data.iter()) {
            *d += val;
        }
    }

    pub(crate) fn data(&self) -> &[f32] {
        &self.buffer
    }

    pub(crate) fn clear(&mut self) {
        self.buffer.fill(0.0);
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

pub(crate) struct TimelineAligner {
    near: TimelineStream,
    far: TimelineStream,
    pub(crate) cursor: i64,
    started: bool,
    pub(crate) bypassed: u64,
}

impl TimelineAligner {
    pub(crate) fn new() -> Self {
        Self {
            near: TimelineStream::new(),
            far: TimelineStream::new(),
            cursor: 0,
            started: false,
            bypassed: 0,
        }
    }

    pub(crate) fn push_near(&mut self, start: i64, samples: &[f32]) {
        if !self.started {
            self.started = true;
            self.cursor = start;
        }
        self.near.place(start, samples, self.cursor);
    }

    pub(crate) fn push_far(&mut self, start: i64, samples: &[f32]) {
        self.far.place(start, samples, self.cursor);
    }

    pub(crate) fn take(&mut self, flush: bool, wait_for_far: bool) -> Option<(Vec<f32>, Vec<f32>)> {
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

pub(crate) fn offset_samples(epoch: Instant, captured_at: Instant) -> i64 {
    if captured_at >= epoch {
        (captured_at.duration_since(epoch).as_secs_f64() * SAMPLE_RATE).round() as i64
    } else {
        -((epoch.duration_since(captured_at).as_secs_f64() * SAMPLE_RATE).round() as i64)
    }
}

pub(crate) struct StreamClock {
    started: bool,
    next_pos: i64,
}

impl StreamClock {
    pub(crate) fn new() -> Self {
        Self {
            started: false,
            next_pos: 0,
        }
    }

    pub(crate) fn advance(&mut self, epoch: Instant, captured_at: Instant, len: usize) -> i64 {
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
