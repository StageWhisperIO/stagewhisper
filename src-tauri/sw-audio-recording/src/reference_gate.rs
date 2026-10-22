use crate::aec::BLOCK_SHIFT;
use realfft::{num_complex::Complex32, RealFftPlanner, RealToComplex};
use std::sync::Arc;

const WINDOW: usize = 4 * BLOCK_SHIFT;
const SEGMENT: usize = 2 * BLOCK_SHIFT;
const WELCH_SEGMENTS: usize = 3;
const SAMPLE_RATE: f32 = 16000.0;
const BAND_LOW_HZ: f32 = 300.0;
const BAND_HIGH_HZ: f32 = 3400.0;
const FAR_FLOOR: f32 = 1e-4;
const NEAR_ABS_FLOOR: f32 = 3e-3;
const MAX_ATTEN_DB: f32 = 18.0;
const ATTACK_COEF: f32 = 0.2;
const RELEASE_COEF: f32 = 0.5;
const LAG_SEARCH: usize = BLOCK_SHIFT * 8;
const EPS: f32 = 1e-9;

const COHERENCE_THRESHOLD: f32 = 0.65;
const NEAR_HOLD_THRESHOLD: f32 = 2.0;

struct WelchCoherence {
    fft: Arc<dyn RealToComplex<f32>>,
    hann: Vec<f32>,
    band_lo: usize,
    band_hi: usize,
}

impl WelchCoherence {
    fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(SEGMENT);
        let hann: Vec<f32> = (0..SEGMENT)
            .map(|n| {
                let x = std::f32::consts::PI * 2.0 * n as f32 / SEGMENT as f32;
                0.5 - 0.5 * x.cos()
            })
            .collect();
        let bins = SEGMENT / 2 + 1;
        let hz_per_bin = SAMPLE_RATE / SEGMENT as f32;
        let band_lo = ((BAND_LOW_HZ / hz_per_bin).floor() as usize).clamp(1, bins - 1);
        let band_hi = ((BAND_HIGH_HZ / hz_per_bin).ceil() as usize).clamp(band_lo + 1, bins);
        Self {
            fft,
            hann,
            band_lo,
            band_hi,
        }
    }

    fn spectrum(&self, segment: &[f32]) -> Vec<Complex32> {
        let mut input: Vec<f32> = segment
            .iter()
            .zip(self.hann.iter())
            .map(|(&s, &w)| s * w)
            .collect();
        let mut output = self.fft.make_output_vec();
        let _ = self.fft.process(&mut input, &mut output);
        output
    }

    fn banded_msc(&self, near: &[f32], far: &[f32]) -> f32 {
        let bins = SEGMENT / 2 + 1;
        let mut sxx = vec![0.0f32; bins];
        let mut syy = vec![0.0f32; bins];
        let mut sxy = vec![Complex32::new(0.0, 0.0); bins];

        let hop = SEGMENT / 2;
        for k in 0..WELCH_SEGMENTS {
            let start = k * hop;
            if start + SEGMENT > near.len() || start + SEGMENT > far.len() {
                break;
            }
            let x = self.spectrum(&near[start..start + SEGMENT]);
            let y = self.spectrum(&far[start..start + SEGMENT]);
            for b in 0..bins {
                sxx[b] += x[b].norm_sqr();
                syy[b] += y[b].norm_sqr();
                sxy[b] += x[b] * y[b].conj();
            }
        }

        let mut acc = 0.0f32;
        let mut count = 0usize;
        for b in self.band_lo..self.band_hi {
            let denom = sxx[b] * syy[b];
            if denom > EPS {
                acc += sxy[b].norm_sqr() / denom;
                count += 1;
            }
        }
        if count == 0 {
            0.0
        } else {
            acc / count as f32
        }
    }
}

pub struct ReferenceGate {
    gain_state: f32,
    welch: WelchCoherence,
    near_buf: Vec<f32>,
    far_buf: Vec<f32>,
    windows: u64,
    gated: u64,
    held: u64,
    atten_db_sum: f32,
}

impl ReferenceGate {
    pub fn new() -> Self {
        Self {
            gain_state: 1.0,
            welch: WelchCoherence::new(),
            near_buf: Vec::new(),
            far_buf: Vec::new(),
            windows: 0,
            gated: 0,
            held: 0,
            atten_db_sum: 0.0,
        }
    }

    pub fn summary(&self) -> (u64, u64, f32) {
        let mean = if self.gated > 0 {
            self.atten_db_sum / self.gated as f32
        } else {
            0.0
        };
        (self.windows, self.gated, mean)
    }

