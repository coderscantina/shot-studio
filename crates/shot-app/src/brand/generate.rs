//! The theme generator and the contrast checks: from one accent color, or the accent found in
//! the logo, four distinct looks (palette and fonts) to pick from and then fine-tune.

use shot_core::{BundledFont, Color, Colors, Font, Fonts};

/// The colors a brand draws in when it names none, as `shot-render`'s default palette.
pub const DEFAULTS: [Color; 4] = [
    Color([0xc8, 0xf2, 0x5a]),
    Color([0xf0, 0xf3, 0xf6]),
    Color([0x16, 0x18, 0x1d]),
    Color([0x0f, 0x11, 0x15]),
];

/// The four colors, each with the default filled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub accent: Color,
    pub text: Color,
    pub card: Color,
    pub background: Color,
}

impl Palette {
    pub fn of(colors: &Colors) -> Self {
        Self {
            accent: colors.accent.unwrap_or(DEFAULTS[0]),
            text: colors.text.unwrap_or(DEFAULTS[1]),
            card: colors.card.unwrap_or(DEFAULTS[2]),
            background: colors.background.unwrap_or(DEFAULTS[3]),
        }
    }

    fn colors(self) -> Colors {
        Colors {
            accent: Some(self.accent),
            text: Some(self.text),
            card: Some(self.card),
            background: Some(self.background),
        }
    }
}

/// A look the generator proposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub name: &'static str,
    pub colors: Colors,
    pub fonts: Fonts,
}

/// Where text sits on a color that is too weak for it, by the color it sits on.
#[derive(Debug, Clone, PartialEq)]
pub struct Warning {
    /// `accent`, `text`, `card` or `background`: the row the warning shows on.
    pub on: &'static str,
    pub message: String,
}

