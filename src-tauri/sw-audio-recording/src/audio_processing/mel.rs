use ndarray::Array2;
use realfft::RealFftPlanner;

const SLANEY_LINEAR_HZ_PER_MEL: f64 = 200.0 / 3.0;
const SLANEY_LOG_START_HZ: f64 = 1000.0;
const SLANEY_LOG_START_MEL: f64 = SLANEY_LOG_START_HZ / SLANEY_LINEAR_HZ_PER_MEL;
const SLANEY_LOG_STEP: f64 = 0.068_751_777_420_949_12;
const NORMALIZE_EPSILON: f32 = 1e-5;

pub struct MelConfig {
    pub sample_rate: usize,
    pub n_fft: usize,
    pub win_length: usize,
    pub hop_length: usize,
    pub n_mels: usize,
    pub fmin: f64,
    pub fmax: f64,
    pub preemphasis: f32,
    pub log_guard: f32,
}

impl Default for MelConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            n_fft: 512,
            win_length: 400,
            hop_length: 160,
            n_mels: 128,
            fmin: 0.0,
            fmax: 8000.0,
            preemphasis: 0.97,
            log_guard: 2f32.powi(-24),
        }
    }
}

pub fn compute_mel_spectrogram(samples: &[f32], config: &MelConfig) -> Array2<f32> {
    let mut features = log_mel_energies(samples, config);
    normalize_features(&mut features);
    features
}

fn log_mel_energies(samples: &[f32], config: &MelConfig) -> Array2<f32> {
    let n_frames = samples.len() / config.hop_length;
    let mut result = Array2::zeros((n_frames, config.n_mels));
    if n_frames == 0 {
        return result;
    }

    let audio = apply_preemphasis(samples, config.preemphasis);
    let window = hann_window(config.win_length);
    let filterbank = mel_filterbank(
        config.n_fft,
        config.n_mels,
        config.sample_rate as f64,
        config.fmin,
        config.fmax,
    );
    let centre_padding = config.n_fft / 2;
    let window_offset = (config.n_fft - config.win_length) / 2;

    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(config.n_fft);
    let mut fft_input = vec![0.0f32; config.n_fft];
    let mut spectrum = fft.make_output_vec();
    let mut power = vec![0.0f32; config.n_fft / 2 + 1];

    for frame_idx in 0..n_frames {
        fft_input.iter_mut().for_each(|x| *x = 0.0);
        let frame_start = frame_idx * config.hop_length + window_offset;
        for (i, weight) in window.iter().enumerate() {
            let padded_idx = frame_start + i;
            if padded_idx >= centre_padding {
                if let Some(sample) = audio.get(padded_idx - centre_padding) {
                    fft_input[window_offset + i] = sample * weight;
                }
            }
        }

        fft.process(&mut fft_input, &mut spectrum).unwrap();
        for (bin, c) in power.iter_mut().zip(spectrum.iter()) {
            *bin = c.re * c.re + c.im * c.im;
        }

        for (mel_idx, weights) in filterbank.iter().enumerate() {
            let mel_energy: f32 = weights.iter().zip(power.iter()).map(|(w, p)| w * p).sum();
            result[[frame_idx, mel_idx]] = (mel_energy + config.log_guard).ln();
        }
    }

    result
}

fn apply_preemphasis(samples: &[f32], alpha: f32) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0.0f32; samples.len()];
    out[0] = samples[0];
    for i in 1..samples.len() {
        out[i] = samples[i] - alpha * samples[i - 1];
    }
    out
}

fn hann_window(length: usize) -> Vec<f32> {
    if length < 2 {
        return vec![1.0; length];
    }
    (0..length)
        .map(|i| {
            let phase = 2.0 * std::f64::consts::PI * i as f64 / (length - 1) as f64;
            (0.5 * (1.0 - phase.cos())) as f32
        })
        .collect()
}

fn hz_to_mel(freq: f64) -> f64 {
    if freq >= SLANEY_LOG_START_HZ {
        SLANEY_LOG_START_MEL + (freq / SLANEY_LOG_START_HZ).ln() / SLANEY_LOG_STEP
    } else {
        freq / SLANEY_LINEAR_HZ_PER_MEL
    }
}

fn mel_to_hz(mel: f64) -> f64 {
    if mel >= SLANEY_LOG_START_MEL {
        SLANEY_LOG_START_HZ * (SLANEY_LOG_STEP * (mel - SLANEY_LOG_START_MEL)).exp()
    } else {
        mel * SLANEY_LINEAR_HZ_PER_MEL
    }
}

fn mel_filterbank(
    n_fft: usize,
    n_mels: usize,
    sample_rate: f64,
    fmin: f64,
    fmax: f64,
) -> Vec<Vec<f32>> {
    let n_fft_bins = n_fft / 2 + 1;
    let fft_freqs: Vec<f64> = (0..n_fft_bins)
        .map(|k| k as f64 * sample_rate / n_fft as f64)
        .collect();

    let mel_min = hz_to_mel(fmin);
    let mel_max = hz_to_mel(fmax);
    let edges: Vec<f64> = (0..n_mels + 2)
        .map(|i| mel_to_hz(mel_min + (mel_max - mel_min) * i as f64 / (n_mels + 1) as f64))
        .collect();

    edges
        .windows(3)
        .map(|edge| {
            let (left, centre, right) = (edge[0], edge[1], edge[2]);
            let area_norm = 2.0 / (right - left);
            fft_freqs
                .iter()
                .map(|&freq| {
                    let rising = (freq - left) / (centre - left);
                    let falling = (right - freq) / (right - centre);
                    (rising.min(falling).max(0.0) * area_norm) as f32
                })
                .collect()
        })
        .collect()
}

fn normalize_features(features: &mut Array2<f32>) {
    let n_frames = features.shape()[0];
    if n_frames <= 1 {
        return;
    }

    for mut column in features.columns_mut() {
        let mean = column.iter().map(|&v| v as f64).sum::<f64>() / n_frames as f64;
        let variance = column
            .iter()
            .map(|&v| (v as f64 - mean).powi(2))
            .sum::<f64>()
            / (n_frames - 1) as f64;
        let scale = variance.sqrt() + NORMALIZE_EPSILON as f64;
        column
            .iter_mut()
            .for_each(|v| *v = ((*v as f64 - mean) / scale) as f32);
    }
}

#[cfg(test)]
#[path = "mel_tests.rs"]
mod tests;
