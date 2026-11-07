//! The preview window: decoded IOSurfaces drawn by gpui's surface element, scrubbed with the mouse.
//!
//! Space plays and pauses, `z` toggles a 2x punch-in, dragging on the timeline scrubs. When the
//! mouse enters another keyframe interval while the decoder is still busy, the scrub shows that
//! keyframe first, which decodes in a few milliseconds, and the exact frame once the mouse rests. Each scrub prints how
//! many frames it showed and the longest wait between them. With `--bench` the window
//! plays and seeks by itself and prints the measurements.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, Context, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ObjectFit, TitlebarOptions, Window, WindowBounds, WindowOptions, div, prelude::*,
    px, relative, rgb, size, surface,
};
use shot_media::player::{Player, RING_FRAMES};
use shot_media::timing::FramePacer;
use shot_media::videotoolbox::Frame;

use crate::bench::{Bench, BenchOptions};
use crate::zoom::{ZoomControl, ZoomStage};

pub struct Options {
    pub video: PathBuf,
    pub zoom: f32,
    /// Lock frame phase to the display, see [`FramePacer`]. Off paces by wall clock alone.
    pub display_lock: bool,
    pub bench: Option<BenchOptions>,
}

pub fn open(options: Options, cx: &mut App) {
    let zoom = ZoomControl::new(options.zoom);
    let player = Player::spawn(options.video.clone(), {
        let zoom = zoom.clone();
        move || {
            let mut stage = ZoomStage::new(zoom);
            Some(Box::new(move |frame| stage.process(frame)))
        }
    })
    .unwrap_or_else(|error| panic!("opening {}: {error}", options.video.display()));

    let bench = options.bench.map(Bench::new);
    let bounds = Bounds::centered(None, size(px(1600.), px(1000.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Shot Studio preview spike".into()),
                ..Default::default()
            }),
            // A playing video needs every 60 Hz frame, also while another window has focus, in
            // low power mode or under thermal pressure. gpui's thermal cap of 16.667 ms defers a
            // render that comes a millisecond early, which costs a frame on a 60 Hz display.
            inactive_frame_interval: None,
            low_power_frame_interval: None,
            thermal_frame_interval: None,
            ..Default::default()
        },
        |window, cx| {
            cx.new(|cx| {
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                Preview::new(player, zoom, options.display_lock, focus, bench)
            })
        },
    )
    .expect("opening the preview window");
    cx.activate(true);
}

/// Playback counters, reset when a bench phase starts.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlaybackStats {
    /// Frames that reached the screen.
    pub presented: u64,
    /// Frames that were due but replaced by a newer one before they were shown.
    pub dropped: u64,
    /// Frames shown more than one frame duration after their time.
    pub late: u64,
    /// Renders that found no new frame although the current one was already over.
    pub underruns: u64,
}

pub struct Preview {
    pub player: Player,
    pub zoom: ZoomControl,
    pub pacer: FramePacer,
    origin: Instant,
    pub current: Option<Frame>,
    /// Whether the user wants playback. The clock starts once enough frames are buffered.
    pub playing: bool,
    /// When the seek still waiting for its frame was requested.
    pub pending_seek: Option<Instant>,
    /// The mouse drag on the timeline in progress.
    scrub: Option<Scrub>,
    /// Latency of the last finished seek and the frame time it showed.
    pub last_seek: Option<(Duration, Duration)>,
    pub stats: PlaybackStats,
    focus: FocusHandle,
    bench: Option<Bench>,
}

impl Preview {
    fn new(
        player: Player,
        zoom: ZoomControl,
        display_lock: bool,
        focus: FocusHandle,
        bench: Option<Bench>,
    ) -> Self {
        let frame_duration = player.info().frame_duration;
        // The bench presses play itself, once the window is up.
        let playing = bench.is_none();
        player.set_playing(playing);
        Self {
            pacer: FramePacer::new(frame_duration, frame_duration, display_lock),
            player,
            zoom,
            origin: Instant::now(),
            current: None,
            playing,
            pending_seek: None,
            scrub: None,
            last_seek: None,
            stats: PlaybackStats::default(),
            focus,
            bench,
        }
    }

    pub fn host_now(&self) -> Duration {
        self.origin.elapsed()
    }