    pub fn process(&mut self, near_post: &mut [f32], far: &[f32]) {
        self.near_buf.extend_from_slice(near_post);
        self.far_buf.extend_from_slice(far);
        while self.near_buf.len() >= WINDOW && self.far_buf.len() >= WINDOW {
            self.analyze_window();
            self.near_buf.drain(..WINDOW);
            self.far_buf.drain(..WINDOW);
        }
        for sample in near_post.iter_mut() {
            *sample *= self.gain_state;
        }
    }

    fn analyze_window(&mut self) {
        self.windows += 1;

        let near: Vec<f32> = self.near_buf[..WINDOW].to_vec();
        let far: Vec<f32> = self.far_buf[..WINDOW].to_vec();

        if far.iter().any(|v| !v.is_finite()) {
            self.release();
            return;
        }

        let far_rms = rms(&far);
        if far_rms < FAR_FLOOR {
            self.release();
            return;
        }

        let near_indep_rms = rms(&near);
        let hold =
            near_indep_rms >= NEAR_HOLD_THRESHOLD * far_rms || near_indep_rms >= NEAR_ABS_FLOOR;
        if hold {
            self.held += 1;
            self.release();
            return;
        }

        let lag = best_lag(&near, &far);
        let aligned_far = shift(&far, lag);
        let coh = self.welch.banded_msc(&near, &aligned_far);

        let fire = coh >= COHERENCE_THRESHOLD;
        if !fire {
            self.release();
            return;
        }

        let t = ((coh - COHERENCE_THRESHOLD) / (1.0 - COHERENCE_THRESHOLD)).clamp(0.0, 1.0);
        let atten_db = -MAX_ATTEN_DB * smoothstep(t);
        let target_gain = 10.0f32.powf(atten_db / 20.0);
        let coef = if target_gain < self.gain_state {
            ATTACK_COEF
        } else {
            RELEASE_COEF
        };
        self.gain_state += coef * (target_gain - self.gain_state);

        self.gated += 1;
        self.atten_db_sum += atten_db;
    }

    fn release(&mut self) {
        self.gain_state += RELEASE_COEF * (1.0 - self.gain_state);
    }
}

