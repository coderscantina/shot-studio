//! The Record screen's monitor: the take being recorded drawn as the cut will show it, from
//! capture's live frames. A frame composes as a segment frame of [`crate::paint`] does, through
//! [`compose`] and the same render graph: the layout, the zooms, the cursor and keystrokes, and
//! the overlays the style gave the take so far. It has no cuts, bookends or transitions, and a
//! b-roll clip leaves the take's screen showing.
//!
//! Captions follow the voice as it is heard. The transcriber settles words a second or two after
//! they are said, past the time export shows them, so the monitor shows the cue of the newest
//! words from when they arrive until the voice has been quiet for [`CAPTION_HOLD`], the newest
//! word highlighted in Word pop.

use std::path::Path;
use std::ptr::NonNull;
use std::sync::Arc;

use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBuffer;
use shot_core::{CaptionFix, CaptionStyle, Effects, Micros, Segment, Transcript};

use crate::RenderError;
use crate::assets::Assets;
use crate::captions::{self, Cue};
use crate::events::EventLog;
use crate::frame::{Look, Shot, compose};
use crate::graph::Renderer;
use crate::kinetic;
use crate::layout::{Canvas, Sources};
use crate::paint::PaintError;
use crate::surface::{BufferPool, Matrix, Range, size};
use crate::theme::Theme;

/// How long the newest cue stays after its last word arrived.
pub const CAPTION_HOLD: Micros = Micros(2_500_000);

/// What the style makes of the take so far, in the take's media time.
#[derive(Debug, Clone)]
pub struct Styled {
    /// The take's segments, covering it without cuts.
    pub segments: Vec<Segment>,
    pub effects: Effects,
    pub captions: CaptionStyle,
}

impl Styled {
    fn segment_at(&self, t: Micros) -> Option<&Segment> {
        self.segments
            .iter()
            .rev()
            .find(|s| s.source.from() <= t)
            .or(self.segments.first())
    }
}

/// One moment of the take: its media time and capture's newest frames.
pub struct Feed<'a> {
    pub time: Micros,
    pub screen: &'a CVPixelBuffer,
    pub camera: Option<&'a CVPixelBuffer>,
    /// The recorded screen's size in pixels, which the event log counts in.
    pub screen_size: (u32, u32),
    /// The take's latest events, enough for the cursor, ripples and key labels of `time`.
    pub events: Option<Arc<EventLog>>,
}

/// See the module docs. One per monitor thread.
pub struct Monitor {
    renderer: Renderer,
    assets: Assets,
    pool: BufferPool,
    canvas: Canvas,
    caption: LiveCaption,
}

impl Monitor {
    /// A monitor drawing at `canvas` size into `420f` BT.601 buffers, what gpui's surface
    /// element shows.
    pub fn new(video_dir: &Path, theme: Arc<Theme>, canvas: Canvas) -> Result<Self, RenderError> {
        Ok(Self {
            renderer: Renderer::new()?,
            assets: Assets::with_theme(video_dir, theme),
            pool: BufferPool::new(canvas.width, canvas.height, Range::Full, Matrix::Bt601)?,
            canvas,
            caption: LiveCaption::default(),
        })
    }

    /// Takes the words heard so far, settled and tentative, at take time `at`.
    pub fn hear(&mut self, words: &Transcript, fixes: &[CaptionFix], at: Micros) {
        self.caption.hear(words, fixes, at);
    }