    pub fn seek(&mut self, target: Duration) {
        if let Some(scrub) = &mut self.scrub {
            scrub.requested(target);
        }
        self.pending_seek = Some(Instant::now());
        self.pacer.clock.pause_at(target);
        self.player.seek(target);
    }

    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
        self.player.set_playing(playing);
        if !playing {
            let now = self.host_now();
            self.pacer.clock.pause_at(self.pacer.clock.media_time(now));
        }
    }

    /// Picks the frame for this render from the decode queue.
    fn advance(&mut self) {
        let now = self.host_now();
        if let Some(requested) = self.pending_seek {
            let Some((frame, latest)) = self.player.take_next() else {
                return;
            };
            if let Some(scrub) = &mut self.scrub {
                scrub.shown(latest);
            }
            if !latest {
                self.pacer.clock.pause_at(frame.pts);
                self.current = Some(frame);
                return;
            }
            self.last_seek = Some((requested.elapsed(), frame.pts));
            self.pending_seek = None;
            if let Some(scrub) = &self.scrub {
                let exact = self
                    .player
                    .info()
                    .index
                    .plan(scrub.target)
                    .map(|plan| plan.target);
                if exact != Some(frame.pts) {
                    // A keyframe stood in while the mouse moved; it rests now.
                    let target = scrub.target;
                    self.current = Some(frame);
                    self.seek(target);
                    return;
                }
                if scrub.released
                    && let Some(scrub) = self.scrub.take()
                {
                    eprintln!("{}", scrub.summary());
                }
            }
            if self.playing {
                self.pacer.clock.play_from(frame.pts, now);
            } else {
                self.pacer.clock.pause_at(frame.pts);
            }
            self.current = Some(frame);
            return;
        }
        if !self.playing {
            return;
        }
        if !self.pacer.clock.is_playing() {
            match self.player.front_pts() {
                Some(pts) if self.player.queued() >= RING_FRAMES / 2 => {
                    self.pacer.clock.play_from(pts, now);
                }
                _ => return,
            }
        }

        let due = self.pacer.due(now);
        let frame_duration = self.player.info().frame_duration;
        let (frame, skipped) = self.player.take_due(due);
        self.stats.dropped += skipped as u64;
        match frame {
            Some(frame) => {
                self.pacer.shown(now, frame.pts);
                self.stats.presented += 1;
                if due > frame.pts + frame_duration {
                    self.stats.late += 1;
                }
                self.current = Some(frame);
            }
            None => {
                let over = self
                    .current
                    .as_ref()
                    .is_some_and(|frame| due > frame.pts + frame_duration);
                if over {
                    self.stats.underruns += 1;
                }
            }
        }
    }

    fn status_line(&self) -> String {
        let shown = self
            .current
            .as_ref()
            .map(|frame| frame.pts)
            .unwrap_or_default();
        let clip = self.player.info().duration;
        let mut line = format!(
            "{:>7.3} s  {}  zoom {:.0}x  hardware decode {}",
            shown.as_secs_f64() % clip.as_secs_f64(),
            if self.pacer.clock.is_playing() {
                "playing"
            } else {
                "paused"
            },
            self.zoom.get(),
            if self.player.info().hardware_decode {
                "on"
            } else {
                "OFF"
            },
        );
        if let Some((latency, _)) = self.last_seek {
            line += &format!("  last seek {:.1} ms", latency.as_secs_f64() * 1000.0);
        }
        match &self.bench {
            Some(bench) => line += &format!("  bench: {}", bench.phase_label()),
            None => line += "  space play/pause, z zoom, drag the timeline to scrub",
        }
        line
    }

    fn scrub(&mut self, x: f32, window: &Window) {
        let width = f32::from(window.viewport_size().width).max(1.0);
        let fraction = (x / width).clamp(0.0, 1.0) as f64;
        let info = self.player.info();
        let Some(plan) = info.index.plan(info.duration.mul_f64(fraction)) else {
            return;
        };
        let shown_keyframe = self
            .current
            .as_ref()
            .and_then(|frame| info.index.plan(frame.pts))
            .map(|shown| shown.keyframe);
        let request = if self.player.seeking() && shown_keyframe != Some(plan.keyframe) {
            plan.keyframe
        } else {
            plan.target
        };
        self.set_playing(false);
        let scrub = self.scrub.get_or_insert_with(Scrub::new);
        scrub.moves += 1;
        scrub.target = plan.target;
        if scrub.requested != Some(request) {
            self.seek(request);
        }
    }

    fn end_scrub(&mut self) {
        match self.scrub.as_mut() {
            Some(scrub) if self.pending_seek.is_some() => scrub.released = true,
            Some(_) => {
                if let Some(scrub) = self.scrub.take() {
                    eprintln!("{}", scrub.summary());
                }
            }
            None => {}
        }
    }

    /// Where the shown frame sits in the clip, from 0 to 1.
    fn position(&self) -> f32 {
        let clip = self.player.info().duration.as_secs_f64();
        let shown = self
            .current
            .as_ref()
            .map_or(0.0, |frame| frame.pts.as_secs_f64());
        ((shown % clip) / clip) as f32
    }
}

