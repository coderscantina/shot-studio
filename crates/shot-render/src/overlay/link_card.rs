//! The `link-card` template: a card for a URL, sliding in at the bottom left from the word where
//! the URL is said.
//!
//! | Parameter | Value |
//! | --- | --- |
//! | `url` | the link, `https://...` |
//! | `accent` | optional, draws the card's stripe in the accent color instead of its kind's |
//!
//! The card's title and kind come from the URL alone, so it renders offline and the same every
//! time: a GitHub pull request, repository or issue, a post on X, a Hacker News item, an npm
//! package, or any other link by its host. [`cards`] places one where a URL is said, as
//! `shot_analyze::elements::urls` reads it, written (`github.com/owner/repo`) or spoken
//! (`github dot com slash owner slash repo`).

use shot_core::{Micros, Overlay, ParamValue, TimeRange, Word};

use super::{ParamKind, ParamSpec, Params, TemplateSpec, overlay};
use crate::drawing::{Font, Rgba, Shape};
use crate::layout::{Canvas, Rect};

pub const TEMPLATE: &str = "link-card";

pub const SPEC: TemplateSpec = TemplateSpec {
    name: TEMPLATE,
    about: "a card for a link said in the video",
    params: &[
        ParamSpec::required("url", ParamKind::Text, "the link"),
        ParamSpec::optional("accent", ParamKind::Bool, "stripe in the accent color"),
    ],
    animated: true,
};

/// How long a card stays from the word where its URL is said.
pub const HOLD: Micros = Micros(6_000_000);
/// The default `enter` and `exit`.
const SLIDE: Micros = Micros(350_000);

/// What a URL points at, for the card's label and stripe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    PullRequest {
        repo: String,
        number: u64,
    },
    Issue {
        repo: String,
        number: u64,
    },
    Repository {
        repo: String,
    },
    Post {
        user: String,
    },
    HackerNews {
        item: u64,
    },
    Npm {
        package: String,
    },
    /// Any other link, by its host.
    Web {
        host: String,
    },
}

/// A parsed `link-card` overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkCard {
    pub url: String,
    pub kind: LinkKind,
    pub accent: bool,
}

impl LinkCard {
    /// The card of a `link-card` overlay, `None` for another template or a URL that is not a
    /// link.
    pub fn of(overlay: &Overlay) -> Option<Self> {
        if overlay.template.as_str() != TEMPLATE {
            return None;
        }
        let params = Params(overlay);
        let url = params.text("url")?.to_owned();
        Some(Self {
            kind: kind(&url)?,
            accent: params.bool("accent").unwrap_or(false),
            url,
        })
    }

    /// The label above the title and the title.
    pub fn text(&self) -> (String, String) {
        match &self.kind {
            LinkKind::PullRequest { repo, number } => {
                ("GitHub pull request".into(), format!("{repo} #{number}"))
            }
            LinkKind::Issue { repo, number } => {
                ("GitHub issue".into(), format!("{repo} #{number}"))
            }
            LinkKind::Repository { repo } => ("GitHub repository".into(), repo.clone()),
            LinkKind::Post { user } => ("Post on X".into(), format!("@{user}")),
            LinkKind::HackerNews { item } => ("Hacker News".into(), format!("item {item}")),
            LinkKind::Npm { package } => ("npm package".into(), package.clone()),
            LinkKind::Web { host } => ("Link".into(), host.clone()),
        }
    }

    /// A short mark for the badge and its color.
    fn badge(&self) -> (&'static str, Rgba) {
        match self.kind {
            LinkKind::PullRequest { .. } | LinkKind::Issue { .. } | LinkKind::Repository { .. } => {
                ("GH", Rgba::hex(0xF0F3F6, 1.0))
            }
            LinkKind::Post { .. } => ("X", Rgba::hex(0xF0F3F6, 1.0)),
            LinkKind::HackerNews { .. } => ("Y", Rgba::hex(0xFF6600, 1.0)),
            LinkKind::Npm { .. } => ("npm", Rgba::hex(0xCB3837, 1.0)),
            LinkKind::Web { .. } => ("www", Rgba::hex(0x9AA4B2, 1.0)),
        }
    }

