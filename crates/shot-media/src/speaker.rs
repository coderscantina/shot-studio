//! Audio output: mono [`SAMPLE_RATE`] samples played on the default output device through
//! CoreAudio's default output unit, with a clock that says which sample is audible now.
//!
//! One thread owns the [`Speaker`] and pushes samples into a lock-free ring; the device's render
//! callback takes them. Each callback stamps the host time its first sample reaches the device,
//! so [`SpeakerClock`] can tell any thread which sample plays at a host time, with the device's
//! output latency added.

use std::ffi::c_void;
use std::mem;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use objc2_audio_toolbox::{
    AURenderCallbackStruct, AudioComponentDescription, AudioComponentFindNext,
    AudioComponentInstanceDispose, AudioComponentInstanceNew, AudioOutputUnitStart,
    AudioOutputUnitStop, AudioUnit, AudioUnitGetProperty, AudioUnitInitialize,
    AudioUnitRenderActionFlags, AudioUnitSetProperty, AudioUnitUninitialize,
    kAudioOutputUnitProperty_CurrentDevice, kAudioUnitManufacturer_Apple,
    kAudioUnitProperty_Latency, kAudioUnitProperty_SetRenderCallback,
    kAudioUnitProperty_StreamFormat, kAudioUnitScope_Global, kAudioUnitScope_Input,
    kAudioUnitSubType_DefaultOutput, kAudioUnitType_Output,
};
use objc2_core_audio::{
    AudioConvertHostTimeToNanos, AudioGetCurrentHostTime, AudioObjectGetPropertyData,
    AudioObjectID, AudioObjectPropertyAddress, kAudioDevicePropertyLatency,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertySafetyOffset,
    kAudioDevicePropertyStreams, kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeOutput, kAudioStreamPropertyLatency,
};
use objc2_core_audio_types::{
    AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp, AudioTimeStampFlags,
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked, kAudioFormatLinearPCM,
};

use crate::audio::SAMPLE_RATE;

/// Samples the ring holds, one second.
const CAPACITY: usize = SAMPLE_RATE as usize;
/// The device gets the mono signal on both channels: a mono stream would play on the left only.
const CHANNELS: u32 = 2;

#[derive(Debug, thiserror::Error)]
pub enum SpeakerError {
    #[error("there is no default audio output")]
    NoOutput,
    #[error("audio output: {0} failed with OSStatus {1}")]
    Unit(&'static str, i32),
}

/// Plays pushed samples on the default output device. Stopped until [`play`](Self::play).
pub struct Speaker {
    unit: AudioUnit,
    shared: Arc<Shared>,
    playing: bool,
}

/// Which pushed sample is audible, readable from any thread.
#[derive(Clone)]
pub struct SpeakerClock(Arc<Shared>);

struct Shared {
    ring: Ring,
    /// The last callbacks' buffers, empty until the first one after a [`Speaker::clear`].
    stamps: Mutex<Stamps>,
    /// From the host time a sample reaches the device to the time it is heard, in nanoseconds.
    latency: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct Stamp {
    /// Samples taken from the ring before this buffer.
    first: u64,
    /// Samples of the ring in this buffer; the rest of it was silence.
    count: u64,
    /// Host time, in nanoseconds, when the buffer's first sample reaches the device.
    at: u64,
}

/// Buffers rendered recently. The device renders ahead of what it plays, so the buffer heard
/// now is usually a few callbacks old.
#[derive(Default)]
struct Stamps {
    items: [Stamp; Stamps::KEPT],
    len: usize,
    next: usize,
}

impl Stamps {
    const KEPT: usize = 16;

    fn record(&mut self, stamp: Stamp) {
        self.items[self.next] = stamp;
        self.next = (self.next + 1) % Self::KEPT;
        self.len = (self.len + 1).min(Self::KEPT);
    }

