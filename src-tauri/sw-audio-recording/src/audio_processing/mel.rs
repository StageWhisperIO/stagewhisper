use ndarray::Array2;
use realfft::RealFftPlanner;

/// NeMo-compatible mel spectrogram parameters.
pub struct MelConfig {
    pub sample_rate: usize,
    pub n_fft: usize,
    pub win_length: usize,
    pub hop_length: usize,
    pub n_mels: usize,
    pub fmin: f64,
    pub fmax: f64,
    pub preemphasis: f32,
    pub log_floor: f32,
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
            log_floor: 1e-10,
        }
    }
}

/// Compute 128-bin log-mel spectrogram from 16kHz mono audio.
///
/// Returns `Array2<f32>` of shape `[time_steps, n_mels]`.
pub fn compute_mel_spectrogram(samples: &[f32], config: &MelConfig) -> Array2<f32> {
    let audio = apply_preemphasis(samples, config.preemphasis);
    let window = hann_window(config.win_length);
    let filterbank = mel_filterbank(
        config.n_fft,
        config.n_mels,
        config.sample_rate as f64,
        config.fmin,
        config.fmax,
    );

    let n_frames = if audio.len() >= config.win_length {
        (audio.len() - config.win_length) / config.hop_length + 1
    } else {
        0
    };

    if n_frames == 0 {
        return Array2::zeros((0, config.n_mels));
    }

    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(config.n_fft);

    let mut result = Array2::zeros((n_frames, config.n_mels));
    let n_fft_bins = config.n_fft / 2 + 1;

    let mut fft_input = vec![0.0f32; config.n_fft];
    let mut spectrum = fft.make_output_vec();

    for frame_idx in 0..n_frames {
        let start = frame_idx * config.hop_length;

        fft_input.iter_mut().for_each(|x| *x = 0.0);

        for i in 0..config.win_length {
            let sample_idx = start + i;
            if sample_idx < audio.len() {
                fft_input[i] = audio[sample_idx] * window[i];
            }
        }

        fft.process(&mut fft_input, &mut spectrum).unwrap();

        let mut power = vec![0.0f32; n_fft_bins];
        for (i, c) in spectrum.iter().enumerate() {
            power[i] = c.re * c.re + c.im * c.im;
        }

        for mel_idx in 0..config.n_mels {
            let mut mel_energy: f32 = 0.0;
            for (k, item) in power.iter().enumerate().take(n_fft_bins) {
                mel_energy += filterbank[mel_idx][k] * item;
            }
            result[[frame_idx, mel_idx]] = (mel_energy.max(config.log_floor)).ln();
        }
    }

    normalize_features(&mut result);

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
    (0..length)
        .map(|i| {
            let phase = 2.0 * std::f32::consts::PI * i as f32 / length as f32;
            0.5 * (1.0 - phase.cos())
        })
        .collect()
}

fn hz_to_mel(freq: f64) -> f64 {
    2595.0 * (1.0 + freq / 700.0).log10()
}

fn mel_to_hz(mel: f64) -> f64 {
    700.0 * (10.0_f64.powf(mel / 2595.0) - 1.0)
}

fn mel_filterbank(
    n_fft: usize,
    n_mels: usize,
    sample_rate: f64,
    fmin: f64,
    fmax: f64,
) -> Vec<Vec<f32>> {
    let n_fft_bins = n_fft / 2 + 1;

    let mel_min = hz_to_mel(fmin);
    let mel_max = hz_to_mel(fmax);

    let n_points = n_mels + 2;
    let mel_points: Vec<f64> = (0..n_points)
        .map(|i| mel_min + (mel_max - mel_min) * i as f64 / (n_points - 1) as f64)
        .collect();

    let hz_points: Vec<f64> = mel_points.iter().map(|&m| mel_to_hz(m)).collect();

    let bin_points: Vec<f64> = hz_points
        .iter()
        .map(|&f| f * n_fft as f64 / sample_rate)
        .collect();

    let mut filterbank = vec![vec![0.0f32; n_fft_bins]; n_mels];

    for m in 0..n_mels {
        let left = bin_points[m];
        let center = bin_points[m + 1];
        let right = bin_points[m + 2];

        for k in 0..n_fft_bins {
            let freq_bin = k as f64;

            if freq_bin >= left && freq_bin <= center {
                let denom = center - left;
                if denom > 0.0 {
                    filterbank[m][k] = ((freq_bin - left) / denom) as f32;
                }
            } else if freq_bin > center && freq_bin <= right {
                let denom = right - center;
                if denom > 0.0 {
                    filterbank[m][k] = ((right - freq_bin) / denom) as f32;
                }
            }
        }
    }

    filterbank
}

