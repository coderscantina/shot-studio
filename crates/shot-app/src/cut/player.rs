//! Plays one review item of the Cut screen, picture and sound, with [`LEAD`] of what plays
//! before and after it: as recorded, or as it would play cut. The clip is the item's block cut
//! down to that stretch and planned like the Edit screen's preview, so it looks and sounds like
//! the export without the bookends.
//!
//! The render thread starts with the first item shown; the sound device opens on the first play.

use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use shot_core::{Cut, Decision, Id, Micros, Project, RangeEdit, TakeAt, TimeRange};
use shot_media::timing::ClockFollower;

use crate::engine::{Engine, Plan, PreviewFrame};
use crate::folder::VideoFolder;
use crate::model::{EditMap, FPS, View};
use crate::sound::Sound;

/// Kept media played on each side of an item.
pub const LEAD: Micros = Micros(2_000_000);
/// A frame picked at a render is on screen about one refresh later.
const VIDEO_AHEAD: Duration = Duration::from_micros(16_667);

/// How an item plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hear {
    /// As recorded, the item included.
    Around,
    /// As it plays once cut.
    Cut,
}

/// An item by its take and media range, which stay put while answers change the cut.
pub type ItemKey = (Id, TimeRange);

/// The cut that plays `range` of the block at `at` with up to [`LEAD`] of the block's kept media
/// on each side: one chapter with one block, the item kept or cut as C would cut it. The item
/// is an open review item, or a cut the rules applied, which plays restored as recorded. `None`
/// when the block or the item is not in `cut`.
pub fn clip(cut: &Cut, at: TakeAt, range: TimeRange, hear: Hear) -> Option<Cut> {
    let applied = cut
        .chapters
        .get(at.chapter)?
        .takes
        .get(at.take)?
        .ranges
        .iter()
        .any(
            |piece| matches!(piece, RangeEdit::Remove { range: removed, .. } if *removed == range),
        );
    let recorded = if applied {
        cut.restore(at, range).ok()?
    } else {
        cut.clone()
    };
    let source = match (hear, applied) {
        (Hear::Around, _) => recorded.clone(),
        (Hear::Cut, true) => cut.clone(),
        (Hear::Cut, false) => cut.decide(at, range, Decision::Cut).ok()?,
    };
    let chapter = source.chapters.get(at.chapter)?;
    let mut block = chapter.takes.get(at.take)?.clone();
    let kept: Vec<TimeRange> = recorded.chapters[at.chapter].takes[at.take]
        .ranges
        .iter()
        .filter_map(|piece| match piece {
            RangeEdit::Keep(kept) => Some(*kept),
            RangeEdit::Remove { .. } => None,
        })
        .collect();
    let window = TimeRange::new(
        lead_before(&kept, range.from()),
        lead_after(&kept, range.to()),
    )?;
    block.ranges = block
        .ranges
        .iter()
        .filter_map(|piece| {
            let inside = piece.range().intersect(window)?;
            Some(match *piece {
                RangeEdit::Keep(_) => RangeEdit::Keep(inside),
                RangeEdit::Remove { by, .. } => RangeEdit::Remove { range: inside, by },
            })
        })
        .collect();
    // Review items change no pixels, and the clip's are not answered here.
    block.reviews.clear();
    Some(Cut {
        // The bed would play from its start under every item.
        music: None,
        chapters: vec![shot_core::Chapter {
            title: chapter.title.clone(),
            takes: vec![block],
        }],
        derived: Vec::new(),
        ..cut.clone()
    })
}

/// Where [`LEAD`] of the kept media before `t` begins.
fn lead_before(kept: &[TimeRange], t: Micros) -> Micros {
    let mut left = LEAD.0;
    let mut start = t;
    for range in kept.iter().rev().filter(|r| r.from() < t) {
        let end = range.to().min(t);
        let taken = (end.0 - range.from().0).min(left);
        start = Micros(end.0 - taken);
        left -= taken;
        if left == 0 {
            break;
        }
    }
    start
}

/// Where [`LEAD`] of the kept media after `t` ends.
fn lead_after(kept: &[TimeRange], t: Micros) -> Micros {
    let mut left = LEAD.0;
    let mut end = t;
    for range in kept.iter().filter(|r| r.to() > t) {
        let start = range.from().max(t);
        let taken = (range.to().0 - start.0).min(left);
        end = Micros(start.0 + taken);
        left -= taken;
        if left == 0 {
            break;
        }
    }
    end
}

