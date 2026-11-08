//! The take being recorded, live: a thread that streams the take's elements as words, clicks,
//! keys and markers arrive ([`Stream`]), and while the monitor is styled rebuilds the take's
//! style from them and paints capture's newest frames with shot-render's [`Monitor`] at 60 Hz.
//!
//! The thread starts with the recorder and makes the style and the GPU monitor while the models
//! load and capture starts, so the first styled frame follows the first captured one.
//!
//! The style is what the stop path will write: the take appended to `cut.kdl` as a chapter of
//! its own with its markers as nodes, and on a migrated video the lock built from the files
//! with the take's live elements, its cuts left out. A build takes a few milliseconds and runs
//! only when the elements change. The thread never touches the capture queues: it reads the
//! preview's converted frames, so a slow frame skips a refresh and never holds up the recording.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBuffer;
use shot_analyze::elements::Stream;
use shot_capture::Preview;
use shot_capture::events::Marker;
use shot_core::{
    Applied, Elements, Id, Micros, Project, StyleInput, TimeRange, Timeline, Transcript, Word,
};
use shot_mcp::style::{self, Disk, Inputs};
use shot_render::events::EventLog;
use shot_render::layout::Canvas;
use shot_render::monitor::{Feed, Monitor, Styled};
use shot_render::theme::Theme;

/// The monitor's render size; the stage shows it at about 900 points, 1800 pixels.
const CANVAS: Canvas = Canvas {
    width: 1920,
    height: 1080,
};
const REFRESH: Duration = Duration::from_micros(16_667);
/// The live take's kept range: longer than any take.
const OPEN_END: Micros = Micros(24 * 3600 * 1_000_000);

/// What the transcriber hears, from its thread.
pub enum Heard {
    Word(Word),
    Tentative(Vec<Word>),
}

/// The video as the take starts: its files, and whether its style comes from a lock.
pub struct Setup {
    pub dir: PathBuf,
    pub project: Project,
    pub migrated: bool,
}

/// What the Record screen reads from the live take.
#[derive(Default)]
pub struct View {
    frame: Mutex<Option<CFRetained<CVPixelBuffer>>>,
    error: Mutex<Option<String>>,
}

// SAFETY: CoreVideo buffers are reference counted atomically and the thread never writes a
// buffer once it is published.
unsafe impl Send for View {}
unsafe impl Sync for View {}

impl View {
    /// The newest styled frame, `None` before the first one or while the feed is raw.
    pub fn frame(&self) -> Option<CFRetained<CVPixelBuffer>> {
        self.frame.lock().expect("monitor frame").clone()
    }

    /// Why the monitor cannot show the style, when it cannot.
    pub fn error(&self) -> Option<String> {
        self.error.lock().expect("monitor error").clone()
    }

    fn fail(&self, error: String) {
        self.frame.lock().expect("monitor frame").take();
        *self.error.lock().expect("monitor error") = Some(error);
    }
}

pub struct LiveTake {
    view: Arc<View>,
    quit: Arc<AtomicBool>,
    feeds: Option<Sender<Feeds>>,
    thread: JoinHandle<()>,
}

/// The live take's inputs besides the setup.
pub struct Feeds {
    pub take: Id,
    /// When media time zero was.
    pub origin: Instant,
    pub preview: Preview,
    /// The recorded screen's pixel size.
    pub screen_size: (u32, u32),
    pub heard: Receiver<Heard>,
    /// Whether the monitor shows the style, switchable while recording.
    pub styled: Arc<AtomicBool>,
}

