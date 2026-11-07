//! Audio for export: a track decoded to mono PCM by AVFoundation, and AAC encode with AudioToolbox.
//! Audio is small, so unlike video it passes through the CPU.

use std::ffi::c_void;
use std::path::Path;
use std::ptr::{self, NonNull};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_audio_toolbox::{
    AudioConverterDispose, AudioConverterFillComplexBuffer, AudioConverterGetProperty,
    AudioConverterNew, AudioConverterPrimeInfo, AudioConverterRef, AudioConverterSetProperty,
    kAudioConverterCodecQuality, kAudioConverterEncodeBitRate, kAudioConverterPrimeInfo,
    kAudioConverterPropertyMaximumOutputPacketSize, kAudioConverterQuality_High,
};
use objc2_av_foundation::{
    AVAssetReader, AVAssetReaderStatus, AVAssetReaderTrackOutput, AVAssetTrack, AVMediaTypeAudio,
    AVURLAsset,
};
use objc2_avf_audio::{
    AVFormatIDKey, AVLinearPCMBitDepthKey, AVLinearPCMIsBigEndianKey, AVLinearPCMIsFloatKey,
    AVLinearPCMIsNonInterleaved, AVNumberOfChannelsKey, AVSampleRateKey,
};
use objc2_core_audio_types::{
    AudioBufferList, AudioStreamBasicDescription, AudioStreamPacketDescription,
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked, kAudioFormatLinearPCM, kAudioFormatMPEG4AAC,
};
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeRange, kCMTimePositiveInfinity};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

/// The export's audio sample rate.
pub const SAMPLE_RATE: u32 = 48_000;

/// Samples per AAC packet.
pub const AAC_PACKET_SAMPLES: u32 = 1024;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("{0} has no audio track")]
    NoAudioTrack(String),
    #[error("reading audio failed: {0}")]
    Reader(String),
    #[error("AAC encoder: {0} failed with OSStatus {1}")]
    Converter(&'static str, i32),
}

/// Mono [`SAMPLE_RATE`] samples of a file's first audio track, read forward from any position.
pub struct PcmReader {
    asset: Retained<AVURLAsset>,
    track: Retained<AVAssetTrack>,
    reader: Retained<AVAssetReader>,
    output: Retained<AVAssetReaderTrackOutput>,
    /// Decoded samples starting at sample `buffer_start`.
    buffer: Vec<f32>,
    buffer_start: u64,
    end_of_track: bool,
}

impl PcmReader {
    pub fn open(path: &Path) -> Result<Self, AudioError> {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) };
        let media_type = unsafe { AVMediaTypeAudio }.expect("AVFoundation defines the audio type");
        // See the video decoder: a local file loads at once, and the async loader's block is not Send.
        #[allow(deprecated)]
        let track = unsafe { asset.tracksWithMediaType(media_type) }
            .firstObject()
            .ok_or_else(|| AudioError::NoAudioTrack(path.display().to_string()))?;
        let (reader, output) = start_reader(&asset, &track, 0)?;
        Ok(Self {
            asset,
            track,
            reader,
            output,
            buffer: Vec::new(),
            buffer_start: 0,
            end_of_track: false,
        })
    }

    /// Appends samples `from..to` to `out`, as silence past the end of the track.
    /// Reading forward continues decoding; reading backward restarts the reader.
    pub fn read(&mut self, from: u64, to: u64, out: &mut Vec<f32>) -> Result<(), AudioError> {
        let mut at = from;
        if at < self.buffer_start {
            (self.reader, self.output) = start_reader(&self.asset, &self.track, at)?;
            self.buffer.clear();
            self.buffer_start = at;
            self.end_of_track = false;
        }
        while at < to {
            let buffer_end = self.buffer_start + self.buffer.len() as u64;
            if (self.buffer_start..buffer_end).contains(&at) {
                let start = (at - self.buffer_start) as usize;
                let end = (to.min(buffer_end) - self.buffer_start) as usize;
                out.extend_from_slice(&self.buffer[start..end]);
                at += (end - start) as u64;
                continue;
            }
            let next = if self.end_of_track {
                None
            } else {
                next_samples(&self.reader, &self.output)?
            };
            let Some((start, samples)) = next else {
                self.end_of_track = true;
                out.resize(out.len() + (to - at) as usize, 0.0);
                break;
            };
            if start > at {
                let gap = to.min(start) - at;
                out.resize(out.len() + gap as usize, 0.0);
                at += gap;
            }
            self.buffer = samples;
            self.buffer_start = start;
        }
        Ok(())
    }

    /// Whether every sample from `at` on lies past the end of the track. Known only once a
    /// read has reached the end.
    pub fn ended_before(&self, at: u64) -> bool {
        self.end_of_track && at >= self.buffer_start + self.buffer.len() as u64
    }
}

