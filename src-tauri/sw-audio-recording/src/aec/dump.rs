use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use hound::{SampleFormat, WavSpec, WavWriter};

const DUMP_DIR_ENV: &str = "STAGEWHISPER_AEC_DUMP_DIR";

type Writer = WavWriter<BufWriter<File>>;

pub(super) struct AecDump {
    near: Writer,
    far: Writer,
    cleaned: Writer,
}

fn to_pcm(sample: f32) -> i16 {
    let finite = if sample.is_finite() { sample } else { 0.0 };
    (finite.clamp(-1.0, 1.0) * 32767.0) as i16
}

impl AecDump {
    pub(super) fn from_env() -> Option<Self> {
        if !cfg!(debug_assertions) {
            return None;
        }
        let dir = PathBuf::from(std::env::var_os(DUMP_DIR_ENV)?);
        std::fs::create_dir_all(&dir).ok()?;
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        let spec = WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let open =
            |role: &str| WavWriter::create(dir.join(format!("aec-{stamp}-{role}.wav")), spec).ok();
        let dump = Self {
            near: open("near")?,
            far: open("far")?,
            cleaned: open("cleaned")?,
        };
        log::warn!(
            "[aec] debug build: writing microphone, reference and cleaned audio to {}",
            dir.display()
        );
        Some(dump)
    }

    pub(super) fn write(&mut self, near: &[f32], far: &[f32], cleaned: &[f32]) {
        for ((&near_sample, &far_sample), &cleaned_sample) in near.iter().zip(far).zip(cleaned) {
            let _ = self.near.write_sample(to_pcm(near_sample));
            let _ = self.far.write_sample(to_pcm(far_sample));
            let _ = self.cleaned.write_sample(to_pcm(cleaned_sample));
        }
    }
}
