//! The motion system: springs solved in closed form over media time, choreographed across the
//! parts of an element, in four presets tuned by feel controls. Pure.
//!
//! Nothing here keeps state: a pose is a function of the element's range and the media time, so
//! a frame looks the same whether playback reached it or a seek did.
//!
//! An element on screen over a range moves in phases: it travels in on its entrance spring, hits
//! its target and settles, holds, then exits on a faster spring without bounce that finishes by
//! the end of the range. Its parts (a card's panel, title and label, a line's words) follow each
//! other by the stagger, and exit in the same order at half the stagger.
//!
//! The feel controls map to springs like this, each from 0 to 100:
//!
//! | Control | 0 | 100 |
//! | --- | --- | --- |
//! | `tempo` | entrance response 900 ms | 225 ms, halving every 50 |
//! | `bounce` | damping ratio 1, no overshoot | 0.4, about 25% overshoot |
//! | `density` | 30% of a template's travel and tilt | 120% |
//! | `stagger` | parts move together | 120 ms between parts |
//!
//! Exits respond in 60% of the entrance's time, critically damped.

use std::str::FromStr;

use shot_core::{Micros, TimeRange};

/// How close to its target a spring counts as settled, as a fraction of its travel. A 4K card
/// travelling across the screen lands within half a pixel.
const SETTLED: f32 = 1e-4;

/// A damped spring from 0 to 1, starting at rest. `value` is its step response in closed form.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    /// Natural angular frequency, radians per second.
    omega: f32,
    /// Damping ratio, in (0, 1]. 1 is critically damped.
    zeta: f32,
}

impl Spring {
    /// A spring whose undamped period is `response` seconds, with `damping` as its ratio, clamped
    /// to (0.05, 1].
    pub const fn new(response: f32, damping: f32) -> Self {
        Self {
            omega: std::f32::consts::TAU / response.max(1e-3),
            zeta: damping.clamp(0.05, 1.0),
        }
    }

    /// The same spring, sped up or slowed down so it settles in `duration`.
    pub fn settling_in(self, duration: Micros) -> Self {
        let wanted = duration.0.max(1) as f32 / 1e6;
        Self {
            omega: self.omega * self.settle() / wanted,
            ..self
        }
    }

    /// Position `seconds` after release, 0 before it. Exactly 1 once settled.
    pub fn value(self, seconds: f32) -> f32 {
        if seconds <= 0.0 {
            return 0.0;
        }
        if seconds >= self.settle() {
            return 1.0;
        }
        let Self { omega, zeta } = self;
        let decay = (-zeta * omega * seconds).exp();
        if zeta >= 1.0 {
            return 1.0 - decay * (1.0 + omega * seconds);
        }
        let damped = omega * (1.0 - zeta * zeta).sqrt();
        let phase = damped * seconds;
        1.0 - decay * (phase.cos() + zeta * omega / damped * phase.sin())
    }

    /// Seconds until the spring stays within [`SETTLED`] of its target.
    pub fn settle(self) -> f32 {
        let Self { omega, zeta } = self;
        if zeta >= 1.0 {
            // Solves e^-x (1 + x) = SETTLED for x = omega t by Newton's method.
            let mut x: f32 = 12.0;
            for _ in 0..8 {
                let f = (-x).exp() * (1.0 + x) - SETTLED;
                let slope = -x * (-x).exp();
                x -= f / slope;
            }
            return x / omega;
        }
        // The envelope of the oscillation.
        (1.0 / (SETTLED * (1.0 - zeta * zeta).sqrt())).ln() / (zeta * omega)
    }

    /// Seconds until the spring first reaches its target: the impact of a bouncing move, the
    /// settle of one without bounce.
    pub fn impact(self) -> f32 {
        if self.zeta >= 1.0 {
            return self.settle();
        }
        std::f32::consts::PI / (self.omega * (1.0 - self.zeta * self.zeta).sqrt())
    }
}