/// The wall clock that paces playback without sound.
#[derive(Clone, Copy)]
struct Clock {
    started: Instant,
    from: u64,
}

/// What the player has loaded.
struct Loaded {
    key: ItemKey,
    hear: Hear,
    /// The item played as recorded, which the waveform shows whichever way it plays.
    around: EditMap,
    /// The item's output frames in `around`.
    item: Range<u64>,
    /// What plays: `around`, or the clip cut.
    map: EditMap,
    dir: PathBuf,
    project: Project,
}

#[derive(Default)]
pub struct Player {
    engine: Option<Engine>,
    sound: Option<Sound>,
    loaded: Option<Loaded>,
    shown: Option<PreviewFrame>,
    playhead: u64,
    playing: bool,
    /// Nothing played since the item was loaded, so play starts at the lead-in.
    fresh: bool,
    clock: Option<Clock>,
    follower: ClockFollower,
    /// A seek's frame is still on its way.
    seeking: bool,
}

impl Player {
    /// Loads the item at `range` of the block at `at` of the folder's shown cut, to play as
    /// `hear`, and shows its first frame, or as cut the frame after the cut. Nothing changes
    /// when it is loaded that way already.
    pub fn load(
        &mut self,
        folder: &VideoFolder,
        at: TakeAt,
        range: TimeRange,
        hear: Hear,
    ) -> Result<(), String> {
        let shown = &folder.shown;
        let take = shown.cut.chapters[at.chapter].takes[at.take].take.clone();
        let key = (take, range);
        if self.loaded().is_some_and(|loaded| loaded == (&key, hear)) {
            return Ok(());
        }
        let plan = |hear| {
            let cut = clip(&shown.cut, at, range, hear)
                .ok_or_else(|| "this item is no longer in the cut".to_owned())?;
            let project = Project {
                cut,
                ..shown.clone()
            };
            Ok::<_, String>((EditMap::of(&project, View::Edited), project))
        };
        let (around, around_project) = plan(Hear::Around)?;
        let block = TakeAt {
            chapter: 0,
            take: 0,
        };
        let item = around.output_range(block, range).unwrap_or(0..0);
        let (map, project) = match hear {
            Hear::Around => (around.clone(), around_project),
            Hear::Cut => plan(Hear::Cut)?,
        };
        // As recorded, the poster is the item's first frame; cut, the frame after the cut.
        let poster = match hear {
            Hear::Around => item.start,
            Hear::Cut => map
                .output_frame(block, range.to())
                .unwrap_or(map.frames.saturating_sub(1)),
        };
        let engine = self.engine.get_or_insert_with(Engine::spawn);
        engine.set_plan(
            Plan {
                map: map.clone(),
                dir: folder.dir.clone(),
                effects: shown.format.effects(),
                theme: folder.theme.clone(),
            },
            poster,
        );
        if let Some(sound) = &self.sound {
            sound.set_playing(false);
            sound.set_plan(&map, folder.dir.clone(), project.clone(), poster);
        }
        self.loaded = Some(Loaded {
            key,
            hear,
            around,
            item,
            map,
            dir: folder.dir.clone(),
            project,
        });
        self.playhead = poster;
        self.playing = false;
        self.fresh = true;
        self.clock = None;
        self.seeking = true;
        Ok(())
    }

    /// Loads `cut`, a clip made elsewhere such as attempts of the Takes view, to play whole as
    /// recorded, and shows its first frame. `key` names it for [`Player::loaded`]; nothing
    /// changes when that key is loaded already.
    pub fn load_clip(
        &mut self,
        folder: &VideoFolder,
        key: ItemKey,
        cut: Cut,
    ) -> Result<(), String> {
        if self
            .loaded()
            .is_some_and(|loaded| loaded == (&key, Hear::Around))
        {
            return Ok(());
        }
        let project = Project {
            cut,
            ..folder.shown.clone()
        };
        let map = EditMap::of(&project, View::Edited);
        if map.frames == 0 {
            return Err("there is nothing to play here".to_owned());
        }
        let engine = self.engine.get_or_insert_with(Engine::spawn);
        engine.set_plan(
            Plan {
                map: map.clone(),
                dir: folder.dir.clone(),
                effects: folder.shown.format.effects(),
                theme: folder.theme.clone(),
            },
            0,
        );
        if let Some(sound) = &self.sound {
            sound.set_playing(false);
            sound.set_plan(&map, folder.dir.clone(), project.clone(), 0);
        }
        self.loaded = Some(Loaded {
            key,
            hear: Hear::Around,
            around: map.clone(),
            item: 0..map.frames,
            map,
            dir: folder.dir.clone(),
            project,
        });
        self.playhead = 0;
        self.playing = false;
        self.fresh = true;
        self.clock = None;
        self.seeking = true;
        Ok(())
    }

