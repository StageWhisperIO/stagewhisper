const TARGET_SAMPLE_RATE: f64 = 16000.0;
const FALLBACK_SAMPLE_RATE: f64 = 48000.0;

pub(super) fn planar_channels(buffers: &[(usize, &[u8])]) -> Vec<Vec<f32>> {
    let mut channels = Vec::new();
    for &(interleaved_channels, bytes) in buffers {
        let samples: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_ne_bytes(*b))
            .collect();
        let width = interleaved_channels.max(1);
        for channel in 0..width {
            channels.push(
                samples
                    .iter()
                    .skip(channel)
                    .step_by(width)
                    .copied()
                    .collect(),
            );
        }
    }
    channels
}

#[derive(Default)]
pub(super) struct MonoDownsampler {
    position: f64,
}

impl MonoDownsampler {
    pub(super) fn push(&mut self, channels: &[Vec<f32>], source_rate: f64) -> Vec<i16> {
        let Some(samples_per_channel) = channels.first().map(Vec::len) else {
            return Vec::new();
        };
        let source_rate = if source_rate.is_finite() && source_rate > 0.0 {
            source_rate
        } else {
            FALLBACK_SAMPLE_RATE
        };
        let step = source_rate / TARGET_SAMPLE_RATE;
        let num_channels = channels.len() as f32;
        let mut converted = Vec::new();

        while self.position < samples_per_channel as f64 {
            let sample_index = self.position as usize;
            let mixed: f32 = channels
                .iter()
                .filter_map(|channel| channel.get(sample_index))
                .sum::<f32>()
                / num_channels;
            converted.push((mixed.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
            self.position += step;
        }
        self.position -= samples_per_channel as f64;
        converted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_ne_bytes()).collect()
    }

    fn mono_16k(channels: &[Vec<f32>], source_rate: f64) -> Vec<i16> {
        MonoDownsampler::default().push(channels, source_rate)
    }

    #[test]
    fn non_interleaved_buffers_become_one_channel_each() {
        let left = bytes(&[0.1, 0.2]);
        let right = bytes(&[0.3, 0.4]);
        let channels = planar_channels(&[(1, &left), (1, &right)]);
        assert_eq!(channels, vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
    }

    #[test]
    fn an_interleaved_buffer_is_split_into_its_channels() {
        let frames = bytes(&[0.1, 0.3, 0.2, 0.4]);
        let channels = planar_channels(&[(2, &frames)]);
        assert_eq!(channels, vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
    }

    #[test]
    fn stereo_48k_is_averaged_and_decimated_to_16k() {
        let left = vec![0.5_f32; 48];
        let right = vec![-0.5_f32; 48];
        let mono = mono_16k(&[left, vec![0.5_f32; 48]], 48000.0);
        assert_eq!(mono.len(), 16);
        assert!(mono.iter().all(|&s| s == (0.5 * i16::MAX as f32) as i16));
        let silent = mono_16k(&[vec![0.5_f32; 48], right], 48000.0);
        assert!(silent.iter().all(|&s| s == 0));
    }

    #[test]
    fn the_reported_sample_rate_sets_the_decimation() {
        assert_eq!(mono_16k(&[vec![0.0; 24]], 24000.0).len(), 16);
        assert_eq!(mono_16k(&[vec![0.0; 16]], 16000.0).len(), 16);
    }

    #[test]
    fn an_unusable_sample_rate_falls_back_to_48k() {
        assert_eq!(mono_16k(&[vec![0.0; 48]], 0.0).len(), 16);
        assert_eq!(mono_16k(&[vec![0.0; 48]], f64::NAN).len(), 16);
    }

    #[test]
    fn full_scale_is_clamped() {
        let mono = mono_16k(&[vec![2.0; 3]], 48000.0);
        assert_eq!(mono, vec![i16::MAX]);
    }

    #[test]
    fn no_channels_yield_no_audio() {
        assert!(mono_16k(&[], 48000.0).is_empty());
    }

    #[test]
    fn short_io_buffers_keep_the_16k_rate_across_callbacks() {
        let mut downsampler = MonoDownsampler::default();
        let delivered: usize = (0..10)
            .map(|_| downsampler.push(&[vec![0.0; 100]], 48000.0).len())
            .sum();
        assert_eq!(delivered, mono_16k(&[vec![0.0; 1000]], 48000.0).len());
    }

    #[test]
    fn a_44_1k_tap_is_resampled_without_drift() {
        let mut downsampler = MonoDownsampler::default();
        let delivered: usize = (0..441)
            .map(|_| downsampler.push(&[vec![0.0; 100]], 44100.0).len())
            .sum();
        assert!(
            (15999..=16001).contains(&delivered),
            "delivered {delivered}"
        );
    }
}