impl Default for ReferenceGate {
    fn default() -> Self {
        Self::new()
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|&v| v * v).sum();
    (sum / samples.len() as f32).sqrt()
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn best_lag(near: &[f32], far: &[f32]) -> i32 {
    let max_lag = LAG_SEARCH.min(near.len().saturating_sub(1)) as i32;
    let mut best = 0i32;
    let mut best_score = f32::MIN;
    for lag in -max_lag..=max_lag {
        let mut dot = 0.0f32;
        let mut energy = 0.0f32;
        for i in 0..near.len() {
            let j = i as i32 + lag;
            if j < 0 || j as usize >= far.len() {
                continue;
            }
            let f = far[j as usize];
            dot += near[i] * f;
            energy += f * f;
        }
        let score = if energy > EPS {
            dot.abs() / energy.sqrt()
        } else {
            0.0
        };
        if score > best_score {
            best_score = score;
            best = lag;
        }
    }
    best
}

fn shift(samples: &[f32], lag: i32) -> Vec<f32> {
    let n = samples.len();
    let mut out = vec![0.0f32; n];
    for i in 0..n {
        let j = i as i32 + lag;
        if j >= 0 && (j as usize) < n {
            out[i] = samples[j as usize];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, len: usize, amp: f32) -> Vec<f32> {
        (0..len)
            .map(|n| amp * (2.0 * std::f32::consts::PI * freq * n as f32 / SAMPLE_RATE).sin())
            .collect()
    }

    fn rir() -> Vec<f32> {
        let mut taps = vec![0.0f32; 640];
        taps[0] = 0.8;
        taps[80] = 0.4;
        taps[200] = 0.25;
        taps[400] = 0.15;
        taps[639] = 0.08;
        taps
    }

    fn convolve(signal: &[f32], filter: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0f32; signal.len()];
        for i in 0..signal.len() {
            let mut acc = 0.0f32;
            for (k, &h) in filter.iter().enumerate() {
                if i >= k {
                    acc += h * signal[i - k];
                }
            }
            out[i] = acc;
        }
        out
    }

    #[test]
    fn pure_reverberant_echo_is_attenuated() {
        let len = WINDOW * 6;
        let far = tone(800.0, len, 0.3);
        let near_pre = convolve(&far, &rir());
        let aec_estimate: Vec<f32> = near_pre.iter().map(|&v| v * 0.995).collect();
        let mut near_post: Vec<f32> = near_pre
            .iter()
            .zip(aec_estimate.iter())
            .map(|(&n, &e)| n - e)
            .collect();
        let pre_rms = rms(&near_post);
        assert!(pre_rms < NEAR_ABS_FLOOR, "residual echo must sit below noise floor");

        let mut gate = ReferenceGate::new();
        gate.process(&mut near_post, &far);

        let post_rms = rms(&near_post);
        assert!(post_rms < pre_rms, "echo not reduced: {pre_rms} -> {post_rms}");
        let (_, gated, _) = gate.summary();
        assert!(gated > 0, "expected gating on pure echo");
    }

    #[test]
    fn independent_near_speech_triggers_hold() {
        let len = WINDOW * 6;
        let far = tone(800.0, len, 0.3);
        let near_pre = tone(220.0, len, 0.3);
        let mut near_post = near_pre.clone();
        let pre = near_post.clone();

        let mut gate = ReferenceGate::new();
        gate.process(&mut near_post, &far);

        let (_, gated, _) = gate.summary();
        assert_eq!(gated, 0, "genuine near speech must not be attenuated");
        for (a, b) in near_post.iter().zip(pre.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn reverberant_double_talk_keeps_near() {
        let len = WINDOW * 6;
        let far = tone(800.0, len, 0.3);
        let echo = convolve(&far, &rir());
        let independent = tone(220.0, len, 0.3);
        let near_pre: Vec<f32> = independent
            .iter()
            .zip(echo.iter())
            .map(|(&a, &b)| a + b)
            .collect();
        let aec_estimate: Vec<f32> = echo.iter().map(|&v| v * 0.9).collect();
        let mut near_post: Vec<f32> = near_pre
            .iter()
            .zip(aec_estimate.iter())
            .map(|(&n, &e)| n - e)
            .collect();
        let pre_rms = rms(&near_post);

        let mut gate = ReferenceGate::new();
        gate.process(&mut near_post, &far);

        let post_rms = rms(&near_post);
        let (_, gated, _) = gate.summary();
        assert_eq!(gated, 0, "double-talk must hold and not attenuate");
        assert!((post_rms - pre_rms).abs() / pre_rms < 0.01);
    }

    #[test]
    fn two_correlated_parties_are_not_gated() {
        let len = WINDOW * 6;
        let far = tone(700.0, len, 0.0015);
        let near_pre = tone(950.0, len, 0.0015);
        let mut near_post = near_pre.clone();

        let mut gate = ReferenceGate::new();
        gate.process(&mut near_post, &far);

        let (_, gated, _) = gate.summary();
        assert_eq!(gated, 0, "independent speech must not look like echo");
    }

    #[test]
    fn far_silent_passes_through() {
        let len = WINDOW * 4;
        let far = vec![0.0f32; len];
        let near_pre = tone(300.0, len, 0.2);
        let mut near_post = near_pre.clone();
        let pre = near_post.clone();

        let mut gate = ReferenceGate::new();
        gate.process(&mut near_post, &far);

        assert_eq!(near_post, pre);
    }

    #[test]
    fn nan_far_passes_through() {
        let len = WINDOW * 4;
        let reference = tone(800.0, len, 0.3);
        let far = vec![f32::NAN; len];
        let near_pre = convolve(&reference, &rir());
        let mut near_post = near_pre.clone();
        let pre = near_post.clone();

        let mut gate = ReferenceGate::new();
        gate.process(&mut near_post, &far);

        assert_eq!(near_post, pre);
    }

    #[test]
    fn sub_window_chunks_still_gate() {
        let len = WINDOW * 6;
        let far = tone(800.0, len, 0.3);
        let near_pre = convolve(&far, &rir());
        let mut near_post: Vec<f32> = near_pre.iter().map(|&v| v * 0.004).collect();
        let pre_rms = rms(&near_post);
        assert!(pre_rms < NEAR_ABS_FLOOR);

        let mut gate = ReferenceGate::new();
        let chunk = 200;
        let mut i = 0;
        while i < len {
            let end = (i + chunk).min(len);
            gate.process(&mut near_post[i..end], &far[i..end]);
            i = end;
        }

        let post_rms = rms(&near_post);
        assert!(post_rms < pre_rms, "sub-window chunks not gated: {pre_rms} -> {post_rms}");
        let (_, gated, _) = gate.summary();
        assert!(gated > 0, "expected gating across accumulated sub-window chunks");
    }

    #[test]
    fn attenuation_not_limited_by_prior_aec_removal() {
        let len = WINDOW * 8;
        let far = tone(800.0, len, 0.3);
        let near_pre = convolve(&far, &rir());
        let mut near_post: Vec<f32> = near_pre.iter().map(|&v| v * 0.004).collect();
        let pre_rms = rms(&near_post);
        assert!(pre_rms < NEAR_ABS_FLOOR);

        let mut gate = ReferenceGate::new();
        gate.process(&mut near_post, &far);

        let post_rms = rms(&near_post);
        assert!(post_rms < 0.5 * pre_rms, "gate under-attenuated: {pre_rms} -> {post_rms}");
    }
}