impl Drop for PcmReader {
    fn drop(&mut self) {
        unsafe { self.reader.cancelReading() };
    }
}

fn start_reader(
    asset: &AVURLAsset,
    track: &AVAssetTrack,
    from: u64,
) -> Result<(Retained<AVAssetReader>, Retained<AVAssetReaderTrackOutput>), AudioError> {
    let reader = unsafe { AVAssetReader::assetReaderWithAsset_error(asset) }
        .map_err(|error| AudioError::Reader(error.localizedDescription().to_string()))?;
    // SAFETY: Constant NSStrings from AVFAudio.
    let keys: [&NSString; 7] = unsafe {
        [
            AVFormatIDKey,
            AVSampleRateKey,
            AVNumberOfChannelsKey,
            AVLinearPCMBitDepthKey,
            AVLinearPCMIsFloatKey,
            AVLinearPCMIsBigEndianKey,
            AVLinearPCMIsNonInterleaved,
        ]
    }
    .map(|key| key.expect("AVFAudio defines the settings keys"));
    let values: [Retained<NSNumber>; 7] = [
        NSNumber::new_u32(kAudioFormatLinearPCM),
        NSNumber::new_u32(SAMPLE_RATE),
        NSNumber::new_u32(1),
        NSNumber::new_u32(32),
        NSNumber::new_bool(true),
        NSNumber::new_bool(false),
        NSNumber::new_bool(false),
    ];
    let values: Vec<&AnyObject> = values.iter().map(|v| v.as_ref()).collect();
    let settings = NSDictionary::<NSString, AnyObject>::from_slices(&keys, &values);
    let output = unsafe {
        AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(
            track,
            Some(&settings),
        )
    };
    unsafe {
        output.setAlwaysCopiesSampleData(false);
        reader.setTimeRange(CMTimeRange {
            start: CMTime::new(from as i64, SAMPLE_RATE as i32),
            duration: kCMTimePositiveInfinity,
        });
        reader.addOutput(&output);
        if !reader.startReading() {
            return Err(reader_error(&reader));
        }
    }
    Ok((reader, output))
}

/// The next decoded buffer as its first sample index and its samples.
fn next_samples(
    reader: &AVAssetReader,
    output: &AVAssetReaderTrackOutput,
) -> Result<Option<(u64, Vec<f32>)>, AudioError> {
    loop {
        let Some(sample) = (unsafe { output.copyNextSampleBuffer() }) else {
            if unsafe { reader.status() } == AVAssetReaderStatus::Failed {
                return Err(reader_error(reader));
            }
            return Ok(None);
        };
        if let Some(samples) = pcm(&sample) {
            let pts = unsafe { sample.presentation_time_stamp() };
            let start = if pts.timescale > 0 && pts.value > 0 {
                ((pts.value as i128 * i128::from(SAMPLE_RATE) + i128::from(pts.timescale / 2))
                    / i128::from(pts.timescale)) as u64
            } else {
                0
            };
            return Ok(Some((start, samples)));
        }
    }
}

fn pcm(sample: &CMSampleBuffer) -> Option<Vec<f32>> {
    let block = unsafe { sample.data_buffer() }?;
    let length = unsafe { block.data_length() };
    if length == 0 {
        return None;
    }
    let mut samples = vec![0f32; length / 4];
    // SAFETY: The destination holds `length` bytes rounded down to whole samples.
    unsafe {
        block.copy_data_bytes(0, samples.len() * 4, NonNull::from(&mut samples[..]).cast());
    }
    Some(samples)
}

fn reader_error(reader: &AVAssetReader) -> AudioError {
    let message = unsafe { reader.error() }
        .map(|error| error.localizedDescription().to_string())
        .unwrap_or_else(|| "unknown reader error".into());
    AudioError::Reader(message)
}

/// AAC-LC packets with what an MP4 track needs to play them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AacStream {
    /// Packets back to back, each [`AAC_PACKET_SAMPLES`] long.
    pub data: Vec<u8>,
    pub sizes: Vec<u32>,
    /// Encoder delay in samples before the first input sample.
    pub priming: u32,
    pub bitrate: u32,
    /// The MPEG-4 AudioSpecificConfig.
    pub config: Vec<u8>,
}