    /// The card at `t`: slides in from the left edge over `enter`, out over `exit`.
    pub fn draw(&self, params: Params, t: Micros, canvas: Canvas) -> Vec<Shape> {
        let s = canvas.width as f32 / 3840.0;
        let presence = params.presence(SLIDE).at(t);
        if presence <= 0.0 {
            return Vec::new();
        }
        let (label, title) = self.text();
        let width = (1500.0 * s).min(canvas.width as f32 * 0.8);
        let height = 300.0 * s;
        let margin = 120.0 * s;
        let left = margin - (1.0 - presence) * (width + margin);
        let top = canvas.height as f32 * 0.9 - height;
        let card = Rect::new(left, top, left + width, top + height);
        let (mark, mark_color) = self.badge();
        let stripe = if self.accent {
            Rgba::hex(0xC8F25A, 1.0)
        } else {
            mark_color
        };
        let fade = |c: Rgba| c.faded(presence);
        let badge = Rect::new(
            left + 64.0 * s,
            top + 70.0 * s,
            left + 224.0 * s,
            top + 230.0 * s,
        );
        let text_left = badge.right + 56.0 * s;
        let shown_url = self.url.trim_start_matches("https://");
        vec![
            Shape::Shadow {
                rect: card,
                radius: 36.0 * s,
                blur: 40.0 * s,
                color: fade(Rgba::hex(0x000000, 0.5)),
            },
            Shape::Clip {
                rect: card,
                radius: 36.0 * s,
                shapes: vec![
                    Shape::Rect {
                        rect: card,
                        radius: 0.0,
                        color: fade(Rgba::hex(0x16181D, 0.96)),
                    },
                    Shape::Rect {
                        rect: Rect::new(left, top, left + 14.0 * s, top + height),
                        radius: 0.0,
                        color: fade(stripe),
                    },
                ],
            },
            Shape::Rect {
                rect: badge,
                radius: 28.0 * s,
                color: fade(Rgba::hex(0x24272E, 1.0)),
            },
            Shape::Label {
                center: [
                    (badge.left + badge.right) / 2.0,
                    (badge.top + badge.bottom) / 2.0,
                ],
                text: mark.to_owned(),
                size: 56.0 * s,
                color: fade(mark_color),
                background: Rgba::hex(0x000000, 0.0),
            },
            Shape::Text {
                origin: [text_left, top + 48.0 * s],
                text: label,
                size: 44.0 * s,
                font: Font::Sans,
                color: fade(Rgba::hex(0x9AA4B2, 1.0)),
            },
            Shape::Text {
                origin: [text_left, top + 108.0 * s],
                text: clip_chars(&title, 34),
                size: 72.0 * s,
                font: Font::Sans,
                color: fade(Rgba::hex(0xF0F3F6, 1.0)),
            },
            Shape::Text {
                origin: [text_left, top + 204.0 * s],
                text: clip_chars(shown_url, 52),
                size: 38.0 * s,
                font: Font::Mono,
                color: fade(Rgba::hex(0x6B7380, 1.0)),
            },
        ]
    }
}

fn clip_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// What `url` points at, `None` when it is not an http(s) link with a host.
pub fn kind(url: &str) -> Option<LinkKind> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    if !host.contains('.') || host.starts_with('.') || host.ends_with('.') {
        return None;
    }
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let number = |s: &str| s.parse::<u64>().ok();
    let kind = match (host, parts.as_slice()) {
        ("github.com", [owner, repo, "pull", n, ..]) if number(n).is_some() => {
            LinkKind::PullRequest {
                repo: format!("{owner}/{repo}"),
                number: number(n)?,
            }
        }
        ("github.com", [owner, repo, "issues", n, ..]) if number(n).is_some() => LinkKind::Issue {
            repo: format!("{owner}/{repo}"),
            number: number(n)?,
        },
        ("github.com", [owner, repo, ..]) => LinkKind::Repository {
            repo: format!("{owner}/{repo}"),
        },
        ("x.com" | "twitter.com", [user, "status", n, ..]) if number(n).is_some() => {
            LinkKind::Post {
                user: (*user).to_owned(),
            }
        }
        ("news.ycombinator.com", ["item"]) => LinkKind::HackerNews {
            item: query
                .split('&')
                .find_map(|kv| kv.strip_prefix("id="))
                .and_then(number)?,
        },
        ("npmjs.com", ["package", name @ ..]) if !name.is_empty() => LinkKind::Npm {
            package: name.join("/"),
        },
        _ => LinkKind::Web {
            host: host.to_owned(),
        },
    };
    Some(kind)
}

