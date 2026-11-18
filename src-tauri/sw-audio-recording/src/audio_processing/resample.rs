use anyhow::{Context, Result};

/// Stateful streaming linear resampler.
///
/// Preserves leftover samples and fractional position across calls
/// so callback-sized chunks behave like a single continuous stream.
pub struct StreamingResampler {
    src_rate: u32,
    target_rate: u32,
    step: f64,
    input: Vec<f32>,
    position: f64,
    total_input_samples: usize,
    total_output_samples: usize,
}

impl StreamingResampler {
    pub fn new(src_rate: u32, target_rate: u32) -> Self {
        Self {
            src_rate,
            target_rate,
            step: src_rate as f64 / target_rate as f64,
            input: Vec::new(),
            position: 0.0,
            total_input_samples: 0,
            total_output_samples: 0,
        }
    }

    pub fn process(&mut self, samples: &[f32]) -> Vec<f32> {
        if self.src_rate == self.target_rate {
            self.total_input_samples += samples.len();
            self.total_output_samples += samples.len();
            return samples.to_vec();
        }

        self.input.extend_from_slice(samples);
        self.total_input_samples += samples.len();

        let mut output = Vec::new();
        while self.position + 1.0 < self.input.len() as f64 {
            output.push(self.sample_at(self.position));
            self.position += self.step;
        }

        self.compact_input();
        self.total_output_samples += output.len();
        output
    }

    pub fn finish(&mut self) -> Vec<f32> {
        if self.src_rate == self.target_rate {
            return Vec::new();
        }

        if self.input.is_empty() {
            return Vec::new();
        }

        let target_total = (self.total_input_samples as f64 * self.target_rate as f64
            / self.src_rate as f64)
            .ceil() as usize;

        let mut output = Vec::new();
        while self.total_output_samples + output.len() < target_total {
            output.push(self.sample_at_clamped(self.position));
            self.position += self.step;
        }

        self.total_output_samples += output.len();
        self.input.clear();
        self.position = 0.0;
        output
    }

    fn sample_at(&self, pos: f64) -> f32 {
        let idx = pos.floor() as usize;
        let frac = (pos - idx as f64) as f32;
        self.input[idx] * (1.0 - frac) + self.input[idx + 1] * frac
    }

    fn sample_at_clamped(&self, pos: f64) -> f32 {
        let idx = pos.floor() as usize;
        let frac = (pos - idx as f64) as f32;
        let a = self.input.get(idx).copied().unwrap_or(0.0);
        let b = self.input.get(idx + 1).copied().unwrap_or(a);
        a * (1.0 - frac) + b * frac
    }

    fn compact_input(&mut self) {
        if self.input.len() <= 1 {
            return;
        }

        let consumed = (self.position.floor() as usize).min(self.input.len() - 1);
        if consumed > 0 {
            self.input.drain(..consumed);
            self.position -= consumed as f64;
        }
    }
}

/// Resample audio using linear interpolation. No-op when rates match.
pub fn resample_linear(samples: &[f32], src_rate: u32, target_rate: u32) -> Vec<f32> {
    if src_rate == target_rate {
        return samples.to_vec();
    }

    let ratio = target_rate as f64 / src_rate as f64;
    let output_len = (samples.len() as f64 * ratio).ceil() as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 / ratio;
        let src_idx = src_pos.floor() as usize;
        let frac = (src_pos - src_idx as f64) as f32;

        let sample = if src_idx + 1 < samples.len() {
            samples[src_idx] * (1.0 - frac) + samples[src_idx + 1] * frac
        } else if src_idx < samples.len() {
            samples[src_idx]
        } else {
            0.0
        };
        output.push(sample);
    }

    output
}

/// Convert interleaved multi-channel samples to mono by averaging.
pub fn stereo_to_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    if channels == 1 {
        return samples.to_vec();
    }

    let ch = channels as usize;
    let n_frames = samples.len() / ch;
    let mut mono = Vec::with_capacity(n_frames);

    for i in 0..n_frames {
        let mut sum = 0.0f32;
        for c in 0..ch {
            sum += samples[i * ch + c];
        }
        mono.push(sum / ch as f32);
    }

    mono
}