fn normalize_features(features: &mut Array2<f32>) {
    let n_frames = features.shape()[0];
    let n_mels = features.shape()[1];

    if n_frames <= 1 {
        return;
    }

    for mel in 0..n_mels {
        let mut sum = 0.0f64;
        for t in 0..n_frames {
            sum += features[[t, mel]] as f64;
        }
        let mean = sum / n_frames as f64;

        let mut var_sum = 0.0f64;
        for t in 0..n_frames {
            let diff = features[[t, mel]] as f64 - mean;
            var_sum += diff * diff;
        }
        let std = (var_sum / n_frames as f64).sqrt().max(1e-5);

        for t in 0..n_frames {
            features[[t, mel]] = ((features[[t, mel]] as f64 - mean) / std) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preemphasis() {
        let samples = vec![1.0, 2.0, 3.0, 4.0];
        let result = apply_preemphasis(&samples, 0.97);
        assert_eq!(result[0], 1.0);
        assert!((result[1] - (2.0 - 0.97)).abs() < 1e-6);
        assert!((result[2] - (3.0 - 0.97 * 2.0)).abs() < 1e-6);
    }

    #[test]
    fn test_hann_window() {
        let w = hann_window(400);
        assert_eq!(w.len(), 400);
        assert!(w[0].abs() < 1e-6);
        assert!((w[200] - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_mel_filterbank_shape() {
        let fb = mel_filterbank(512, 128, 16000.0, 0.0, 8000.0);
        assert_eq!(fb.len(), 128);
        assert_eq!(fb[0].len(), 257);
    }

    #[test]
    fn test_compute_mel_basic() {
        let samples = vec![0.0f32; 16000];
        let config = MelConfig::default();
        let mel = compute_mel_spectrogram(&samples, &config);

        assert_eq!(mel.shape()[0], 98);
        assert_eq!(mel.shape()[1], 128);
    }

    #[test]
    fn test_hz_mel_roundtrip() {
        let freq = 1000.0;
        let mel = hz_to_mel(freq);
        let back = mel_to_hz(mel);
        assert!((freq - back).abs() < 1e-6);
    }

    #[test]
    fn test_mel_output_dimensions() {
        let config = MelConfig::default();
        let signal_length = 16000usize;
        let samples: Vec<f32> = (0..signal_length)
            .map(|i| {
                (2.0 * std::f32::consts::PI * 440.0 * i as f32 / config.sample_rate as f32).sin()
            })
            .collect();
        let mel = compute_mel_spectrogram(&samples, &config);
        let expected_frames = (signal_length - config.win_length) / config.hop_length + 1;
        assert_eq!(mel.shape()[0], expected_frames);
        assert_eq!(mel.shape()[1], 128);
    }

    #[test]
    fn test_mel_silence_produces_low_energy() {
        let config = MelConfig::default();
        let samples = vec![0.0f32; config.win_length];
        let mel = compute_mel_spectrogram(&samples, &config);
        assert_eq!(mel.shape()[0], 1);
        let expected = config.log_floor.ln();
        for mel_idx in 0..config.n_mels {
            assert!(
                (mel[[0, mel_idx]] - expected).abs() < 1e-3,
                "bin {} value {} not close to log floor {}",
                mel_idx,
                mel[[0, mel_idx]],
                expected
            );
        }
    }

    #[test]
    fn test_mel_sine_wave_energy_concentration() {
        let config = MelConfig::default();
        let freq = 1000.0f32;
        let samples: Vec<f32> = (0..config.win_length)
            .map(|i| {
                (2.0 * std::f32::consts::PI * freq * i as f32 / config.sample_rate as f32).sin()
            })
            .collect();
        let mel = compute_mel_spectrogram(&samples, &config);
        assert_eq!(mel.shape()[0], 1);

        let mut max_bin = 0;
        let mut max_val = f32::NEG_INFINITY;
        for m in 0..config.n_mels {
            if mel[[0, m]] > max_val {
                max_val = mel[[0, m]];
                max_bin = m;
            }
        }

        assert!(max_bin > 5);
        assert!(max_bin < config.n_mels - 5);
        assert!(max_val > mel[[0, 0]] + 5.0);
        assert!(max_val > mel[[0, config.n_mels - 1]] + 5.0);
    }

    #[test]
    fn test_mel_preemphasis_effect() {
        let mut samples = vec![0.0f32; 400];
        samples[200] = 1.0;

        let config_with = MelConfig {
            preemphasis: 0.97,
            ..MelConfig::default()
        };
        let config_without = MelConfig {
            preemphasis: 0.0,
            ..MelConfig::default()
        };

        let mel_with = compute_mel_spectrogram(&samples, &config_with);
        let mel_without = compute_mel_spectrogram(&samples, &config_without);

        assert_eq!(mel_with.shape()[0], 1);
        assert_eq!(mel_without.shape()[0], 1);

        let low_mean_with: f32 = (0..20).map(|m| mel_with[[0, m]]).sum::<f32>() / 20.0;
        let high_mean_with: f32 = (108..128).map(|m| mel_with[[0, m]]).sum::<f32>() / 20.0;
        let low_mean_without: f32 = (0..20).map(|m| mel_without[[0, m]]).sum::<f32>() / 20.0;
        let high_mean_without: f32 = (108..128).map(|m| mel_without[[0, m]]).sum::<f32>() / 20.0;

        let high_low_diff_with = high_mean_with - low_mean_with;
        let high_low_diff_without = high_mean_without - low_mean_without;

        assert!(
            high_low_diff_with > high_low_diff_without,
            "preemphasis should boost high freq: with_diff={} without_diff={}",
            high_low_diff_with,
            high_low_diff_without
        );
    }
}
