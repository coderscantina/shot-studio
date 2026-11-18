//! Plays a clip of the Takes view, picture and sound: attempts cut down to a clip and planned
//! like the Edit screen's preview, so they look and sound like the export without the bookends.
//!
//! The render thread starts with the first clip shown; the sound device opens on the first play.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use shot_core::{Cut, Id, Lock, Micros, Project, TakeAt, TimeRange};
use shot_media::timing::ClockFollower;

use crate::engine::{Engine, Plan, PreviewFrame};
use crate::folder::VideoFolder;
use crate::model::{EditMap, FPS, View};
use crate::sound::Sound;

/// A frame picked at a render is on screen about one refresh later.
const VIDEO_AHEAD: Duration = Duration::from_micros(16_667);

/// An item by its take and media range, which stay put while answers change the cut.
pub type ItemKey = (Id, TimeRange);

/// The wall clock that paces playback without sound.
#[derive(Clone, Copy)]
struct Clock {
    started: Instant,
    from: u64,
}

/// What the player has loaded.
struct Loaded {
    key: ItemKey,
    map: EditMap,
    dir: PathBuf,
    project: Project,
    lock: Lock,
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
    /// Loads `cut`, a clip such as attempts of the Takes view, to play whole, and shows its
    /// first frame. `key` names it for [`Player::loaded`]; nothing
    /// changes when that key is loaded already.
    pub fn load_clip(
        &mut self,
        folder: &VideoFolder,
        key: ItemKey,
        cut: Cut,
    ) -> Result<(), String> {
        if self.loaded() == Some(&key) {
            return Ok(());
        }
        let project = Project {
            cut,
            ..folder.project.clone()
        };
        let lock = folder.lock().clone();
        let map = EditMap::of(&project, &lock, View::Edited);
        if map.frames == 0 {
            return Err("there is nothing to play here".to_owned());
        }
        let engine = self.engine.get_or_insert_with(Engine::spawn);
        engine.set_plan(
            Plan {
                map: map.clone(),
                dir: folder.dir.clone(),
                effects: folder.project.format.effects(),
                theme: folder.theme.clone(),
                captions: None,
            },
            0,
        );
        if let Some(sound) = &self.sound {
            sound.set_playing(false);
            sound.set_plan(&map, folder.dir.clone(), project.clone(), lock.clone(), 0);
        }
        self.loaded = Some(Loaded {
            key,
            map,
            dir: folder.dir.clone(),
            project,
            lock,
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

    /// The clip loaded.
    pub fn loaded(&self) -> Option<&ItemKey> {
        self.loaded.as_ref().map(|l| &l.key)
    }

    /// Stops and forgets the loaded item, so the next [`Player::load_clip`] plans it again. The last
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
                loaded.lock.clone(),
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