impl LiveTake {
    /// Starts the thread for take `expected`, the id capture will most likely give the take,
    /// and makes the style and the monitor for it. [`LiveTake::record`] hands it the capture.
    pub fn start(setup: Setup, expected: Id) -> Self {
        let view = Arc::new(View::default());
        let quit = Arc::new(AtomicBool::new(false));
        let (feeds, received) = mpsc::channel::<Feeds>();
        let thread = std::thread::Builder::new()
            .name("shot-live-take".into())
            .spawn({
                let (view, quit) = (view.clone(), quit.clone());
                move || {
                    let ready = prepare(&setup, &expected);
                    if let Ok(feeds) = received.recv() {
                        // Another take id draws chapter titles for the wrong take: make it again.
                        let ready = (feeds.take == expected).then_some(ready);
                        run(setup, feeds, ready, &view, &quit);
                    }
                }
            })
            .expect("spawning the live take thread");
        Self {
            view,
            quit,
            feeds: Some(feeds),
            thread,
        }
    }

    /// Starts streaming and painting the running capture.
    pub fn record(&mut self, feeds: Feeds) {
        if let Some(sender) = self.feeds.take() {
            let _ = sender.send(feeds);
        }
    }

    pub fn view(&self) -> Arc<View> {
        self.view.clone()
    }

    /// Stops after the current refresh, once the elements are written.
    pub fn stop(mut self) {
        self.feeds.take();
        self.quit.store(true, Ordering::Relaxed);
        let _ = self.thread.join();
    }
}

fn run(
    setup: Setup,
    feeds: Feeds,
    ready: Option<Result<(LiveStyle, Monitor), String>>,
    view: &View,
    quit: &AtomicBool,
) {
    let mut stream = match Stream::create(&setup.dir, &feeds.take) {
        Ok(stream) => stream,
        Err(e) => return view.fail(format!("elements: {e}")),
    };
    // The style and the GPU monitor, made again when the monitor is first styled if they are
    // missing. After a failure the monitor stays raw and the elements stream on.
    let mut painter: Option<(LiveStyle, Monitor)> = None;
    let mut failed = false;
    match ready {
        Some(Ok(ready)) => painter = Some(ready),
        Some(Err(error)) => {
            failed = true;
            view.fail(error);
        }
        None => {}
    }
    let mut styled: Option<Styled> = None;
    let mut events: Option<Arc<EventLog>> = None;
    let mut next = Instant::now();
    while !quit.load(Ordering::Relaxed) {
        next += REFRESH;
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
        while let Ok(heard) = feeds.heard.try_recv() {
            match heard {
                Heard::Word(word) => stream.word(word),
                Heard::Tentative(words) => stream.tentative(words),
            }
        }
        // A failed read or write is tried again on the next refresh; the stop path writes
        // the elements once more.
        let lines = stream.poll().unwrap_or(0);
        let changed = stream.update().unwrap_or(true);
        if !feeds.styled.load(Ordering::Relaxed) {
            view.frame.lock().expect("monitor frame").take();
            continue;
        }
        if failed {
            continue;
        }
        if painter.is_none() {
            match prepare(&setup, &feeds.take) {
                Ok(ready) => painter = Some(ready),
                Err(error) => {
                    failed = true;
                    view.fail(error);
                    continue;
                }
            }
        }
        let Some((live, monitor)) = &mut painter else {
            continue;
        };
        let now = Micros(feeds.origin.elapsed().as_micros() as u64);
        if changed || styled.is_none() {
            match live.styled(stream.live(), stream.markers()) {
                Ok(new) => styled = Some(new),
                Err(error) => {
                    failed = true;
                    view.fail(error);
                    continue;
                }
            }
            monitor.hear(
                &heard(&feeds.take, &stream),
                &live.project.cut.caption_fixes,
                now,
            );
        }
        if lines > 0 || events.is_none() {
            events = EventLog::parse(&stream.recent_events()).ok().map(Arc::new);
        }
        let (Some(styled), Some(screen)) = (&styled, feeds.preview.screen()) else {
            continue;
        };
        let camera = feeds.preview.camera();
        let feed = Feed {
            time: now,
            screen: &screen.buffer,
            camera: camera.as_ref().map(|c| &*c.buffer),
            screen_size: feeds.screen_size,
            events: events.clone(),
        };
        match monitor.paint(styled, &feed) {
            Ok(frame) => *view.frame.lock().expect("monitor frame") = Some(frame),
            Err(e) => {
                failed = true;
                view.fail(e.to_string());
            }
        }
    }
    let _ = stream.update();
}

