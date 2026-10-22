use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

type SharedAudioCallback = Arc<Mutex<dyn FnMut(Vec<i16>) + Send>>;

const TARGET_SAMPLE_RATE: f32 = 16000.0;

pub struct LoopbackCapture {
    on_audio_data: SharedAudioCallback,
    capture_stream: Option<cpal::Stream>,
    keepalive_stream: Option<cpal::Stream>,
}

pub fn build_system_output_stream_streaming(
    on_audio_data: impl FnMut(Vec<i16>) + Send + 'static,
) -> Result<LoopbackCapture, Box<dyn std::error::Error>> {
    Ok(LoopbackCapture {
        on_audio_data: Arc::new(Mutex::new(on_audio_data)),
        capture_stream: None,
        keepalive_stream: None,
    })
}

impl LoopbackCapture {
    pub fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("no system audio output device available")?;

        let supported_config = device.default_output_config()?;
        let sample_rate = supported_config.sample_rate() as f32;
        let channels = supported_config.channels() as usize;
        let sample_format = supported_config.sample_format();

        if channels == 0 {
            return Err("system audio output device reports zero channels".into());
        }

        let ratio = sample_rate / TARGET_SAMPLE_RATE;
        let config: cpal::StreamConfig = supported_config.into();

        let err_fn = |err: cpal::StreamError| {
            log::error!("system loopback stream error: {}", err);
        };

        let callback = Arc::clone(&self.on_audio_data);
        let capture_stream = match sample_format {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    emit_chunk(&callback, downmix_resample(data, channels, ratio));
                },
                err_fn,
                None,
            )?,
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let samples: Vec<f32> =
                        data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                    emit_chunk(&callback, downmix_resample(&samples, channels, ratio));
                },
                err_fn,
                None,
            )?,
            cpal::SampleFormat::I32 => device.build_input_stream(
                &config,
                move |data: &[i32], _: &cpal::InputCallbackInfo| {
                    let samples: Vec<f32> =
                        data.iter().map(|&s| s as f32 / i32::MAX as f32).collect();
                    emit_chunk(&callback, downmix_resample(&samples, channels, ratio));
                },
                err_fn,
                None,
            )?,
            cpal::SampleFormat::U16 => device.build_input_stream(
                &config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    let samples: Vec<f32> = data
                        .iter()
                        .map(|&s| (s as f32 - 32768.0) / 32768.0)
                        .collect();
                    emit_chunk(&callback, downmix_resample(&samples, channels, ratio));
                },
                err_fn,
                None,
            )?,
            unsupported => {
                return Err(
                    format!("unsupported system audio sample format: {:?}", unsupported).into(),
                );
            }
        };

        let keepalive_stream = build_keepalive_stream(&device, &config, sample_format)?;

        capture_stream.play()?;
        keepalive_stream.play()?;

        self.capture_stream = Some(capture_stream);
        self.keepalive_stream = Some(keepalive_stream);

        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.keepalive_stream.take();
        self.capture_stream.take();
        Ok(())
    }
}

fn build_keepalive_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    let err_fn = |err: cpal::StreamError| {
        log::error!("system loopback keep-alive stream error: {}", err);
    };

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| data.fill(0.0),
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_output_stream(
            config,
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| data.fill(0),
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I32 => device.build_output_stream(
            config,
            move |data: &mut [i32], _: &cpal::OutputCallbackInfo| data.fill(0),
            err_fn,
            None,
        )?,
        cpal::SampleFormat::U16 => device.build_output_stream(
            config,
            move |data: &mut [u16], _: &cpal::OutputCallbackInfo| data.fill(32768),
            err_fn,
            None,
        )?,
        unsupported => {
            return Err(
                format!("unsupported system audio sample format: {:?}", unsupported).into(),
            );
        }
    };

    Ok(stream)
}

fn emit_chunk(callback: &SharedAudioCallback, chunk: Vec<i16>) {
    if chunk.is_empty() {
        return;
    }
    if let Ok(mut on_audio_data) = callback.lock() {
        (&mut *on_audio_data)(chunk);
    }
}

fn downmix_resample(data: &[f32], channels: usize, ratio: f32) -> Vec<i16> {
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

    converted
}
