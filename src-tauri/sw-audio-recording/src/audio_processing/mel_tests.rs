use super::*;

fn three_tone_second() -> Vec<f32> {
    (0..16000)
        .map(|n| {
            let t = n as f64 / 16000.0;
            let tau = 2.0 * std::f64::consts::PI;
            (0.3 * (tau * 220.0 * t).sin()
                + 0.2 * (tau * 1250.0 * t).sin()
                + 0.1 * (tau * 3100.0 * t).sin() * t) as f32
        })
        .collect()
}

#[test]
fn features_match_nemo_for_the_same_audio() {
    let features = compute_mel_spectrogram(&three_tone_second(), &MelConfig::default());

    let nemo = [
        (0, [6.4063, -9.8974, 7.8482, 8.4934]),
        (37, [-0.4078, 0.0944, -0.1477, -0.1595]),
        (99, [4.2919, 0.1901, 4.0803, 3.6837]),
    ];
    for (frame, expected) in nemo {
        for (mel, value) in [0, 10, 64, 127].into_iter().zip(expected) {
            let ours = features[[frame, mel]];
            assert!(
                (ours - value).abs() < 2e-3,
                "frame {frame} mel {mel}: ours {ours}, NeMo {value}"
            );
        }
    }
}

#[test]
fn the_filterbank_is_librosas_slaney_filterbank() {
    let bank = mel_filterbank(512, 128, 16000.0, 0.0, 8000.0);
    let nonzero = |mel: usize| -> Vec<(usize, f32)> {
        bank[mel]
            .iter()
            .copied()
            .enumerate()
            .filter(|&(_, weight)| weight > 0.0)
            .collect()
    };
    let close = |ours: f32, librosa: f32| (ours - librosa).abs() < 1e-6;

    assert_eq!(nonzero(0).len(), 1);
    assert_eq!(nonzero(0)[0].0, 1);
    assert!(close(nonzero(0)[0].1, 0.028_377_5));
    assert_eq!(nonzero(10)[0].0, 8);
    assert!(close(nonzero(10)[0].1, 0.029_579));
    let middle: Vec<f32> = nonzero(64).iter().map(|&(_, weight)| weight).collect();
    assert_eq!(nonzero(64)[0].0, 54);
    for (ours, librosa) in middle.iter().zip([0.011_216_7, 0.018_818_3, 0.000_649_1]) {
        assert!(close(*ours, librosa), "ours {ours}, librosa {librosa}");
    }
    let top = nonzero(127);
    assert_eq!((top[0].0, top[top.len() - 1].0), (244, 255));
    assert!(close(
        top.iter().map(|&(_, weight)| weight).sum(),
        0.031_943_7
    ));
}

#[test]
fn there_is_one_frame_per_hop() {
    let config = MelConfig::default();
    let features = compute_mel_spectrogram(&vec![0.1f32; 16000], &config);
    assert_eq!(features.shape(), &[100, 128]);
}

#[test]
fn audio_shorter_than_one_hop_has_no_frames() {
    let features = compute_mel_spectrogram(&[0.5f32; 100], &MelConfig::default());
    assert_eq!(features.shape(), &[0, 128]);
}

#[test]
fn silence_sits_at_the_log_guard() {
    let config = MelConfig::default();
    let energies = log_mel_energies(&vec![0.0f32; 1600], &config);
    let floor = config.log_guard.ln();
    assert!(energies.iter().all(|&value| (value - floor).abs() < 1e-3));
}

#[test]
fn a_tone_lands_in_the_middle_bands() {
    let config = MelConfig::default();
    let samples: Vec<f32> = (0..1600)
        .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 16000.0).sin())
        .collect();
    let energies = log_mel_energies(&samples, &config);
    let frame = energies.row(5);
    let loudest = frame
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;

    assert!(loudest > 5 && loudest < config.n_mels - 5);
    assert!(frame[loudest] > frame[0] + 5.0);
    assert!(frame[loudest] > frame[config.n_mels - 1] + 5.0);
}

#[test]
fn preemphasis_subtracts_the_previous_sample() {
    let result = apply_preemphasis(&[1.0, 2.0, 3.0, 4.0], 0.97);
    assert_eq!(result[0], 1.0);
    assert!((result[1] - (2.0 - 0.97)).abs() < 1e-6);
    assert!((result[2] - (3.0 - 0.97 * 2.0)).abs() < 1e-6);
}

#[test]
fn the_window_is_a_symmetric_hann() {
    let window = hann_window(400);
    assert_eq!(window.len(), 400);
    assert!(window[0].abs() < 1e-7);
    assert!(window[399].abs() < 1e-7);
    assert!((window[150] - window[249]).abs() < 1e-6);
    assert!((window[199] - 1.0).abs() < 1e-4);
}

#[test]
fn the_mel_scale_round_trips_on_both_sides_of_1khz() {
    for freq in [440.0, 1000.0, 4000.0] {
        assert!((mel_to_hz(hz_to_mel(freq)) - freq).abs() < 1e-6);
    }
    assert!((hz_to_mel(1000.0) - 15.0).abs() < 1e-9);
}
