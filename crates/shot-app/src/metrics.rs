//! Process CPU time and memory, as Activity Monitor reports them, and main thread stall samples.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// User plus system CPU time of this process.
pub fn cpu_time() -> Duration {
    // SAFETY: getrusage fills the zeroed struct it is given.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        usage
    };
    let to_duration = |time: libc::timeval| {
        Duration::from_secs(time.tv_sec as u64) + Duration::from_micros(time.tv_usec as u64)
    };
    to_duration(usage.ru_utime) + to_duration(usage.ru_stime)
}

/// Physical footprint (Activity Monitor's "Memory") and resident size, in bytes.
pub fn memory() -> (u64, u64) {
    // SAFETY: proc_pid_rusage fills the zeroed V2 struct for our own pid.
    unsafe {
        let mut info: libc::rusage_info_v2 = std::mem::zeroed();
        libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V2,
            (&raw mut info).cast::<libc::rusage_info_t>(),
        );
        (info.ri_phys_footprint, info.ri_resident_size)
    }
}

/// Total CPU time of all `VTDecoderXPCService` processes. Hardware decode runs there, not in the app.
/// Uses only syscalls, so it is cheap enough to call from a render.
#[allow(
    deprecated,
    reason = "libc points mach_timebase_info to the mach2 crate; one call does not justify a dependency"
)]
pub fn decoder_service_cpu_time() -> Duration {
    let mut pids = vec![0 as libc::pid_t; 4096];
    let capacity = (pids.len() * size_of::<libc::pid_t>()) as libc::c_int;
    // SAFETY: The buffer holds `capacity` bytes and the call writes at most that many.
    let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), capacity) };
    pids.truncate(count.max(0) as usize);

    let mut timebase = libc::mach_timebase_info { numer: 0, denom: 0 };
    // SAFETY: Fills the struct it is given.
    unsafe { libc::mach_timebase_info(&mut timebase) };
    let mut ticks: u64 = 0;
    for pid in pids {
        let mut name = [0u8; 64];
        // SAFETY: The buffer is 64 bytes and proc_name writes a NUL-terminated name into it.
        let length = unsafe { libc::proc_name(pid, name.as_mut_ptr().cast(), name.len() as u32) };
        if &name[..length.max(0) as usize] != b"VTDecoderXPCService" {
            continue;
        }
        // SAFETY: proc_pid_rusage fills the zeroed V2 struct.
        let info = unsafe {
            let mut info: libc::rusage_info_v2 = std::mem::zeroed();
            libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, (&raw mut info).cast());
            info
        };
        ticks += info.ri_user_time + info.ri_system_time;
    }
    Duration::from_nanos(ticks * timebase.numer as u64 / timebase.denom.max(1) as u64)
}

/// User plus system CPU time of the calling thread. A few microseconds, cheap enough per render.
#[allow(
    deprecated,
    reason = "libc points the mach thread calls to the mach2 crate; two calls do not justify a dependency"
)]
pub fn thread_cpu_time() -> Duration {
    // SAFETY: pthread_mach_thread_np borrows the port of our own thread without a new reference,
    // and thread_info fills at most `count` words of the zeroed struct.
    let info = unsafe {
        let mut info: libc::thread_basic_info = std::mem::zeroed();
        let mut count = libc::THREAD_BASIC_INFO_COUNT;
        libc::thread_info(
            libc::pthread_mach_thread_np(libc::pthread_self()),
            libc::THREAD_BASIC_INFO as libc::thread_flavor_t,
            (&raw mut info).cast(),
            &mut count,
        );
        info
    };
    let to_duration = |time: libc::time_value_t| {
        Duration::from_secs(time.seconds.max(0) as u64)
            + Duration::from_micros(time.microseconds.max(0) as u64)
    };
    to_duration(info.user_time) + to_duration(info.system_time)
}

/// The one minute load average of the machine.
pub fn load_average() -> f64 {
    let mut load = [0.0];
    // SAFETY: Writes at most one value into the one-element buffer.
    unsafe { libc::getloadavg(load.as_mut_ptr(), 1) };
    load[0]
}

/// A watchdog thread that samples the main thread while it has not rendered for over 18 ms,
/// a little more than one 60 Hz refresh. Each sample holds the thread's scheduler state and its
/// innermost functions, read by suspending it for a few microseconds.
pub struct StallWatch {
    shared: Arc<StallShared>,
}