/// Returned by the input callback when it has no more samples for now.
const NEEDS_INPUT: i32 = 1;

/// What the input callback reads from.
struct Input {
    samples: Vec<f32>,
    /// Samples already handed to the converter.
    consumed: usize,
    end_of_stream: bool,
}

/// Mono AAC-LC encoder fed in chunks of any size.
pub struct AacEncoder {
    converter: AudioConverterRef,
    input: Box<Input>,
    max_packet_size: u32,
    stream: AacStream,
}

impl AacEncoder {
    pub fn new(sample_rate: u32, bitrate: u32) -> Result<Self, AudioError> {
        let source = AudioStreamBasicDescription {
            mSampleRate: f64::from(sample_rate),
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
            mBytesPerPacket: 4,
            mFramesPerPacket: 1,
            mBytesPerFrame: 4,
            mChannelsPerFrame: 1,
            mBitsPerChannel: 32,
            mReserved: 0,
        };
        let destination = AudioStreamBasicDescription {
            mSampleRate: f64::from(sample_rate),
            mFormatID: kAudioFormatMPEG4AAC,
            mFormatFlags: 0,
            mBytesPerPacket: 0,
            mFramesPerPacket: AAC_PACKET_SAMPLES,
            mBytesPerFrame: 0,
            mChannelsPerFrame: 1,
            mBitsPerChannel: 0,
            mReserved: 0,
        };
        let mut converter = ptr::null_mut();
        // SAFETY: Both descriptions are valid and the out pointer is writable.
        let status = unsafe {
            AudioConverterNew(
                NonNull::from(&source),
                NonNull::from(&destination),
                NonNull::from(&mut converter),
            )
        };
        if status != 0 || converter.is_null() {
            return Err(AudioError::Converter("creating the converter", status));
        }
        let mut encoder = Self {
            converter,
            input: Box::new(Input {
                samples: Vec::new(),
                consumed: 0,
                end_of_stream: false,
            }),
            max_packet_size: 0,
            stream: AacStream {
                bitrate,
                config: audio_specific_config(sample_rate),
                ..AacStream::default()
            },
        };
        encoder.set(
            kAudioConverterCodecQuality,
            kAudioConverterQuality_High,
            "quality",
        )?;
        encoder.set(kAudioConverterEncodeBitRate, bitrate, "bitrate")?;
        encoder.max_packet_size = encoder.get(
            kAudioConverterPropertyMaximumOutputPacketSize,
            0u32,
            "max packet size",
        )?;
        let prime = AudioConverterPrimeInfo {
            leadingFrames: 0,
            trailingFrames: 0,
        };
        encoder.stream.priming = encoder
            .get(kAudioConverterPrimeInfo, prime, "prime info")?
            .leadingFrames;
        Ok(encoder)
    }

    /// Encodes as many whole packets as the samples so far allow.
    pub fn push(&mut self, samples: &[f32]) -> Result<(), AudioError> {
        self.input.samples.drain(..self.input.consumed);
        self.input.consumed = 0;
        self.input.samples.extend_from_slice(samples);
        self.convert()
    }

    /// The packets encoded since the last call, with the stream's format, so a muxer can write
    /// them while encoding goes on.
    pub fn take(&mut self) -> AacStream {
        AacStream {
            data: std::mem::take(&mut self.stream.data),
            sizes: std::mem::take(&mut self.stream.sizes),
            config: self.stream.config.clone(),
            ..self.stream
        }
    }

    /// Flushes the encoder and returns every packet not taken yet.
    pub fn finish(mut self) -> Result<AacStream, AudioError> {
        self.input.end_of_stream = true;
        self.convert()?;
        Ok(std::mem::take(&mut self.stream))
    }

    fn convert(&mut self) -> Result<(), AudioError> {
        const PACKETS: usize = 32;
        let mut data = vec![0u8; PACKETS * self.max_packet_size as usize];
        let mut descriptions = [AudioStreamPacketDescription {
            mStartOffset: 0,
            mVariableFramesInPacket: 0,
            mDataByteSize: 0,
        }; PACKETS];
        loop {
            let mut buffers = AudioBufferList {
                mNumberBuffers: 1,
                mBuffers: [objc2_core_audio_types::AudioBuffer {
                    mNumberChannels: 1,
                    mDataByteSize: data.len() as u32,
                    mData: data.as_mut_ptr().cast(),
                }],
            };
            let mut packets = PACKETS as u32;
            // SAFETY: The callback only reads `self.input`, which stays in place for the call, and
            // the output buffers hold `PACKETS` packets of the maximum size.
            let status = unsafe {
                AudioConverterFillComplexBuffer(
                    self.converter,
                    Some(provide_input),
                    (&raw mut *self.input).cast(),
                    NonNull::from(&mut packets),
                    NonNull::from(&mut buffers),
                    descriptions.as_mut_ptr(),
                )
            };
            if status != 0 && status != NEEDS_INPUT {
                return Err(AudioError::Converter("encoding", status));
            }
            for description in &descriptions[..packets as usize] {
                let start = description.mStartOffset as usize;
                let size = description.mDataByteSize as usize;
                self.stream
                    .data
                    .extend_from_slice(&data[start..start + size]);
                self.stream.sizes.push(size as u32);
            }
            if packets == 0 || status == NEEDS_INPUT {
                return Ok(());
            }
        }
    }

