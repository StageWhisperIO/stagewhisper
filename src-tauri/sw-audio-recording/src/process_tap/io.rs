use std::ffi::c_void;
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::NonNull;

use objc2_core_audio::AudioObjectID;
use objc2_core_audio_types::{
    kAudioFormatFlagIsBigEndian, kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved,
    kAudioFormatLinearPCM, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp,
};

use super::convert::{planar_channels, MonoDownsampler};
use super::SharedAudioCallback;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TapFormat {
    pub(super) sample_rate: f64,
    channels: u32,
    bits_per_channel: u32,
    float: bool,
    interleaved: bool,
    linear_pcm: bool,
    big_endian: bool,
}

impl TapFormat {
    pub(super) fn from_description(description: &AudioStreamBasicDescription) -> Self {
        Self {
            sample_rate: description.mSampleRate,
            channels: description.mChannelsPerFrame,
            bits_per_channel: description.mBitsPerChannel,
            float: description.mFormatFlags & kAudioFormatFlagIsFloat != 0,
            interleaved: description.mFormatFlags & kAudioFormatFlagIsNonInterleaved == 0,
            linear_pcm: description.mFormatID == kAudioFormatLinearPCM,
            big_endian: description.mFormatFlags & kAudioFormatFlagIsBigEndian != 0,
        }
    }

    pub(super) fn is_supported(&self) -> bool {
        self.linear_pcm
            && !self.big_endian
            && self.float
            && self.bits_per_channel == 32
            && self.channels > 0
    }
}

impl fmt::Display for TapFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} Hz, {} ch, {}-bit {}, {}",
            self.sample_rate,
            self.channels,
            self.bits_per_channel,
            if self.float { "float" } else { "integer" },
            if self.interleaved {
                "interleaved"
            } else {
                "non-interleaved"
            }
        )
    }
}

pub(super) struct IoContext {
    on_audio_data: SharedAudioCallback,
    sample_rate: f64,
    downsampler: MonoDownsampler,
}

impl IoContext {
    pub(super) fn new(on_audio_data: SharedAudioCallback, format: TapFormat) -> Self {
        Self {
            on_audio_data,
            sample_rate: format.sample_rate,
            downsampler: MonoDownsampler::default(),
        }
    }

    fn deliver(&mut self, input: &AudioBufferList) {
        let buffers = buffer_bytes(input);
        let chunk = self
            .downsampler
            .push(&planar_channels(&buffers), self.sample_rate);
        if chunk.is_empty() {
            return;
        }
        if let Ok(mut on_audio_data) = self.on_audio_data.lock() {
            (*on_audio_data)(chunk);
        }
    }
}

fn buffer_bytes(input: &AudioBufferList) -> Vec<(usize, &[u8])> {
    let buffers = unsafe {
        std::slice::from_raw_parts(input.mBuffers.as_ptr(), input.mNumberBuffers as usize)
    };
    buffers
        .iter()
        .filter(|buffer| !buffer.mData.is_null() && buffer.mDataByteSize > 0)
        .map(|buffer| {
            let bytes = unsafe {
                std::slice::from_raw_parts(buffer.mData.cast::<u8>(), buffer.mDataByteSize as usize)
            };
            (buffer.mNumberChannels as usize, bytes)
        })
        .collect()
}

pub(super) unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    _output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client_data: *mut c_void,
) -> i32 {
    let Some(mut context) = NonNull::new(client_data.cast::<IoContext>()) else {
        return 0;
    };
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        context.as_mut().deliver(input.as_ref())
    }));
    0
}