struct StallShared {
    origin: Instant,
    /// Nanoseconds from `origin` to the last render.
    last_render: AtomicU64,
    /// Symbolized only when read: `dladdr` scans the binary's symbols, which in a large binary
    /// takes longer than the sampling interval.
    samples: Mutex<Vec<RawSample>>,
}

/// A sample with its return addresses still unnamed.
struct RawSample {
    waiting: bool,
    addresses: Vec<u64>,
}

/// The main thread at one moment of a stall.
pub struct StallSample {
    /// Blocked in the kernel, as opposed to running or waiting for a core.
    pub waiting: bool,
    /// Innermost function first.
    pub frames: Vec<String>,
}

impl StallSample {
    /// Waiting for events in the run loop, which is where an idle main thread sits.
    pub fn is_idle(&self) -> bool {
        self.waiting
            && self
                .frames
                .iter()
                .any(|frame| frame.contains("CFRunLoopServiceMachPort"))
    }
}

const STALL: Duration = Duration::from_millis(18);
const STACK_DEPTH: usize = 16;

impl StallWatch {
    /// Starts watching the calling thread, which must be the main thread.
    pub fn start() -> Self {
        // SAFETY: Reads the identity and stack bounds of the calling thread, which lives as long
        // as the process.
        let (port, stack) = unsafe {
            let thread = libc::pthread_self();
            let top = libc::pthread_get_stackaddr_np(thread) as usize;
            let size = libc::pthread_get_stacksize_np(thread);
            (libc::pthread_mach_thread_np(thread), top - size..top)
        };
        let shared = Arc::new(StallShared {
            origin: Instant::now(),
            last_render: AtomicU64::new(0),
            samples: Mutex::new(Vec::new()),
        });
        std::thread::Builder::new()
            .name("shot-stall-watch".into())
            .spawn({
                let shared = shared.clone();
                move || {
                    // At a lower class, timer coalescing would let the 1 ms sleep run long.
                    set_user_interactive();
                    loop {
                        std::thread::sleep(Duration::from_millis(1));
                        let last = Duration::from_nanos(shared.last_render.load(Ordering::Relaxed));
                        if shared.origin.elapsed().saturating_sub(last) > STALL {
                            let sample = sample_thread(port, stack.clone());
                            shared
                                .samples
                                .lock()
                                .expect("stall samples poisoned")
                                .push(sample);
                        }
                    }
                }
            })
            .expect("spawning the stall watchdog");
        Self { shared }
    }

    /// Marks a render and returns the samples taken since the last one.
    pub fn rendered(&self) -> Vec<StallSample> {
        let now = self.shared.origin.elapsed().as_nanos() as u64;
        self.shared.last_render.store(now, Ordering::Relaxed);
        let raw = std::mem::take(&mut *self.shared.samples.lock().expect("stall samples poisoned"));
        raw.into_iter()
            .map(|sample| {
                let mut frames: Vec<String> = sample.addresses.into_iter().map(symbol).collect();
                frames.dedup();
                StallSample {
                    waiting: sample.waiting,
                    frames,
                }
            })
            .collect()
    }
}

/// Runs the calling thread at the QoS of the main thread.
fn set_user_interactive() {
    // SAFETY: Changes only the calling thread's scheduling class.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0)
    };
}

unsafe extern "C" {
    fn thread_suspend(thread: libc::mach_port_t) -> libc::kern_return_t;
    fn thread_resume(thread: libc::mach_port_t) -> libc::kern_return_t;
    fn thread_get_state(
        thread: libc::mach_port_t,
        flavor: libc::c_int,
        state: *mut u32,
        count: *mut u32,
    ) -> libc::kern_return_t;
}

/// `ARM_THREAD_STATE64` from `<mach/arm/thread_status.h>`: x0 to x28, fp, lr, sp, pc, cpsr, pad.
#[repr(C)]
#[derive(Default)]
struct ArmThreadState {
    x: [u64; 29],
    fp: u64,
    lr: u64,
    sp: u64,
    pc: u64,
    cpsr: u32,
    pad: u32,
}

const ARM_THREAD_STATE64: libc::c_int = 6;

/// Return addresses signed by pointer authentication carry the signature above the 47 bit address.
const ADDRESS_MASK: u64 = (1 << 47) - 1;