/// WCAG relative luminance.
fn luminance(Color([r, g, b]): Color) -> f32 {
    let channel = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

/// The WCAG contrast ratio of two colors, 1 to 21.
pub fn contrast(a: Color, b: Color) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// What the templates draw on each color, checked against WCAG AA: body text needs 4.5:1, the
/// accent's labels and rules 3:1.
pub fn warnings(colors: &Colors) -> Vec<Warning> {
    let p = Palette::of(colors);
    let checks = [
        ("card", p.text, p.card, 4.5, "Text on cards"),
        (
            "background",
            p.text,
            p.background,
            4.5,
            "Text on the end card and title",
        ),
        (
            "accent",
            p.background,
            p.accent,
            4.5,
            "Words on accent pills",
        ),
        ("accent", p.accent, p.card, 3.0, "Accent labels on cards"),
    ];
    checks
        .into_iter()
        .filter_map(|(on, fg, bg, min, what)| {
            let ratio = contrast(fg, bg);
            (ratio < min).then(|| Warning {
                on,
                message: format!("{what} read at {ratio:.1}:1, below {min}:1"),
            })
        })
        .collect()
}

/// Hue in degrees, saturation and lightness 0 to 1.
fn hsl(Color([r, g, b]): Color) -> (f32, f32, f32) {
    let [r, g, b] = [r, g, b].map(|c| f32::from(c) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s, l)
}

fn from_hsl(h: f32, s: f32, l: f32) -> Color {
    let (s, l) = (s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    Color([r, g, b].map(|v| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8))
}

/// `color` lightened (or darkened) in steps until it reaches `min` contrast on `on`.
fn readable(color: Color, on: Color, min: f32) -> Color {
    let (h, s, l) = hsl(color);
    let up = luminance(on) < 0.2;
    (0..=50)
        .map(|step| {
            let shift = step as f32 * 0.02;
            from_hsl(h, s, if up { l + shift } else { l - shift })
        })
        .find(|c| contrast(*c, on) >= min)
        .unwrap_or(if up { Color([0xff; 3]) } else { Color([0; 3]) })
}

fn fonts(heading: BundledFont, body: BundledFont, mono: BundledFont) -> Fonts {
    Fonts {
        heading: Some(Font::Bundled(heading)),
        body: Some(Font::Bundled(body)),
        mono: Some(Font::Bundled(mono)),
    }
}

/// Four looks around `accent`: tinted night, neutral ink, light paper and a bold one in the
/// accent's own hue, each with a font pairing of the bundled fonts.
pub fn propose(accent: Color) -> Vec<Proposal> {
    let (h, s, _) = hsl(accent);
    let tint = (s * 0.25).min(0.18);
    let look = |name, background: Color, card: Color, text: Color, accent: Color, fonts| {
        // Labels on cards need 3:1, words in the background color on accent pills 4.5:1.
        let accent = readable(readable(accent, card, 3.0), background, 4.5);
        Proposal {
            name,
            colors: Palette {
                accent,
                text: readable(text, card, 7.0),
                card,
                background,
            }
            .colors(),
            fonts,
        }
    };
    use BundledFont::*;
    vec![
        look(
            "Night",
            from_hsl(h, tint, 0.06),
            from_hsl(h, tint, 0.10),
            from_hsl(h, 0.12, 0.95),
            accent,
            fonts(Geist, Geist, GeistMono),
        ),
        look(
            "Ink",
            from_hsl(220.0, 0.10, 0.07),
            from_hsl(220.0, 0.09, 0.12),
            Color([0xf4, 0xf6, 0xf8]),
            accent,
            fonts(Inter, Inter, JetbrainsMono),
        ),
        look(
            "Paper",
            from_hsl(h, tint, 0.94),
            from_hsl(h, tint * 0.5, 0.99),
            from_hsl(h, 0.25, 0.10),
            accent,
            fonts(Inter, Geist, GeistMono),
        ),
        look(
            "Bold",
            from_hsl(h, (s * 0.7).min(0.6), 0.13),
            from_hsl(h, (s * 0.5).min(0.45), 0.19),
            Color([0xff, 0xff, 0xff]),
            from_hsl(h + 30.0, s.max(0.7), 0.68),
            fonts(JetbrainsMono, Inter, JetbrainsMono),
        ),
    ]
}

/// The accent of an RGBA image (straight alpha): the most common saturated hue among its opaque
/// pixels, averaged. `None` for a logo without color.
pub fn accent_of(rgba: &[u8]) -> Option<Color> {
    const BINS: usize = 24;
    let mut weight = [0f32; BINS];
    let mut sums = [[0f32; 3]; BINS];
    for px in rgba.as_chunks::<4>().0 {
        if px[3] < 128 {
            continue;
        }
        let color = Color([px[0], px[1], px[2]]);
        let (h, s, l) = hsl(color);
        if s < 0.35 || !(0.2..=0.85).contains(&l) {
            continue;
        }
        let bin = (h / 360.0 * BINS as f32) as usize % BINS;
        weight[bin] += s;
        for (sum, c) in sums[bin].iter_mut().zip(px) {
            *sum += f32::from(*c) * s;
        }
    }
    let (bin, &total) = weight
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?;
    (total > 0.0).then(|| Color(sums[bin].map(|c| (c / total).round() as u8)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_follows_wcag() {
        assert!((contrast(Color([0; 3]), Color([0xff; 3])) - 21.0).abs() < 0.01);
        assert!((contrast(Color([0x77; 3]), Color([0xff; 3])) - 4.48).abs() < 0.02);
    }

    #[test]
    fn proposals_are_distinct_and_readable() {
        let proposals = propose(Color([0xff, 0x7a, 0x59]));
        assert_eq!(proposals.len(), 4);
        for (i, p) in proposals.iter().enumerate() {
            assert_eq!(warnings(&p.colors), vec![], "{}", p.name);
            let palette = Palette::of(&p.colors);
            assert!(contrast(palette.text, palette.card) >= 7.0, "{}", p.name);
            for q in &proposals[i + 1..] {
                assert_ne!(p.colors, q.colors);
            }
        }
    }

    #[test]
    fn the_default_palette_warns_about_nothing() {
        assert_eq!(warnings(&Colors::default()), vec![]);
        let weak = Colors {
            card: Some(Color([0xc0, 0xc0, 0xc0])),
            ..Colors::default()
        };
        assert_eq!(warnings(&weak)[0].on, "card");
    }

    #[test]
    fn a_logo_gives_its_most_common_hue() {
        let mut rgba = Vec::new();
        for _ in 0..30 {
            rgba.extend([0xe0, 0x40, 0x30, 0xff]);
        }
        for _ in 0..10 {
            rgba.extend([0x30, 0x60, 0xe0, 0xff]);
        }
        for _ in 0..50 {
            rgba.extend([0xff, 0xff, 0xff, 0xff]);
        }
        assert_eq!(accent_of(&rgba), Some(Color([0xe0, 0x40, 0x30])));
        assert_eq!(accent_of(&[0x80, 0x80, 0x80, 0xff]), None);
    }
}