    /// The sample heard at host time `heard` in nanoseconds: inside the newest buffer that
    /// started by then, held at its last sample from the ring once that ran out. Before every
    /// kept buffer, the first one's first sample.
    fn sample_at(&self, heard: i128) -> Option<u64> {
        let newest_first =
            (1..=self.len).map(|age| self.items[(self.next + Self::KEPT - age) % Self::KEPT]);
        let mut oldest = None;
        for stamp in newest_first {
            if i128::from(stamp.at) <= heard {
                let offset =
                    (heard - i128::from(stamp.at)) * i128::from(SAMPLE_RATE) / 1_000_000_000;
                return Some(stamp.first + (offset as u64).min(stamp.count));
            }
            oldest = Some(stamp.first);
        }
        oldest
    }
}

impl Speaker {
    /// Opens the default output device, stopped and empty.
    pub fn open() -> Result<Self, SpeakerError> {
        let description = AudioComponentDescription {
            componentType: kAudioUnitType_Output,
            componentSubType: kAudioUnitSubType_DefaultOutput,
            componentManufacturer: kAudioUnitManufacturer_Apple,
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        // SAFETY: The description is a valid pointer for the call.
        let component =
            unsafe { AudioComponentFindNext(ptr::null_mut(), NonNull::from(&description)) };
        if component.is_null() {
            return Err(SpeakerError::NoOutput);
        }
        let mut unit: AudioUnit = ptr::null_mut();
        // SAFETY: The component exists and the out pointer is writable.
        check("AudioComponentInstanceNew", unsafe {
            AudioComponentInstanceNew(component, NonNull::from(&mut unit))
        })?;
        // Owns the unit from here, so a failure below disposes it.
        let mut speaker = Self {
            unit,
            shared: Arc::new(Shared {
                ring: Ring::new(CAPACITY),
                stamps: Mutex::default(),
                latency: 0,
            }),
            playing: false,
        };
        let format = AudioStreamBasicDescription {
            mSampleRate: f64::from(SAMPLE_RATE),
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
            mBytesPerPacket: 4 * CHANNELS,
            mFramesPerPacket: 1,
            mBytesPerFrame: 4 * CHANNELS,
            mChannelsPerFrame: CHANNELS,
            mBitsPerChannel: 32,
            mReserved: 0,
        };
        speaker.set("StreamFormat", kAudioUnitProperty_StreamFormat, &format)?;
        // SAFETY: The unit is live; the value is read back with its own size.
        check("AudioUnitInitialize", unsafe {
            AudioUnitInitialize(speaker.unit)
        })?;
        let latency = speaker.latency_nanos()?;
        Arc::get_mut(&mut speaker.shared)
            .expect("no clock handed out yet")
            .latency = latency;
        let callback = AURenderCallbackStruct {
            inputProc: Some(render),
            inputProcRefCon: Arc::as_ptr(&speaker.shared) as *mut c_void,
        };
        speaker.set(
            "SetRenderCallback",
            kAudioUnitProperty_SetRenderCallback,
            &callback,
        )?;
        Ok(speaker)
    }

    pub fn clock(&self) -> SpeakerClock {
        SpeakerClock(self.shared.clone())
    }

    /// From the time a sample reaches the device to the time it is heard.
    pub fn latency(&self) -> Duration {
        Duration::from_nanos(self.shared.latency)
    }

    /// Samples queued and not yet taken by the device.
    pub fn queued(&self) -> usize {
        CAPACITY - self.shared.ring.space()
    }

    /// Queues as many of `samples` as fit and returns how many did.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        self.shared.ring.push(samples)
    }

    pub fn play(&mut self) -> Result<(), SpeakerError> {
        if !self.playing {
            // SAFETY: The unit is initialized.
            check("AudioOutputUnitStart", unsafe {
                AudioOutputUnitStart(self.unit)
            })?;
            self.playing = true;
        }
        Ok(())
    }

    /// Stops the device. When this returns the render callback no longer runs.
    pub fn stop(&mut self) {
        if self.playing {
            // SAFETY: The unit is initialized. Stop waits for a running render to end.
            unsafe { AudioOutputUnitStop(self.unit) };
            self.playing = false;
        }
    }

    /// Stops and drops every queued sample. The clock counts from zero again and reports
    /// nothing until the device plays.
    pub fn clear(&mut self) {
        self.stop();
        self.shared.ring.clear();
        *self.shared.stamps.lock().expect("speaker clock poisoned") = Stamps::default();
    }

    /// Sets a property on the input side of the unit, where our samples come in.
    fn set<T>(&self, name: &'static str, property: u32, value: &T) -> Result<(), SpeakerError> {
        // SAFETY: `value` is the property's type and lives for the call.
        check(name, unsafe {
            AudioUnitSetProperty(
                self.unit,
                property,
                kAudioUnitScope_Input,
                0,
                (value as *const T).cast(),
                mem::size_of::<T>() as u32,
            )
        })
    }