#[allow(
    deprecated,
    reason = "libc points the mach thread calls to the mach2 crate; they do not justify a dependency"
)]
fn sample_thread(port: libc::mach_port_t, stack: std::ops::Range<usize>) -> RawSample {
    // SAFETY: thread_info fills at most `count` words of the zeroed struct.
    let waiting = unsafe {
        let mut info: libc::thread_basic_info = std::mem::zeroed();
        let mut count = libc::THREAD_BASIC_INFO_COUNT;
        libc::thread_info(
            port,
            libc::THREAD_BASIC_INFO as libc::thread_flavor_t,
            (&raw mut info).cast(),
            &mut count,
        );
        info.run_state == libc::TH_STATE_WAITING
    };
    // Nothing between suspend and resume may allocate: the suspended thread could hold the
    // allocator's lock.
    let mut addresses = [0u64; STACK_DEPTH];
    let mut depth = 0;
    // SAFETY: The thread is suspended while its registers and frame records are read, and every
    // frame pointer is checked to lie inside its stack before it is dereferenced.
    unsafe {
        if thread_suspend(port) != 0 {
            return RawSample {
                waiting,
                addresses: Vec::new(),
            };
        }
        let mut state = ArmThreadState::default();
        let mut count = (size_of::<ArmThreadState>() / 4) as u32;
        if thread_get_state(
            port,
            ARM_THREAD_STATE64,
            (&raw mut state).cast(),
            &mut count,
        ) == 0
        {
            addresses[0] = state.pc;
            addresses[1] = state.lr & ADDRESS_MASK;
            depth = 2;
            let mut fp = state.fp as usize;
            while depth < STACK_DEPTH && stack.contains(&fp) && fp.is_multiple_of(8) {
                let record = fp as *const [u64; 2];
                let [next, ret] = std::ptr::read_volatile(record);
                addresses[depth] = ret & ADDRESS_MASK;
                depth += 1;
                if next as usize <= fp {
                    break;
                }
                fp = next as usize;
            }
        }
        thread_resume(port);
    }
    RawSample {
        waiting,
        addresses: addresses[..depth]
            .iter()
            .copied()
            .filter(|&address| address != 0)
            .collect(),
    }
}

fn symbol(address: u64) -> String {
    // SAFETY: dladdr fills the zeroed struct and leaves names null when it finds none.
    let info = unsafe {
        let mut info: libc::Dl_info = std::mem::zeroed();
        libc::dladdr(
            (address.saturating_sub(1)) as *const libc::c_void,
            &mut info,
        );
        info
    };
    if info.dli_sname.is_null() {
        return format!("{address:#x}");
    }
    // SAFETY: dladdr returns a NUL-terminated name.
    let name = unsafe { std::ffi::CStr::from_ptr(info.dli_sname) }.to_string_lossy();
    demangle(&name)
}

/// Turns a legacy Rust symbol (`_ZN4gpui6window6Window4draw17h0123E`) into `gpui::window::Window::draw`.
/// Other names come back as they are.
fn demangle(name: &str) -> String {
    let Some(mut rest) = name.strip_prefix("_ZN") else {
        return name.to_owned();
    };
    let mut parts = Vec::new();
    while let Some(length_end) = rest.find(|c: char| !c.is_ascii_digit()) {
        let Ok(length) = rest[..length_end].parse::<usize>() else {
            break;
        };
        let Some(part) = rest.get(length_end..length_end + length) else {
            break;
        };
        parts.push(part);
        rest = &rest[length_end + length..];
    }
    if parts
        .last()
        .is_some_and(|part| part.starts_with('h') && part.len() == 17)
    {
        parts.pop();
    }
    if parts.is_empty() {
        return name.to_owned();
    }
    parts.join("::")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_watchdog_names_where_a_stalled_thread_waits_or_runs() {
        let watch = StallWatch::start();
        watch.rendered();
        std::thread::sleep(Duration::from_millis(60));
        let samples = watch.rendered();
        assert!(samples.len() >= 10, "{} samples", samples.len());
        assert!(
            samples
                .iter()
                .all(|sample| sample.waiting && !sample.is_idle())
        );
        assert!(
            samples[0]
                .frames
                .iter()
                .any(|frame| frame.contains("nanosleep")),
            "{:?}",
            samples[0].frames
        );

        let busy_until = Instant::now() + Duration::from_millis(60);
        while Instant::now() < busy_until {
            std::hint::spin_loop();
        }
        let samples = watch.rendered();
        assert!(samples.iter().filter(|sample| !sample.waiting).count() * 2 > samples.len());
    }

    #[test]
    fn legacy_rust_symbols_lose_their_mangling_and_hash() {
        assert_eq!(
            demangle("_ZN4gpui6window6Window4draw17h0123456789abcdefE"),
            "gpui::window::Window::draw"
        );
        assert_eq!(demangle("mach_msg2_trap"), "mach_msg2_trap");
    }
}