/// Load a WAV file and return mono 16kHz f32 samples.
pub fn load_wav_file(path: &std::path::Path, verbose: bool) -> Result<Vec<f32>> {
    let reader = hound::WavReader::open(path)
        .with_context(|| format!("Failed to open WAV file: {}", path.display()))?;

    let spec = reader.spec();
    let channels = spec.channels;
    let sample_rate = spec.sample_rate;

    if verbose {
        println!(
            "Audio: {}ch, {}Hz, {:?} {}bit",
            channels, sample_rate, spec.sample_format, spec.bits_per_sample
        );
    }

    let raw_samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max_val = (1u32 << (spec.bits_per_sample - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| {
                    s.map(|sample| sample as f32 / max_val).with_context(|| {
                        format!(
                            "Failed to decode PCM samples from WAV file: {}",
                            path.display()
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?
        }
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .map(|s| {
                s.with_context(|| {
                    format!(
                        "Failed to decode float samples from WAV file: {}",
                        path.display()
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?,
    };

    let mono = stereo_to_mono(&raw_samples, channels);
    let resampled = resample_linear(&mono, sample_rate, 16000);

    if verbose {
        println!(
            "Loaded {} samples ({:.2}s at 16kHz)",
            resampled.len(),
            resampled.len() as f64 / 16000.0
        );
    }

    Ok(resampled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resample_same_rate() {
        let input = vec![1.0, 2.0, 3.0];
        let output = resample_linear(&input, 16000, 16000);
        assert_eq!(output, input);
    }

    #[test]
    fn test_resample_upsample() {
        let input = vec![0.0, 1.0];
        let output = resample_linear(&input, 8000, 16000);
        assert!(output.len() >= 3);
        assert!((output[0]).abs() < 1e-6);
    }

    #[test]
    fn test_stereo_to_mono() {
        let stereo = vec![1.0, 0.0, 0.5, 0.5, 0.0, 1.0];
        let mono = stereo_to_mono(&stereo, 2);
        assert_eq!(mono.len(), 3);
        assert!((mono[0] - 0.5).abs() < 1e-6);
        assert!((mono[1] - 0.5).abs() < 1e-6);
        assert!((mono[2] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_streaming_resampler_matches_one_shot() {
        let input: Vec<f32> = (0..4800)
            .map(|i| ((i as f32) / 19.0).sin() * 0.5 + ((i as f32) / 7.0).cos() * 0.1)
            .collect();

        let expected = resample_linear(&input, 48_000, 16_000);

        let mut streaming = StreamingResampler::new(48_000, 16_000);
        let mut actual = Vec::new();
        for chunk in input.chunks(137) {
            actual.extend(streaming.process(chunk));
        }
        actual.extend(streaming.finish());

        assert_eq!(actual.len(), expected.len());
        for (a, b) in actual.iter().zip(expected.iter()) {
            assert!((a - b).abs() < 1e-4, "streaming={a} expected={b}");
        }
    }

    #[test]
    fn test_streaming_resampler_output_length() {
        let input: Vec<f32> = (0..4800).map(|i| (i as f32 / 10.0).sin()).collect();
        let mut resampler = StreamingResampler::new(48000, 16000);
        let mut output = resampler.process(&input);
        output.extend(resampler.finish());
        assert_eq!(output.len(), 1600);
    }

    #[test]
    fn test_streaming_resampler_multiple_chunks_consistent() {
        let input: Vec<f32> = (0..9600).map(|i| (i as f32 / 13.0).sin() * 0.8).collect();

        let mut single = StreamingResampler::new(48000, 16000);
        let mut single_output = single.process(&input);
        single_output.extend(single.finish());

        let mut multi = StreamingResampler::new(48000, 16000);
        let mut multi_output = Vec::new();
        for chunk in input.chunks(480) {
            multi_output.extend(multi.process(chunk));
        }
        multi_output.extend(multi.finish());

        assert_eq!(single_output.len(), multi_output.len());
        for (i, (a, b)) in single_output.iter().zip(multi_output.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-4,
                "mismatch at {}: single={} multi={}",
                i,
                a,
                b
            );
        }
    }

    #[test]
    fn test_resample_f32_matches_i16() {
        let input: Vec<f32> = (0..4800).map(|i| (i as f32 / 15.0).sin() * 0.5).collect();

        let f32_output = resample_linear(&input, 48000, 16000);
        let i16_output: Vec<i16> = f32_output
            .iter()
            .map(|&s| (s * 32768.0).round().clamp(-32768.0, 32767.0) as i16)
            .collect();

        assert_eq!(f32_output.len(), i16_output.len());
        for (&f_val, &i_val) in f32_output.iter().zip(i16_output.iter()) {
            let f_scaled = f_val * 32768.0;
            assert!(
                (f_scaled - i_val as f32).abs() <= 1.0,
                "f32 scaled {} vs i16 {}",
                f_scaled,
                i_val
            );
        }
    }
}