    /// Unit, device, safety offset and stream latency of the current output device.
    fn latency_nanos(&self) -> Result<u64, SpeakerError> {
        let mut unit_latency = 0.0_f64;
        let mut size = mem::size_of::<f64>() as u32;
        // SAFETY: The out pointers are writable and sized for an f64.
        check("AudioUnitGetProperty(Latency)", unsafe {
            AudioUnitGetProperty(
                self.unit,
                kAudioUnitProperty_Latency,
                kAudioUnitScope_Global,
                0,
                NonNull::from(&mut unit_latency).cast(),
                NonNull::from(&mut size),
            )
        })?;
        let mut device: AudioObjectID = 0;
        let mut size = mem::size_of::<AudioObjectID>() as u32;
        // SAFETY: As above, sized for a device id.
        check("AudioUnitGetProperty(CurrentDevice)", unsafe {
            AudioUnitGetProperty(
                self.unit,
                kAudioOutputUnitProperty_CurrentDevice,
                kAudioUnitScope_Global,
                0,
                NonNull::from(&mut device).cast(),
                NonNull::from(&mut size),
            )
        })?;
        let rate: f64 = property(device, kAudioDevicePropertyNominalSampleRate, false)?;
        let device_frames: u32 = property(device, kAudioDevicePropertyLatency, true)?;
        let safety_frames: u32 = property(device, kAudioDevicePropertySafetyOffset, true)?;
        // The first output stream's own latency; a device without one reports none.
        let stream_frames: u32 =
            property::<AudioObjectID>(device, kAudioDevicePropertyStreams, true)
                .and_then(|stream| property(stream, kAudioStreamPropertyLatency, false))
                .unwrap_or(0);
        let frames = f64::from(device_frames + safety_frames + stream_frames);
        let seconds = unit_latency + if rate > 0.0 { frames / rate } else { 0.0 };
        Ok((seconds * 1e9) as u64)
    }
}

impl Drop for Speaker {
    fn drop(&mut self) {
        // SAFETY: The unit is live; after these calls no callback refers to `shared` again.
        unsafe {
            AudioOutputUnitStop(self.unit);
            AudioUnitUninitialize(self.unit);
            AudioComponentInstanceDispose(self.unit);
        }
    }
}

impl SpeakerClock {
    /// The pushed sample heard `ahead` from now, counted from the last [`Speaker::clear`].
    /// `None` until the device has played since then. While the ring runs dry the clock holds
    /// at the last sample heard.
    pub fn audible(&self, ahead: Duration) -> Option<u64> {
        // SAFETY: Plain host clock reads.
        let now = unsafe { AudioConvertHostTimeToNanos(AudioGetCurrentHostTime()) };
        let heard = i128::from(now) + ahead.as_nanos() as i128 - i128::from(self.0.latency);
        self.0
            .stamps
            .lock()
            .expect("speaker clock poisoned")
            .sample_at(heard)
    }
}

/// The device's render callback: fills the buffer from the ring and stamps its time.
unsafe extern "C-unwind" fn render(
    refcon: NonNull<c_void>,
    _flags: NonNull<AudioUnitRenderActionFlags>,
    timestamp: NonNull<AudioTimeStamp>,
    _bus: u32,
    frames: u32,
    data: *mut AudioBufferList,
) -> i32 {
    // SAFETY: The refcon is the `Shared` the speaker keeps alive while the unit exists, and
    // CoreAudio passes a valid time stamp and buffer list for `frames` stereo float frames.
    let (shared, timestamp, list) = unsafe {
        (
            refcon.cast::<Shared>().as_ref(),
            timestamp.as_ref(),
            &mut *data,
        )
    };
    let buffer = list.mBuffers[0];
    let length = (buffer.mDataByteSize as usize / mem::size_of::<f32>()).min(frames as usize * 2);
    // SAFETY: The buffer holds `mDataByteSize` bytes of interleaved floats.
    let out = unsafe { std::slice::from_raw_parts_mut(buffer.mData.cast::<f32>(), length) };
    let first = shared.ring.taken();
    let count = shared.ring.pop_stereo(out);
    if timestamp
        .mFlags
        .contains(AudioTimeStampFlags::HostTimeValid)
        && let Ok(mut stamps) = shared.stamps.try_lock()
    {
        // SAFETY: Plain host clock conversion.
        let at = unsafe { AudioConvertHostTimeToNanos(timestamp.mHostTime) };
        stamps.record(Stamp {
            first: first as u64,
            count: count as u64,
            at,
        });
    }
    0
}

fn check(name: &'static str, status: i32) -> Result<(), SpeakerError> {
    if status == 0 {
        Ok(())
    } else {
        Err(SpeakerError::Unit(name, status))
    }
}

/// A fixed-size property of a CoreAudio object, on the output scope or the global one.
fn property<T: Default>(
    object: AudioObjectID,
    selector: u32,
    output: bool,
) -> Result<T, SpeakerError> {
    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: if output {
            kAudioObjectPropertyScopeOutput
        } else {
            kAudioObjectPropertyScopeGlobal
        },
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut value = T::default();
    let mut size = mem::size_of::<T>() as u32;
    // SAFETY: The out pointer is writable for `size` bytes; array properties fill the first one.
    check("AudioObjectGetPropertyData", unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    })?;
    Ok(value)
}

/// A single-producer, single-consumer ring of samples. The indices only grow, so their
/// difference is the fill level and no slot is ever both read and written.
struct Ring {
    samples: Box<[AtomicU32]>,
    /// Samples taken by the consumer.
    read: AtomicUsize,
    /// Samples written by the producer.
    write: AtomicUsize,
}

impl Ring {
    fn new(capacity: usize) -> Self {
        Self {
            samples: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
        }
    }