/// The four motion presets, one per format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Preset {
    /// Reactions: fast springs and pops.
    Reactive,
    /// Opinions: bold kinetic type and mask reveals.
    Editorial,
    /// Product tours: glass, depth and slow springs.
    Launch,
    /// Tutorials: legibility first, minimal motion.
    Calm,
}

impl Preset {
    pub const ALL: [Self; 4] = [Self::Reactive, Self::Editorial, Self::Launch, Self::Calm];

    pub fn name(self) -> &'static str {
        match self {
            Self::Reactive => "reactive",
            Self::Editorial => "editorial",
            Self::Launch => "launch",
            Self::Calm => "calm",
        }
    }

    /// The preset's own feel, which a brand tunes.
    pub fn feel(self) -> Feel {
        let feel = |tempo, bounce, density, stagger| Feel {
            tempo,
            bounce,
            density,
            stagger,
        };
        match self {
            Self::Reactive => feel(80, 60, 70, 40),
            Self::Editorial => feel(60, 20, 60, 50),
            Self::Launch => feel(40, 10, 30, 60),
            Self::Calm => feel(30, 0, 10, 20),
        }
    }

    /// How the preset's elements look and come in, besides their springs.
    pub fn moves(self) -> Moves {
        let moves = |reveal, unit, glass, tilt| Moves {
            reveal,
            unit,
            glass,
            tilt,
        };
        match self {
            Self::Reactive => moves(Reveal::Pop, Unit::Word, false, 0.0),
            Self::Editorial => moves(Reveal::Mask, Unit::Word, false, 0.0),
            Self::Launch => moves(Reveal::Rise, Unit::Glyph, true, 14.0),
            Self::Calm => moves(Reveal::Fade, Unit::Word, false, 0.0),
        }
    }

    /// The preset's motion with its own feel.
    pub fn motion(self) -> Motion {
        Motion::new(self.feel())
    }
}

impl FromStr for Preset {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|p| p.name() == name)
            .ok_or_else(|| format!("there is no motion preset `{name}`"))
    }
}

/// The feel controls, each from 0 to 100. See the module docs for what they map to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Feel {
    pub tempo: u8,
    pub bounce: u8,
    pub density: u8,
    pub stagger: u8,
}

/// How text comes in, unit by unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reveal {
    /// Rises a little and fades in.
    Rise,
    /// Slides up out of a mask at its line.
    Mask,
    /// Scales up from its center past full size and settles.
    Pop,
    /// Fades in with a hint of rise.
    Fade,
}

/// What moves on its own spring in kinetic type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unit {
    Word,
    Glyph,
}

/// A preset's look besides its springs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moves {
    pub reveal: Reveal,
    pub unit: Unit,
    /// Cards sit on frosted glass.
    pub glass: bool,
    /// How far cards tilt in perspective on their way in, in degrees at full density.
    pub tilt: f32,
}

/// Springs and choreography for elements on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    pub enter: Spring,
    pub exit: Spring,
    /// Between one part's entrance and the next.
    pub stagger: Micros,
    /// How much of a template's travel, tilt and scale the moves use.
    pub travel: f32,
}

impl Motion {
    pub fn new(feel: Feel) -> Self {
        let unit = |v: u8| f32::from(v.min(100)) / 100.0;
        let response = 0.9 * 0.25_f32.powf(unit(feel.tempo));
        Self {
            enter: Spring::new(response, 1.0 - 0.6 * unit(feel.bounce)),
            exit: Spring::new(response * 0.6, 1.0),
            stagger: Micros((120_000.0 * unit(feel.stagger)) as u64),
            travel: 0.3 + 0.9 * unit(feel.density),
        }
    }

    /// The same motion with its springs settling in `enter` and `exit`, the duration hints an
    /// overlay may carry.
    pub fn with_hints(self, enter: Option<Micros>, exit: Option<Micros>) -> Self {
        Self {
            enter: enter.map_or(self.enter, |d| self.enter.settling_in(d)),
            exit: exit.map_or(self.exit, |d| self.exit.settling_in(d)),
            ..self
        }
    }