/// A `link-card` overlay for each URL said in `words` that no other card covers yet, from the
/// first word of the URL for [`HOLD`].
pub fn cards(words: &[Word], existing: &[Overlay]) -> Vec<Overlay> {
    shot_analyze::elements::urls(words)
        .into_iter()
        .filter(|said| kind(&said.url).is_some())
        .filter_map(|said| {
            let range = TimeRange::new(said.words.from(), Micros(said.words.from().0 + HOLD.0))?;
            let taken = existing
                .iter()
                .any(|o| o.template.as_str() == TEMPLATE && o.range.overlaps(range));
            (!taken).then(|| overlay(TEMPLATE, range, vec![("url", ParamValue::Text(said.url))]))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<Word> {
        text.split_whitespace()
            .enumerate()
            .map(|(i, w)| Word {
                text: w.to_owned(),
                range: TimeRange::new(
                    Micros(i as u64 * 400_000),
                    Micros(i as u64 * 400_000 + 300_000),
                )
                .unwrap(),
                confidence: 90,
            })
            .collect()
    }

    #[test]
    fn urls_read_as_their_kind() {
        let k = |url| kind(url).unwrap();
        assert_eq!(
            k("https://github.com/coderscantina/shot-studio/pull/1"),
            LinkKind::PullRequest {
                repo: "coderscantina/shot-studio".into(),
                number: 1
            }
        );
        assert_eq!(
            k("https://github.com/zed-industries/zed"),
            LinkKind::Repository {
                repo: "zed-industries/zed".into()
            }
        );
        assert_eq!(
            k("https://x.com/mwallner/status/1850000000000000000"),
            LinkKind::Post {
                user: "mwallner".into()
            }
        );
        assert_eq!(
            k("https://news.ycombinator.com/item?id=41234567"),
            LinkKind::HackerNews { item: 41234567 }
        );
        assert_eq!(
            k("https://www.npmjs.com/package/@vue/reactivity"),
            LinkKind::Npm {
                package: "@vue/reactivity".into()
            }
        );
        assert_eq!(
            k("https://kdl.dev/spec"),
            LinkKind::Web {
                host: "kdl.dev".into()
            }
        );
        assert_eq!(kind("not a link"), None);
    }

    #[test]
    fn a_card_starts_where_the_url_is_said_and_is_written_once() {
        let said = words("see news.ycombinator.com/item?id=4 now");
        let cards = cards(&said, &[]);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].range.from(), Micros(400_000));
        assert_eq!(cards[0].range.duration(), HOLD);
        assert!(super::super::check(&cards[0]).is_ok());
        assert!(super::cards(&said, &cards).is_empty());
    }

    #[test]
    fn the_card_shows_from_the_word_where_the_url_is_said() {
        use crate::frame::{Look, Shot, compose};
        use crate::layout::Sources;
        use shot_core::Layout;

        let said = words("my PR is at github.com/coderscantina/shot-studio/pull/1 today");
        let overlays = cards(&said, &[]);
        let shows_card = |t: u64| {
            let shot = Shot {
                layout: Layout::Face,
                zooms: &[],
                overlays: &overlays,
                time: Micros(t),
            };
            let sizes = Sources {
                screen: (3840, 2160),
                camera: (1920, 1080),
            };
            let frame = compose(&shot, &Look::default(), Canvas::UHD, sizes).unwrap();
            format!("{:?}", frame.drawing.over).contains("coderscantina/shot-studio #1")
        };
        // The URL is the 5th word, said from 1.6 s.
        assert!(!shows_card(1_599_999));
        assert!(shows_card(1_650_000));
        assert!(shows_card(1_600_000 + HOLD.0 - 100_000));
        assert!(!shows_card(1_600_000 + HOLD.0));
    }
}