    /// Draws `feed` styled. Without segments there is no style to draw and the screen shows as
    /// captured.
    pub fn paint(
        &mut self,
        styled: &Styled,
        feed: &Feed,
    ) -> Result<CFRetained<CVPixelBuffer>, PaintError> {
        let Some(segment) = styled.segment_at(feed.time) else {
            // SAFETY: `feed.screen` is a live CoreVideo buffer; retaining it adds a reference.
            return Ok(unsafe { CFRetained::retain(NonNull::from(feed.screen)) });
        };
        let look = Look {
            events: feed.events.clone(),
            ..self
                .assets
                .look(&segment.take, &segment.overlays, styled.effects)?
        };
        let shot = Shot {
            layout: segment.layout,
            zooms: &segment.zooms,
            overlays: &segment.overlays,
            time: feed.time,
        };
        let sizes = Sources {
            screen: feed.screen_size,
            camera: feed.camera.map_or((16, 9), size),
        };
        let mut frame = compose(&shot, &look, self.canvas, sizes)?;
        frame.cutaway = None;
        if let Some((cue, word, pop)) = self.caption.at(styled.captions, feed.time) {
            frame
                .drawing
                .over
                .push(crate::derived::caption_shape(cue, word, pop, self.canvas));
        }
        let sources = Sources {
            screen: Some(feed.screen),
            camera: feed.camera,
        };
        frame
            .scene
            .layers
            .retain(|layer| sources.get(layer.source).is_some());
        let target = self.pool.buffer()?;
        self.renderer.draw(&frame, sources, &target)?;
        Ok(target)
    }
}

/// The newest cue heard and the take time it arrived.
#[derive(Debug, Default)]
struct LiveCaption {
    heard: Option<(Cue, Micros)>,
}

impl LiveCaption {
    fn hear(&mut self, words: &Transcript, fixes: &[CaptionFix], at: Micros) {
        let newest = captions::group(captions::apply_fixes(words, fixes)).pop();
        if newest.as_ref() != self.heard.as_ref().map(|(cue, _)| cue) {
            self.heard = newest.map(|cue| (cue, at));
        }
    }