    /// When part `part` of an element on screen over `range` starts to exit.
    fn exit_start(&self, range: TimeRange, part: Part) -> Micros {
        let span = seconds_to_micros(self.exit.settle()) + self.exit_stagger().0 * part.last();
        let start = range.to().0.saturating_sub(span) + self.exit_stagger().0 * part.index as u64;
        Micros(start.max(range.from().0))
    }

    fn exit_stagger(&self) -> Micros {
        Micros(self.stagger.0 / 2)
    }

    fn enter_start(&self, range: TimeRange, part: Part) -> Micros {
        Micros(range.from().0 + self.stagger.0 * part.index as u64)
    }

    /// The pose of `part` of an element on screen over `range`, at media time `t`.
    pub fn pose(&self, range: TimeRange, part: Part, t: Micros) -> Pose {
        if t < range.from() || t >= range.to() {
            return Pose::GONE;
        }
        let since = |start: Micros| (t.0 as i64 - start.0 as i64) as f32 / 1e6;
        Pose {
            enter: self.enter.value(since(self.enter_start(range, part))),
            exit: self.exit.value(since(self.exit_start(range, part))),
        }
    }

    /// Which phase `part` is in at `t`, for sounds that follow a move.
    pub fn phase(&self, range: TimeRange, part: Part, t: Micros) -> Phase {
        if t < range.from() || t >= range.to() {
            return Phase::Off;
        }
        let enter = self.enter_start(range, part);
        let exit = self.exit_start(range, part);
        let at = |start: Micros, seconds: f32| Micros(start.0 + seconds_to_micros(seconds));
        if t >= exit {
            Phase::Exit
        } else if t < enter {
            Phase::Waiting
        } else if t < at(enter, self.enter.impact()) {
            Phase::Travel
        } else if t < at(enter, self.enter.settle()) {
            Phase::Settle
        } else {
            Phase::Hold
        }
    }
}

fn seconds_to_micros(seconds: f32) -> u64 {
    (seconds * 1e6).ceil() as u64
}

/// One of the parts of an element that move in turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Part {
    pub index: usize,
    pub count: usize,
}

impl Part {
    /// An element that moves as one.
    pub const WHOLE: Self = Self { index: 0, count: 1 };

    pub fn new(index: usize, count: usize) -> Self {
        Self {
            index: index.min(count.saturating_sub(1)),
            count: count.max(1),
        }
    }

    fn last(self) -> u64 {
        self.count as u64 - 1
    }
}

/// Where a part is in its move: `enter` from 0 to 1, past 1 while it overshoots, and `exit`
/// from 0 to 1 as it leaves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub enter: f32,
    pub exit: f32,
}

impl Pose {
    pub const GONE: Self = Self {
        enter: 0.0,
        exit: 1.0,
    };
    pub const HELD: Self = Self {
        enter: 1.0,
        exit: 0.0,
    };

    /// How much of it shows, from 0 to 1, for opacity.
    pub fn presence(self) -> f32 {
        (self.enter.clamp(0.0, 1.0) * (1.0 - self.exit)).clamp(0.0, 1.0)
    }

    /// How far it is from its resting place along its way in, 1 before it starts and 0 at rest,
    /// negative while it overshoots. Exits travel back along the same way.
    pub fn offset(self) -> f32 {
        1.0 - self.enter + self.exit
    }

    pub fn visible(self) -> bool {
        self.presence() > 0.0
    }
}

