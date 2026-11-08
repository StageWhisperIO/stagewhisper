use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::Sender;

use super::{Control, Fanin};
use crate::loopback_binder::Endpoint;

type LegResult<T> = Result<T, String>;

const TARGET_SAMPLE_RATE: f32 = 16000.0;

pub(super) struct EndpointLeg {
    capture_stream: cpal::Stream,
    keepalive_stream: cpal::Stream,
    format: String,
}

impl EndpointLeg {
    pub(super) fn open(
        role: &'static str,
        endpoint: &Endpoint,
        source: usize,
        fanin: &Arc<Mutex<Fanin>>,
        control: &Sender<Control>,
    ) -> LegResult<Self> {
        let host = cpal::default_host();
        let device = host
            .device_by_id(&cpal::DeviceId(cpal::HostId::Wasapi, endpoint.id.clone()))
            .ok_or_else(|| format!("{role} endpoint {endpoint} is no longer available"))?;
        let supported_config = device
            .default_output_config()
            .map_err(|err| format!("{role} endpoint {endpoint} has no output format: {err}"))?;
        let sample_rate = supported_config.sample_rate() as f32;
        let channels = supported_config.channels() as usize;
        let sample_format = supported_config.sample_format();
        if channels == 0 {
            return Err(format!("{role} endpoint {endpoint} reports zero channels"));
        }
        let ratio = sample_rate / TARGET_SAMPLE_RATE;
        let config: cpal::StreamConfig = supported_config.into();

        let capture_stream = build_capture_stream(
            &device,
            &config,
            sample_format,
            channels,
            ratio,
            source,
            fanin,
            failure_reporter(role, control, "loopback"),
        )
        .map_err(|err| format!("{role} endpoint {endpoint} loopback failed: {err}"))?;
        let keepalive_stream = build_keepalive_stream(
            &device,
            &config,
            sample_format,
            failure_reporter(role, control, "keep-alive"),
        )
        .map_err(|err| format!("{role} endpoint {endpoint} keep-alive failed: {err}"))?;

        Ok(Self {
            capture_stream,
            keepalive_stream,
            format: format!("{sample_rate} Hz, {channels} ch, {sample_format:?}"),
        })
    }

    pub(super) fn format(&self) -> &str {
        &self.format
    }

    pub(super) fn play(&self) -> LegResult<()> {
        self.capture_stream
            .play()
            .map_err(|err| format!("loopback did not start: {err}"))?;
        self.keepalive_stream
            .play()
            .map_err(|err| format!("keep-alive did not start: {err}"))
    }
}

fn failure_reporter(
    role: &'static str,
    control: &Sender<Control>,
    stream: &'static str,
) -> impl FnMut(cpal::StreamError) + Send + 'static {
    let control = control.clone();
    move |err: cpal::StreamError| {
        log::warn!("[audio] system {stream} stream error on the {role} endpoint: {err}");
        let _ = control.send(Control::LegFailed);
    }
}

#[allow(clippy::too_many_arguments)]
fn build_capture_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    channels: usize,
    ratio: f32,
    source: usize,
    fanin: &Arc<Mutex<Fanin>>,
    on_error: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    let fanin = Arc::clone(fanin);
    let emit = move |samples: &[f32]| {
        let chunk = downmix_resample(samples, channels, ratio);
        if chunk.is_empty() {
            return;
        }
        if let Ok(mut fanin) = fanin.lock() {
            fanin.push(source, &chunk);
        }
    };
    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| emit(data),
            on_error,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_input_stream(
            config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                let samples: Vec<f32> = data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                emit(&samples);
            },
            on_error,
            None,
        )?,
        cpal::SampleFormat::I32 => device.build_input_stream(
            config,
            move |data: &[i32], _: &cpal::InputCallbackInfo| {
                let samples: Vec<f32> = data.iter().map(|&s| s as f32 / i32::MAX as f32).collect();
                emit(&samples);
            },
            on_error,
            None,
        )?,
        cpal::SampleFormat::U16 => device.build_input_stream(
            config,
            move |data: &[u16], _: &cpal::InputCallbackInfo| {
                let samples: Vec<f32> = data
                    .iter()
                    .map(|&s| (s as f32 - 32768.0) / 32768.0)
                    .collect();
                emit(&samples);
            },
            on_error,
            None,
        )?,
        unsupported => {
            return Err(format!("unsupported system audio sample format: {unsupported:?}").into());
        }
    };
    Ok(stream)
}

fn build_keepalive_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    on_error: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| data.fill(0.0),
            on_error,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_output_stream(
            config,
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| data.fill(0),
            on_error,
            None,
        )?,
        cpal::SampleFormat::I32 => device.build_output_stream(
            config,
            move |data: &mut [i32], _: &cpal::OutputCallbackInfo| data.fill(0),
            on_error,
            None,
        )?,
        cpal::SampleFormat::U16 => device.build_output_stream(
            config,
            move |data: &mut [u16], _: &cpal::OutputCallbackInfo| data.fill(32768),
            on_error,
            None,
        )?,
        unsupported => {
            return Err(format!("unsupported system audio sample format: {unsupported:?}").into());
        }
    };
    Ok(stream)
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
