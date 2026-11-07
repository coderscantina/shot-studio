//! Process CPU time and memory, as Activity Monitor reports them.

use std::time::Duration;

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