    /// The block of the loaded clip and its media time under the playhead.
    pub fn source(&self) -> Option<(TakeAt, Micros)> {
        self.loaded.as_ref()?.map.source_at(self.playhead)
    }

    /// The item loaded and how it plays.
    pub fn loaded(&self) -> Option<(&ItemKey, Hear)> {
        self.loaded.as_ref().map(|l| (&l.key, l.hear))
    }

    /// Stops and forgets the loaded item, so the next [`Player::load`] plans it again. The last
    /// frame stays on screen until the next one arrives.
    pub fn unload(&mut self) {
        self.pause();
        self.loaded = None;
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn frame(&self) -> Option<&PreviewFrame> {
        self.shown.as_ref()
    }

    /// Plays from the lead-in, or on from where a pause stopped.
    pub fn play(&mut self) {
        let Some(frames) = self.loaded.as_ref().map(|l| l.map.frames) else {
            return;
        };
        if self.fresh || self.playhead + 1 >= frames {
            self.seek(0);
        }
        self.fresh = false;
        if self.sound.is_none()
            && let Some(loaded) = &self.loaded
        {
            let sound = Sound::spawn();
            sound.set_plan(
                &loaded.map,
                loaded.dir.clone(),
                loaded.project.clone(),
                self.playhead,
            );
            self.sound = Some(sound);
        }
        self.playing = true;
        self.clock = None;
        if let Some(engine) = &self.engine {
            engine.set_playing(true);
        }
    }

    pub fn pause(&mut self) {
        if !self.playing {
            return;
        }
        self.playing = false;
        self.clock = None;
        if let Some(engine) = &self.engine {
            engine.set_playing(false);
        }
        if let Some(sound) = &self.sound {
            sound.set_playing(false);
        }
        self.seek(self.playhead);
    }

    fn seek(&mut self, frame: u64) {
        let frames = self.loaded.as_ref().map_or(0, |l| l.map.frames);
        let frame = frame.min(frames.saturating_sub(1));
        self.playhead = frame;
        if let Some(engine) = &self.engine {
            engine.seek(frame);
        }
        if let Some(sound) = &self.sound {
            sound.seek(frame);
        }
        self.seeking = true;
        self.clock = None;
    }

    /// Picks the frame for this render. True while frames are still to come.
    pub fn advance(&mut self) -> bool {
        let (Some(engine), Some(frames)) =
            (&self.engine, self.loaded.as_ref().map(|l| l.map.frames))
        else {
            return false;
        };
        if !self.playing {
            if self.seeking
                && let Some(frame) = engine.take_next()
            {
                self.playhead = frame.frame;
                self.shown = Some(frame);
                self.seeking = false;
            }
            return self.seeking;
        }
        let clock = match self.clock {
            Some(clock) => clock,
            None => {
                let Some(front) = engine.front() else {
                    return true;
                };
                // Sound starts with the first frame, so neither runs ahead of the other.
                if let Some(sound) = &self.sound {
                    sound.set_playing(true);
                }
                self.follower = ClockFollower::default();
                self.seeking = false;
                *self.clock.insert(Clock {
                    started: Instant::now(),
                    from: front,
                })
            }
        };
        let due = match &self.sound {
            Some(sound) if sound.is_clock() => match sound.heard(VIDEO_AHEAD) {
                Some(frame) => self.follower.frame(frame),
                // The device has not played yet: hold the frame on screen.
                None => return true,
            },
            _ => clock.from + (clock.started.elapsed().as_secs_f64() * f64::from(FPS)) as u64,
        };
        if let Some(frame) = engine.take_due(due) {
            self.playhead = frame.frame;
            self.shown = Some(frame);
        }
        if due + 1 >= frames {
            self.pause();
        }
        true
    }

    /// Where the playhead and the item sit along the waveform, as fractions of the clip played
    /// as recorded.
    pub fn marks(&self) -> Option<(f32, Range<f32>)> {
        let loaded = self.loaded.as_ref()?;
        let total = loaded.around.frames.max(1) as f32;
        let at = |frame: u64| frame as f32 / total;
        // Cut, the clip skips the item: map the playhead through the media it shows.
        let playhead = match loaded.hear {
            Hear::Around => self.playhead,
            Hear::Cut => loaded
                .map
                .source_at(self.playhead)
                .and_then(|(_, time)| {
                    let block = TakeAt {
                        chapter: 0,
                        take: 0,
                    };
                    loaded.around.output_frame(block, time)
                })
                .unwrap_or(0),
        };
        Some((at(playhead), at(loaded.item.start)..at(loaded.item.end)))
    }

    /// The take and media time under each of `bars` columns of the clip as recorded, for the
    /// waveform.
    pub fn columns(&self, bars: usize) -> Vec<Option<(Micros, Micros)>> {
        let Some(loaded) = &self.loaded else {
            return vec![None; bars];
        };
        let map = &loaded.around;
        (0..bars as u64)
            .map(|i| {
                let from = i * map.frames / bars as u64;
                let to = ((i + 1) * map.frames / bars as u64).max(from + 1);
                let (_, start) = map.source_at(from)?;
                let (_, end) = map.source_at(to - 1)?;
                Some((start, Micros(end.0 + 1_000_000 / u64::from(FPS))))
            })
            .collect()
    }

    pub fn error(&self) -> Option<String> {
        let engine = self.engine.as_ref().and_then(Engine::error);
        let sound = self
            .sound
            .as_ref()
            .and_then(Sound::error)
            .map(|e| format!("no sound: {e}"));
        engine.or(sound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{RemovedBy, Review, RuleKind};

    fn range(from: u64, to: u64) -> TimeRange {
        TimeRange::new(Micros(from), Micros(to)).unwrap()
    }

    /// A block over 0 to 20 s with 4 to 5 s removed and a pause at 10 to 11 s waiting.
    fn cut() -> Cut {
        let mut cut =
            Cut::default().append_take(Id::new("t01").unwrap(), "A", range(0, 20_000_000));
        let block = &mut cut.chapters[0].takes[0];
        block.ranges = vec![
            RangeEdit::Keep(range(0, 4_000_000)),
            RangeEdit::Remove {
                range: range(4_000_000, 5_000_000),
                by: RemovedBy::Manual,
            },
            RangeEdit::Keep(range(5_000_000, 20_000_000)),
        ];
        block.reviews.push(Review {
            range: range(10_000_000, 11_000_000),
            by: RuleKind::Pause,
            kept: false,
        });
        cut
    }

    const AT: TakeAt = TakeAt {
        chapter: 0,
        take: 0,
    };

    #[test]
    fn a_clip_plays_two_seconds_of_kept_media_on_each_side() {
        let around = clip(&cut(), AT, range(10_000_000, 11_000_000), Hear::Around).unwrap();
        assert_eq!(
            around.chapters[0].takes[0].ranges,
            vec![RangeEdit::Keep(range(8_000_000, 13_000_000))]
        );
        assert!(around.chapters[0].takes[0].reviews.is_empty());

        // Before 6 s the lead-in skips what the edit removed.
        let early = clip(&cut(), AT, range(6_000_000, 7_000_000), Hear::Around);
        assert_eq!(
            early.unwrap().chapters[0].takes[0].ranges,
            vec![
                RangeEdit::Keep(range(3_000_000, 4_000_000)),
                RangeEdit::Remove {
                    range: range(4_000_000, 5_000_000),
                    by: RemovedBy::Manual
                },
                RangeEdit::Keep(range(5_000_000, 9_000_000)),
            ]
        );
    }

    #[test]
    fn played_cut_the_clip_removes_the_item_as_c_would() {
        let played = clip(&cut(), AT, range(10_000_000, 11_000_000), Hear::Cut).unwrap();
        assert_eq!(
            played.chapters[0].takes[0].ranges,
            vec![
                RangeEdit::Keep(range(8_000_000, 10_000_000)),
                RangeEdit::Remove {
                    range: range(10_000_000, 11_000_000),
                    by: RemovedBy::Rule(RuleKind::Pause)
                },
                RangeEdit::Keep(range(11_000_000, 13_000_000)),
            ]
        );
        assert!(clip(&cut(), AT, range(1, 2), Hear::Cut).is_none());
    }
}
