pub mod aec;
pub mod asr;
pub mod audio_processing;
#[cfg(feature = "diarization")]
pub mod diarization;
pub mod download;
pub mod pipeline;
pub mod vad;
#[cfg(feature = "vocab-boost")]
pub mod vocab;

use hound::WavWriter;
use qruhear::{rucallback, RUBuffers, RUHear};
use std::{
    fs::File,
    io::BufWriter,
    path::Path,
    sync::{Arc, Mutex},
};

pub fn build_system_output_stream(
    wav_writer: &Arc<Mutex<WavWriter<BufWriter<File>>>>,
    on_audio_data: impl FnMut(Vec<i16>) + Send + 'static,
) -> Result<RUHear, Box<dyn std::error::Error>> {
    let writer = wav_writer.clone();
    let mut on_audio_data = on_audio_data;

    let original_sample_rate = 48000.0;
    let target_sample_rate = 16000.0;
    let ratio = original_sample_rate / target_sample_rate;

    let callback = move |audio_buffers: RUBuffers| {
        if let Ok(mut writer_guard) = writer.try_lock() {
            let mut converted_samples = Vec::new();

            if audio_buffers.is_empty() {
                return;
            }

            let num_channels = audio_buffers.len();
            let samples_per_channel = audio_buffers[0].len();

            let mut i = 0.0;
            while i < samples_per_channel as f32 {
                let sample_index = i as usize;
                if sample_index < samples_per_channel {
                    // Mix all channels to mono by averaging
                    let mut mixed_sample = 0.0;
                    for channel_data in &audio_buffers {
                        if sample_index < channel_data.len() {
                            mixed_sample += channel_data[sample_index];
                        }
                    }
                    mixed_sample /= num_channels as f32; // Average the channels

                    if let Err(e) = writer_guard.write_sample(mixed_sample) {
                        log::error!("Failed to write sample: {}", e);
                        break;
                    }

                    let sample_i16 = (mixed_sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    converted_samples.push(sample_i16);
                }

                i += ratio;
            }

            if !converted_samples.is_empty() {
                on_audio_data(converted_samples);
            }
        }
    };

    let callback = rucallback!(callback);
    let ruhear = RUHear::new(callback);

    Ok(ruhear)
}

pub fn build_system_output_stream_streaming(
    on_audio_data: impl FnMut(Vec<i16>) + Send + 'static,
) -> Result<RUHear, Box<dyn std::error::Error>> {
    let mut on_audio_data = on_audio_data;

    let original_sample_rate = 48000.0;
    let target_sample_rate = 16000.0;
    let ratio = original_sample_rate / target_sample_rate;

    let callback = move |audio_buffers: RUBuffers| {
        if audio_buffers.is_empty() {
            return;
        }

        let num_channels = audio_buffers.len();
        let samples_per_channel = audio_buffers[0].len();
        let mut converted_samples = Vec::new();

        let mut i = 0.0;
        while i < samples_per_channel as f32 {
            let sample_index = i as usize;
            if sample_index < samples_per_channel {
                let mut mixed_sample = 0.0;
                for channel_data in &audio_buffers {
                    if sample_index < channel_data.len() {
                        mixed_sample += channel_data[sample_index];
                    }
                }
                mixed_sample /= num_channels as f32;

                let sample_i16 = (mixed_sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                converted_samples.push(sample_i16);
            }

            i += ratio;
        }

        if !converted_samples.is_empty() {
            on_audio_data(converted_samples);
        }
    };

    let callback = rucallback!(callback);
    let ruhear = RUHear::new(callback);

    Ok(ruhear)
}

pub fn build_system_output_stream_to_wav(
    wav_path: impl AsRef<Path>,
    on_audio_data: impl FnMut(Vec<i16>) + Send + 'static,
) -> Result<RUHear, Box<dyn std::error::Error>> {
    let wav_spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let wav_file = BufWriter::new(File::create(wav_path)?);
    let wav_writer = Arc::new(Mutex::new(WavWriter::new(wav_file, wav_spec)?));
    build_system_output_stream(&wav_writer, on_audio_data)
}

pub fn build_mic_input_stream(
    mut on_mic_data: impl FnMut(Vec<i16>) + Send + 'static,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or("no microphone input device available")?;

    let supported_config = device.default_input_config()?;
    let sample_rate = supported_config.sample_rate() as f32;
    let channels = supported_config.channels() as usize;
    let sample_format = supported_config.sample_format();

    let target_sample_rate = 16000.0_f32;
    let ratio = sample_rate / target_sample_rate;

    let config: cpal::StreamConfig = supported_config.into();

    let err_fn = |err: cpal::StreamError| {
        log::error!("mic input stream error: {}", err);
    };

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let samples_per_channel = data.len() / channels;
                let mut converted = Vec::new();

                let mut i = 0.0_f32;
                while i < samples_per_channel as f32 {
                    let idx = i as usize;
                    if idx < samples_per_channel {
                        let mut mixed: f32 = 0.0;
                        for ch in 0..channels {
                            let pos = idx * channels + ch;
                            if pos < data.len() {
                                mixed += data[pos];
                            }
                        }
                        mixed /= channels as f32;
                        let sample_i16 = (mixed.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        converted.push(sample_i16);
                    }
                    i += ratio;
                }

                if !converted.is_empty() {
                    on_mic_data(converted);
                }
            },
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                let samples_per_channel = data.len() / channels;
                let mut converted = Vec::new();

                let mut i = 0.0_f32;
                while i < samples_per_channel as f32 {
                    let idx = i as usize;
                    if idx < samples_per_channel {
                        let mut mixed: f32 = 0.0;
                        for ch in 0..channels {
                            let pos = idx * channels + ch;
                            if pos < data.len() {
                                mixed += data[pos] as f32 / i16::MAX as f32;
                            }
                        }
                        mixed /= channels as f32;
                        let sample_i16 = (mixed.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        converted.push(sample_i16);
                    }
                    i += ratio;
                }

                if !converted.is_empty() {
                    on_mic_data(converted);
                }
            },
            err_fn,
            None,
        )?,
        unsupported => {
            return Err(format!("unsupported mic sample format: {:?}", unsupported).into());
        }
    };

    stream.play()?;

    Ok(stream)
}