    fn set(&self, property: u32, value: u32, name: &'static str) -> Result<(), AudioError> {
        // SAFETY: The property takes a UInt32.
        let status = unsafe {
            AudioConverterSetProperty(
                self.converter,
                property,
                size_of::<u32>() as u32,
                NonNull::from(&value).cast(),
            )
        };
        match status {
            0 => Ok(()),
            status => Err(AudioError::Converter(name, status)),
        }
    }

    fn get<T>(&self, property: u32, mut value: T, name: &'static str) -> Result<T, AudioError> {
        let mut size = size_of::<T>() as u32;
        // SAFETY: Callers pass the type the property is documented to return.
        let status = unsafe {
            AudioConverterGetProperty(
                self.converter,
                property,
                NonNull::from(&mut size),
                NonNull::from(&mut value).cast(),
            )
        };
        match status {
            0 => Ok(value),
            status => Err(AudioError::Converter(name, status)),
        }
    }
}

impl Drop for AacEncoder {
    fn drop(&mut self) {
        unsafe { AudioConverterDispose(self.converter) };
    }
}

unsafe extern "C-unwind" fn provide_input(
    _converter: AudioConverterRef,
    packets: NonNull<u32>,
    buffers: NonNull<AudioBufferList>,
    _descriptions: *mut *mut AudioStreamPacketDescription,
    user_data: *mut c_void,
) -> i32 {
    // SAFETY: `user_data` is the encoder's `Input`, alive and not otherwise borrowed during the call.
    let input = unsafe { &mut *user_data.cast::<Input>() };
    let available = input.samples.len() - input.consumed;
    let count = available.min(unsafe { *packets.as_ptr() } as usize);
    // SAFETY: The converter passes a buffer list with one buffer for mono input.
    let buffer = unsafe { &mut (*buffers.as_ptr()).mBuffers[0] };
    unsafe { *packets.as_ptr() = count as u32 };
    if count == 0 {
        buffer.mData = ptr::null_mut();
        buffer.mDataByteSize = 0;
        return if input.end_of_stream { 0 } else { NEEDS_INPUT };
    }
    buffer.mData = input.samples[input.consumed..].as_mut_ptr().cast();
    buffer.mDataByteSize = (count * 4) as u32;
    buffer.mNumberChannels = 1;
    input.consumed += count;
    0
}

/// AAC-LC, mono, at the sample rate's frequency index.
fn audio_specific_config(sample_rate: u32) -> Vec<u8> {
    const RATES: [u32; 13] = [
        96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025,
        8_000, 7_350,
    ];
    let index = RATES
        .iter()
        .position(|&rate| rate == sample_rate)
        .expect("AAC supports the export sample rate") as u8;
    let object_type = 2u8;
    let channels = 1u8;
    vec![
        (object_type << 3) | (index >> 1),
        ((index & 1) << 7) | (channels << 3),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_48k_aac_lc_config() {
        assert_eq!(audio_specific_config(48_000), [0x11, 0x88]);
    }

    #[test]
    fn encoding_keeps_every_sample_after_the_priming() {
        let mut encoder = AacEncoder::new(SAMPLE_RATE, 128_000).unwrap();
        let tone: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5)
            .collect();
        for chunk in tone.chunks(777) {
            encoder.push(chunk).unwrap();
        }
        let stream = encoder.finish().unwrap();
        let covered = stream.sizes.len() as u32 * AAC_PACKET_SAMPLES;
        assert!(stream.priming > 0);
        assert!(covered >= 48_000 + stream.priming);
        assert!(covered < 48_000 + stream.priming + 2 * AAC_PACKET_SAMPLES);
        assert_eq!(
            stream.data.len(),
            stream.sizes.iter().map(|&s| s as usize).sum::<usize>()
        );
    }
}