/// The phase of a part's move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Before its range or after it.
    Off,
    /// In its range, waiting for its turn in the stagger.
    Waiting,
    /// On its way in, until it first reaches its place.
    Travel,
    /// Bouncing around its place until it rests.
    Settle,
    Hold,
    Exit,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(from: u64, to: u64) -> TimeRange {
        TimeRange::new(Micros(from), Micros(to)).unwrap()
    }

    #[test]
    fn springs_start_at_rest_overshoot_by_their_bounce_and_settle_exactly() {
        let stiff = Spring::new(0.4, 1.0);
        let bouncy = Spring::new(0.4, 0.4);
        assert_eq!(stiff.value(0.0), 0.0);
        // Starting at rest: the first millisecond barely moves.
        assert!(stiff.value(0.001) < 1e-3);
        let peak = |s: Spring| {
            (0..1000)
                .map(|i| s.value(i as f32 / 1000.0))
                .fold(0.0, f32::max)
        };
        assert!(peak(stiff) <= 1.0);
        // Damping ratio 0.4 overshoots by e^(-pi 0.4 / sqrt(1 - 0.16)), 25%.
        assert!((peak(bouncy) - 1.254).abs() < 0.005, "{}", peak(bouncy));
        for s in [stiff, bouncy] {
            assert_eq!(s.value(s.settle()), 1.0);
            let before = s.value(s.settle() - 0.001);
            assert!((before - 1.0).abs() <= SETTLED * 1.5, "{before}");
        }
        assert!(bouncy.impact() < bouncy.settle());
        let hinted = bouncy.settling_in(Micros(250_000));
        assert!((hinted.settle() - 0.25).abs() < 1e-4);
    }

    #[test]
    fn feel_controls_map_to_the_documented_springs() {
        let motion = |tempo, bounce, density, stagger| {
            Motion::new(Feel {
                tempo,
                bounce,
                density,
                stagger,
            })
        };
        let slow = motion(0, 0, 0, 0);
        let fast = motion(100, 100, 100, 100);
        let close = |a: Spring, b: Spring| {
            assert!((a.omega - b.omega).abs() < 1e-3 && (a.zeta - b.zeta).abs() < 1e-6);
        };
        close(slow.enter, Spring::new(0.9, 1.0));
        close(motion(50, 0, 0, 0).enter, Spring::new(0.45, 1.0));
        close(fast.enter, Spring::new(0.225, 0.4));
        close(fast.exit, Spring::new(0.225 * 0.6, 1.0));
        assert_eq!((slow.stagger, fast.stagger), (Micros(0), Micros(120_000)));
        assert!((slow.travel - 0.3).abs() < 1e-6 && (fast.travel - 1.2).abs() < 1e-6);
        assert_eq!("launch".parse(), Ok(Preset::Launch));
        assert!("sparkly".parse::<Preset>().is_err());
    }

    #[test]
    fn parts_stagger_in_and_exit_faster_by_the_end_of_their_range() {
        let motion = Preset::Editorial.motion();
        let on = range(1_000_000, 4_000_000);
        let parts = [Part::new(0, 3), Part::new(1, 3), Part::new(2, 3)];
        let at = |part, t| motion.pose(on, part, Micros(t));
        assert_eq!(at(parts[0], 999_999), Pose::GONE);
        assert_eq!(at(parts[0], 1_000_000).enter, 0.0);
        // Each part starts one stagger after the one before it.
        let early = at(parts[0], 1_100_000).enter;
        assert!(early > at(parts[1], 1_100_000).enter);
        assert_eq!(at(parts[1], 1_000_000 + motion.stagger.0).enter, 0.0);
        for part in parts {
            assert_eq!(at(part, 2_500_000), Pose::HELD);
            assert!(at(part, 3_999_999).presence() < 1e-3);
            assert_eq!(motion.phase(on, part, Micros(4_000_000)), Phase::Off);
        }
        assert!(motion.exit.settle() < motion.enter.settle());
        assert_eq!(motion.phase(on, parts[0], Micros(1_050_000)), Phase::Travel);
        assert_eq!(
            motion.phase(on, parts[2], Micros(1_050_000)),
            Phase::Waiting
        );
        assert_eq!(motion.phase(on, parts[0], Micros(2_500_000)), Phase::Hold);
        assert_eq!(motion.phase(on, parts[2], Micros(3_950_000)), Phase::Exit);
    }
}