/// What one mouse drag on the timeline showed, printed when its last seek lands.
struct Scrub {
    started: Instant,
    /// The frame under the mouse.
    target: Duration,
    /// The frame of the last seek.
    requested: Option<Duration>,
    moves: u32,
    frames: u32,
    /// Since when the screen lags behind a seek, while the mouse is down.
    behind_since: Option<Instant>,
    /// How long each lag lasted until a frame reached the screen.
    waits: Vec<Duration>,
    released: bool,
}

impl Scrub {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            target: Duration::ZERO,
            requested: None,
            moves: 0,
            frames: 0,
            behind_since: None,
            waits: Vec::new(),
            released: false,
        }
    }

    fn requested(&mut self, frame: Duration) {
        self.requested = Some(frame);
        if !self.released {
            self.behind_since.get_or_insert_with(Instant::now);
        }
    }

    /// Counts a frame put on screen. `latest` is false while a newer seek still decodes.
    fn shown(&mut self, latest: bool) {
        let now = Instant::now();
        if let Some(since) = self.behind_since.take() {
            self.waits.push(now - since);
            if !latest {
                self.behind_since = Some(now);
            }
        }
        self.frames += 1;
    }

    fn summary(&self) -> String {
        let seconds = self.started.elapsed().as_secs_f64();
        let mut waits = self.waits.clone();
        waits.sort_unstable();
        let wait = |p: f64| {
            let rank = ((waits.len() as f64 * p).ceil() as usize).clamp(1, waits.len().max(1));
            waits
                .get(rank - 1)
                .map_or(0.0, |wait| wait.as_secs_f64() * 1000.0)
        };
        format!(
            "scrub: {:.2} s, {} mouse moves, {} frames shown ({:.0} per second), screen behind the mouse p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
            seconds,
            self.moves,
            self.frames,
            self.frames as f64 / seconds,
            wait(0.5),
            wait(0.95),
            wait(1.0)
        )
    }
}

impl Render for Preview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(error) = self.player.error() {
            panic!("decoding failed: {error}");
        }
        let started = Instant::now();
        let frame_duration = self.player.info().frame_duration;
        let refresh = window.refresh_interval(cx).unwrap_or(frame_duration);
        self.pacer.set_refresh(frame_duration, refresh);
        self.advance();
        if let Some(mut bench) = self.bench.take() {
            if let Some(passed) = bench.step(self) {
                use std::io::Write;
                let _ = std::io::stdout().flush();
                std::process::exit(if passed { 0 } else { 1 });
            }
            self.bench = Some(bench);
        }
        window.request_animation_frame();

        let status = self.status_line();
        let position = self.position();
        if let Some(bench) = &mut self.bench {
            bench.rendered(started.elapsed());
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x000000))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, _| {
                match event.keystroke.key.as_str() {
                    "space" => this.set_playing(!this.playing),
                    "z" => this.zoom.set(if this.zoom.get() > 1.0 { 1.0 } else { 2.0 }),
                    _ => {}
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, _| {
                if this.scrub.is_some() && event.pressed_button == Some(MouseButton::Left) {
                    this.scrub(f32::from(event.position.x), window);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.end_scrub()),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .children(self.current.as_ref().map(|frame| {
                        surface(frame.buffer.clone())
                            .size_full()
                            .object_fit(ObjectFit::Contain)
                    })),
            )
            .child(
                div()
                    .h(px(28.))
                    .bg(rgb(0x1c1c1c))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, _| {
                            this.scrub(f32::from(event.position.x), window);
                        }),
                    )
                    .child(div().h_full().w(relative(position)).bg(rgb(0x3a6ea5))),
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .text_sm()
                    .text_color(rgb(0x9a9a9a))
                    .child(status),
            )
    }
}