    fn space(&self) -> usize {
        let filled = self.write.load(Ordering::Relaxed) - self.read.load(Ordering::Acquire);
        self.samples.len() - filled
    }

    fn taken(&self) -> usize {
        self.read.load(Ordering::Relaxed)
    }

    /// Producer side.
    fn push(&self, samples: &[f32]) -> usize {
        let write = self.write.load(Ordering::Relaxed);
        let count = samples.len().min(self.space());
        for (i, sample) in samples[..count].iter().enumerate() {
            self.samples[(write + i) % self.samples.len()]
                .store(sample.to_bits(), Ordering::Relaxed);
        }
        self.write.store(write + count, Ordering::Release);
        count
    }

    /// Consumer side: fills interleaved stereo `out` with queued samples, then silence.
    /// Returns the samples taken.
    fn pop_stereo(&self, out: &mut [f32]) -> usize {
        let read = self.read.load(Ordering::Relaxed);
        let queued = self.write.load(Ordering::Acquire) - read;
        let count = queued.min(out.len() / 2);
        for (i, frame) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let sample = if i < count {
                f32::from_bits(
                    self.samples[(read + i) % self.samples.len()].load(Ordering::Relaxed),
                )
            } else {
                0.0
            };
            frame.fill(sample);
        }
        self.read.store(read + count, Ordering::Release);
        count
    }

    /// Only while no consumer runs.
    fn clear(&self) {
        self.read.store(0, Ordering::Relaxed);
        self.write.store(0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_hands_out_samples_in_order_across_its_end_then_silence() {
        let ring = Ring::new(4);
        assert_eq!(ring.push(&[1.0, 2.0, 3.0]), 3);
        let mut out = [9.0; 4];
        assert_eq!(ring.pop_stereo(&mut out), 2);
        assert_eq!(out, [1.0, 1.0, 2.0, 2.0]);
        assert_eq!(ring.push(&[4.0, 5.0, 6.0, 7.0]), 3);
        assert_eq!(ring.space(), 0);
        let mut out = [9.0; 10];
        assert_eq!(ring.pop_stereo(&mut out), 4);
        assert_eq!(out, [3.0, 3.0, 4.0, 4.0, 5.0, 5.0, 6.0, 6.0, 0.0, 0.0]);
        assert_eq!(ring.taken(), 6);
    }

    #[test]
    fn the_clock_reads_the_buffer_playing_at_a_time_and_holds_when_the_ring_runs_dry() {
        let ms = |v: u64| v * 1_000_000;
        let mut stamps = Stamps::default();
        assert_eq!(stamps.sample_at(0), None);
        // 10 ms buffers of 480 samples rendered ahead; the third one only had 100 queued.
        for (index, count) in [480, 480, 100, 0].into_iter().enumerate() {
            let index = index as u64;
            stamps.record(Stamp {
                first: (index * 480).min(1060),
                count,
                at: ms(100 + index * 10),
            });
        }
        assert_eq!(stamps.sample_at(i128::from(ms(90))), Some(0));
        assert_eq!(stamps.sample_at(i128::from(ms(105))), Some(240));
        assert_eq!(stamps.sample_at(i128::from(ms(115))), Some(720));
        assert_eq!(stamps.sample_at(i128::from(ms(129))), Some(1060));
        assert_eq!(stamps.sample_at(i128::from(ms(500))), Some(1060));
    }
}
