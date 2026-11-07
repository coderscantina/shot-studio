//! `shot record`: one take into a video folder, until Ctrl-C or a duration.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use shot_capture::{Devices, Session, SessionConfig, Source};

#[derive(clap::Args)]
pub struct RecordArgs {
    /// The video folder. The take goes into its `media/<take>/`.
    #[arg(long, required_unless_present = "list_devices")]
    video: Option<PathBuf>,
    /// Display id from `--list-devices`. Defaults to the main display.
    #[arg(long)]
    display: Option<u32>,
    /// Camera name or unique id. Defaults to the system camera, if there is one.
    #[arg(long, conflicts_with = "no_camera")]
    camera: Option<String>,
    #[arg(long)]
    no_camera: bool,
    /// Mic name or unique id. Defaults to the system mic.
    #[arg(long, conflicts_with = "no_mic")]
    mic: Option<String>,
    #[arg(long)]
    no_mic: bool,
    #[arg(long)]
    no_system_audio: bool,
    /// Stop after this many seconds instead of on Ctrl-C.
    #[arg(long)]
    duration: Option<f64>,
    /// List displays, cameras and mics, and exit.
    #[arg(long)]
    list_devices: bool,
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

pub fn run(args: RecordArgs) -> Result<(), String> {
    if args.list_devices {
        print!("{}", devices_text(&shot_capture::list_devices()));
        return Ok(());
    }
    let mut config = SessionConfig::new(args.video.expect("clap requires --video"));
    config.display = args.display;
    config.camera = match (args.no_camera, args.camera) {
        (true, _) => Source::Off,
        (false, Some(name)) => Source::Named(name),
        (false, None) => Source::Default,
    };
    config.mic = match (args.no_mic, args.mic) {
        (true, _) => Source::Off,
        (false, Some(name)) => Source::Named(name),
        (false, None) => Source::Default,
    };
    config.system_audio = !args.no_system_audio;
    let limit = args
        .duration
        .map(|secs| Duration::try_from_secs_f64(secs).map_err(|e| format!("--duration: {e}")))
        .transpose()?;

    // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
    unsafe {
        libc::signal(
            libc::SIGINT,
            on_interrupt as *const () as libc::sighandler_t,
        )
    };
    let session = Session::start(config).map_err(|e| e.to_string())?;
    eprintln!(
        "recording {} ({}), {}",
        session.id(),
        session.files().join(", "),
        match limit {
            Some(limit) => format!("{:.1} s", limit.as_secs_f64()),
            None => "Ctrl-C stops".into(),
        }
    );
    let started = Instant::now();
    while !INTERRUPTED.load(Ordering::Relaxed) && limit.is_none_or(|l| started.elapsed() < l) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let take = session.stop().map_err(|e| e.to_string())?;

    println!("{}", take.id);
    let shown = |file: &str| take.dir.join(file).display().to_string();
    for track in &take.tracks {
        let dropped = match track.dropped {
            0 => String::new(),
            n => format!(", {n} dropped"),
        };
        println!("{}  {} samples{dropped}", shown(track.file), track.samples);
    }
    println!("{}  {} events", shown("events.jsonl"), take.events);
    eprintln!("{:.1} s", take.duration.0 as f64 / 1e6);
    Ok(())
}

fn devices_text(devices: &Devices) -> String {
    let mut out = String::from("displays\n");
    for d in &devices.displays {
        let main = if d.main { ", main" } else { "" };
        out += &format!(
            "  {}  {}x{} at {} Hz{main}\n",
            d.id, d.width, d.height, d.refresh_hz
        );
    }
    for (title, list) in [("cameras", &devices.cameras), ("mics", &devices.mics)] {
        out += &format!("{title}\n");
        if list.is_empty() {
            out += "  none\n";
        }
        for d in list {
            let default = if d.default { ", default" } else { "" };
            out += &format!("  {}  ({}{default})\n", d.name, d.unique_id);
        }
    }
    out
}