/// The settled and tentative words as one transcript, for the captions.
fn heard(take: &Id, stream: &Stream) -> Transcript {
    let mut transcript = Transcript::new(take.clone());
    for word in stream.heard() {
        // A tentative word that overlaps the settled ones is left out.
        let _ = transcript.push(word.clone());
    }
    transcript
}

fn prepare(setup: &Setup, take: &Id) -> Result<(LiveStyle, Monitor), String> {
    let live = LiveStyle::new(setup, take)?;
    let monitor =
        Monitor::new(&setup.dir, live.theme.clone(), CANVAS).map_err(|e| e.to_string())?;
    Ok((live, monitor))
}

/// Builds the take's style as the stop path will write it.
struct LiveStyle {
    take: Id,
    project: Project,
    migrated: bool,
    theme: Arc<Theme>,
    /// A migrated video in its channel repo: what a build reads, the live take's elements
    /// last in `inputs.elements`.
    inputs: Option<Inputs>,
}

impl LiveStyle {
    fn new(setup: &Setup, take: &Id) -> Result<Self, String> {
        let project = setup.project.clone();
        // A video's first take in a channel repo migrates it at stop, so it is styled now.
        let styles = setup.migrated || project.cut.chapters.is_empty();
        let inputs = match Disk::locate(&setup.dir) {
            Ok((disk, slug)) if styles => {
                let mut inputs = style::inputs(&disk, &slug)?;
                inputs.elements.retain(|e| e.take() != take);
                inputs.elements.push(Elements::new(take.clone()));
                Some(inputs)
            }
            _ => None,
        };
        let migrated = setup.migrated || inputs.is_some();
        let cut = crate::folder::appended(
            &project.cut,
            take,
            open_range(),
            &[],
            &project.format,
            migrated,
        );
        let theme = Theme::load(&setup.dir, project.video.format, &cut)
            .map(Arc::new)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            take: take.clone(),
            project,
            migrated,
            theme,
            inputs,
        })
    }

    /// The take styled with `elements` and `markers`.
    fn styled(&mut self, elements: &Elements, markers: &[Marker]) -> Result<Styled, String> {
        let cut = crate::folder::appended(
            &self.project.cut,
            &self.take,
            open_range(),
            markers,
            &self.project.format,
            self.migrated,
        );
        let project = Project {
            cut,
            ..self.project.clone()
        };
        let timeline = match &mut self.inputs {
            Some(inputs) => {
                *inputs.elements.last_mut().expect("the live take's slot") = elements.clone();
                let mut lock = shot_core::build(&StyleInput {
                    imported: &inputs.imported,
                    brand: &inputs.brand,
                    format: &inputs.format,
                    video: &inputs.video_layer,
                    cut: &project.cut,
                    elements: &inputs.elements,
                    assets: &inputs.assets,
                })
                .map_err(|e| e.to_string())?;
                if let Some(live) = lock.takes.iter_mut().find(|t| t.take == self.take) {
                    live.instances
                        .retain(|i| !matches!(i.applied, Applied::Cut(_)));
                }
                Timeline::styled(&project, &lock)
            }
            None => Timeline::evaluate(&project),
        };
        let main = timeline.main();
        Ok(Styled {
            segments: main
                .chapters
                .last()
                .map(|c| c.segments.clone())
                .unwrap_or_default(),
            effects: main.effects,
            captions: main.captions,
        })
    }
}

fn open_range() -> TimeRange {
    TimeRange::new(Micros(0), OPEN_END).expect("the open range is not empty")
}