    /// The cue on screen at `t`, the word it highlights and how long ago that word popped.
    fn at(&self, style: CaptionStyle, t: Micros) -> Option<(&Cue, Option<usize>, Micros)> {
        let (cue, arrived) = self.heard.as_ref()?;
        let since = Micros(t.0.checked_sub(arrived.0)?);
        if style == CaptionStyle::Off || since >= CAPTION_HOLD {
            return None;
        }
        Some(match style {
            CaptionStyle::WordPop => (cue, cue.words.len().checked_sub(1), since),
            _ => (cue, None, kinetic::word_pop_settle()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{Id, TimeRange, Word};

    fn heard(text: &str) -> Transcript {
        let mut transcript = Transcript::new(Id::new("t01").unwrap());
        for (i, token) in text.split(' ').enumerate() {
            let from = 1_000_000 + i as u64 * 300_000;
            transcript
                .push(Word {
                    text: token.to_owned(),
                    range: TimeRange::new(Micros(from), Micros(from + 300_000)).unwrap(),
                    confidence: 90,
                })
                .unwrap();
        }
        transcript
    }

    #[test]
    fn the_newest_cue_shows_from_its_arrival_until_the_voice_is_quiet() {
        let mut caption = LiveCaption::default();
        caption.hear(&heard("we ship"), &[], Micros(3_000_000));
        let shown = |c: &LiveCaption, t: u64| {
            c.at(CaptionStyle::WordPop, Micros(t))
                .map(|(cue, word, _)| (cue.text(), word))
        };
        assert_eq!(shown(&caption, 2_900_000), None);
        assert_eq!(
            shown(&caption, 3_000_000),
            Some(("we ship".into(), Some(1)))
        );
        // The same words heard again later keep their arrival.
        caption.hear(&heard("we ship"), &[], Micros(4_000_000));
        assert_eq!(
            shown(&caption, 5_499_999),
            Some(("we ship".into(), Some(1)))
        );
        assert_eq!(shown(&caption, 5_500_000), None);
        caption.hear(&heard("we ship on"), &[], Micros(6_000_000));
        assert_eq!(
            shown(&caption, 7_000_000),
            Some(("we ship on".into(), Some(2)))
        );
        assert_eq!(caption.at(CaptionStyle::Off, Micros(7_000_000)), None);
        let clean = caption.at(CaptionStyle::Clean, Micros(7_000_000)).unwrap();
        assert_eq!(clean.1, None);
    }

    #[test]
    #[ignore = "needs the GPU and the generated 30 s fixtures"]
    fn a_live_frame_draws_as_the_painter_draws_the_same_moment_of_the_cut() {
        use std::time::Duration;

        use shot_core::{Cut, FormatKind, MediaId, Project, Script, Timeline, Video};
        use shot_media::videotoolbox::Decoder;

        use crate::cursor::FrameCursor;
        use crate::paint::Painter;
        use crate::plan::{ExportPlan, frame_time};
        use crate::surface::read_rgb;

        let fixtures = std::path::PathBuf::from(std::env::var("SHOT_FIXTURES").unwrap());
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media/t01");
        std::fs::create_dir_all(&media).unwrap();
        for (from, to) in [
            ("screen-30s.mp4", "screen.mp4"),
            ("camera-30s.mp4", "camera.mp4"),
            ("screen-30s.events.jsonl", "events.jsonl"),
        ] {
            std::os::unix::fs::symlink(fixtures.join(from), media.join(to)).unwrap();
        }
        let cut = r#"
chapter "One" {
    take t01 {
        keep from=0 to=10_000_000
        layout split from=0 to=10_000_000
        zoom 150 from=2_000_000 to=4_000_000
        overlay link-card from=1_000_000 to=7_000_000 {
            url "https://github.com/zed-industries/zed"
        }
    }
}
"#;
        let video = "title \"Live\"\nstatus editing\nformat opinion\n";
        std::fs::write(dir.path().join("video.kdl"), video).unwrap();
        std::fs::write(dir.path().join("cut.kdl"), cut).unwrap();
        let project = Project {
            video: Video::parse(video).unwrap(),
            script: Script::parse("# t\n").unwrap(),
            format: FormatKind::Opinion.preset(),
            cut: Cut::parse(cut).unwrap(),
        };
        let timeline = Timeline::evaluate(&project);
        let main = timeline.main();
        let plan = ExportPlan::new(main, 60, |_| MediaId::default());
        let theme = Arc::new(Theme::open(dir.path()).unwrap());
        let canvas = Canvas {
            width: 1280,
            height: 720,
        };
        let mut monitor = Monitor::new(dir.path(), theme.clone(), canvas).unwrap();
        let mut painter = Painter::with_theme(dir.path(), main.effects, theme);
        let renderer = Renderer::new().unwrap();
        let pool =
            BufferPool::new(canvas.width, canvas.height, Range::Full, Matrix::Bt601).unwrap();
        let styled = Styled {
            segments: main.chapters[0].segments.clone(),
            effects: main.effects,
            captions: CaptionStyle::Off,
        };
        let open = |file: &str| FrameCursor::new(Decoder::open_native(&media.join(file)).unwrap());
        let (mut screen, mut camera) = (open("screen.mp4"), open("camera.mp4"));
        let events = std::fs::read_to_string(media.join("events.jsonl")).unwrap();
        let events = Arc::new(EventLog::parse(&events).unwrap());
        for frame in [30, 150, 400] {
            let time = frame_time(frame, 60);
            let at = Duration::from_micros(time.0);
            let screen = screen.goto(at).unwrap().unwrap().buffer.clone();
            let camera = camera.goto(at).unwrap().unwrap().buffer.clone();
            let live = monitor
                .paint(
                    &styled,
                    &Feed {
                        time,
                        screen: &screen,
                        camera: Some(&camera),
                        screen_size: size(&screen),
                        events: Some(events.clone()),
                    },
                )
                .unwrap();
            let painted = pool.buffer().unwrap();
            painter.paint(&renderer, &plan, frame, &painted).unwrap();
            let (a, b) = (read_rgb(&live).unwrap(), read_rgb(&painted).unwrap());
            crate::golden::assert_match(&format!("frame {frame}"), &a, &b);
        }
    }
}
