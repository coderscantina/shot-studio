//! Thumbnail stills: a [`Variant`] of `thumbnail.kdl` drawn with the vector pass on the same
//! renderer as every frame, so the studio's canvas and the exported `main.png` are one drawing.
//!
//! [`Painter::groups`] lays a variant out on the 1280x720 canvas as shapes: the backdrop, the
//! face (a cutout with its outline, or a framed square), the headline auto-fitted to the preset's
//! box with an outline or a shadow, and the stickers. [`Painter::render`] draws them at any size.
//!
//! ```ignore
//! let renderer = Renderer::new()?;
//! let mut painter = Painter::default();
//! let still = painter.render(&renderer, &variant, &theme, &sources, (1280, 720))?;
//! ```

use std::sync::{Arc, OnceLock};

use parley::{FontContext, LayoutContext};
use shot_core::thumbnail::{
    Backdrop, CANVAS, FaceStyle, HeadlineColor, HeadlineStyle, Sticker, StickerShape,
    ThumbnailPreset, Variant,
};
use vello::kurbo::Affine;
use vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};

use crate::RenderError;
use crate::drawing::{Font, Rgba, Shape};
use crate::graph::Renderer;
use crate::layout::Rect;
use crate::theme::Theme;

const WIDTH: f32 = CANVAS.0 as f32;
const HEIGHT: f32 = CANVAS.1 as f32;
/// The headline's weight: heavier than the brand's heading, thumbnails read at 168 px wide.
const HEADLINE_WEIGHT: u16 = 800;
/// Headline sizes the fit chooses from, in canvas pixels.
const HEADLINE_SIZE: (f32, f32) = (44.0, 230.0);
/// The outline around a cutout, in pixels of the cutout picture.
pub const OUTLINE: u32 = 10;

/// A straight-alpha RGBA8 picture.
#[derive(Clone, PartialEq)]
pub struct Picture {
    pub rgba: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({}x{})", self.width, self.height)
    }
}

impl Picture {
    fn image(&self) -> ImageData {
        ImageData {
            data: Blob::new(self.rgba.clone()),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: self.width,
            height: self.height,
        }
    }
}

/// A person cut out of a camera frame: the person on a transparent ground, cropped to them with
/// [`OUTLINE`] pixels to spare, and that outline as a white silhouette of the same size.
#[derive(Debug, Clone, PartialEq)]
pub struct Cutout {
    pub person: Picture,
    pub outline: Picture,
    /// Where the crop sits in the camera frame, in fractions of it.
    pub crop: Rect,
}

/// The pictures of a variant's frame. Any of them may be missing: a take without a screen, a
/// frame without a person, or no frame picked yet.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sources {
    pub screen: Option<Picture>,
    pub camera: Option<Picture>,
    pub cutout: Option<Cutout>,
}

/// Lays variants out and draws them. Holds the fonts the headline is measured with.
#[derive(Default)]
pub struct Painter {
    fonts: FontContext,
    layouts: LayoutContext<()>,
}

/// Shapes under one transform, in canvas pixels.
type Group = (Affine, Vec<Shape>);

impl Painter {
    /// `variant` drawn at `size`, as straight-alpha RGBA8.
    pub fn render(
        &mut self,
        renderer: &Renderer,
        variant: &Variant,
        theme: &Theme,
        sources: &Sources,
        size: (u32, u32),
    ) -> Result<Picture, RenderError> {
        let scale = Affine::scale_non_uniform(
            f64::from(size.0) / f64::from(WIDTH),
            f64::from(size.1) / f64::from(HEIGHT),
        );
        let groups: Vec<Group> = self
            .groups(variant, theme, sources)
            .into_iter()
            .map(|(transform, shapes)| (scale * transform, shapes))
            .collect();
        let rgba = renderer.draw_still(&groups, size)?;
        Ok(Picture {
            rgba: Arc::new(rgba),
            width: size.0,
            height: size.1,
        })
    }

    /// `variant` on the 1280x720 canvas, back to front.
    pub fn groups(&mut self, variant: &Variant, theme: &Theme, sources: &Sources) -> Vec<Group> {
        let palette = &theme.palette;
        let canvas = Rect::new(0.0, 0.0, WIDTH, HEIGHT);
        let mut back = vec![Shape::Rect {
            rect: canvas,
            radius: 0.0,
            color: opaque(palette.background),
        }];
        let backdrop = match variant.backdrop {
            Backdrop::Screen => sources.screen.as_ref(),
            Backdrop::Camera => sources.camera.as_ref(),
            Backdrop::Brand => None,
        };
        if let Some(picture) = backdrop {
            back.push(cover(picture, canvas, 0.0));
        }
        if variant.dim > 0 {
            back.push(Shape::Rect {
                rect: canvas,
                radius: 0.0,
                color: Rgba([0.0, 0.0, 0.0, f32::from(variant.dim) / 100.0]),
            });
        }
        let mut groups = vec![(Affine::IDENTITY, back)];
        groups.push((Affine::IDENTITY, face(variant, theme, sources)));
        if let Some(headline) = &variant.headline {
            let font = Font::Face(theme.fonts.heading.at(HEADLINE_WEIGHT));
            let area = headline_box(variant.preset);
            let measured: Vec<(f32, f32)> = headline
                .lines
                .iter()
                .map(|line| {
                    crate::vector::measure(&mut self.fonts, &mut self.layouts, line, 100.0, font)
                })
                .collect();
            let size = fit(&measured, area);
            let line_height = size * 1.05;
            let block = line_height * headline.lines.len() as f32;
            let mut top = area.top + (area.height() - block) / 2.0;
            let mut shapes = Vec::new();
            for (index, (line, (width, _))) in headline.lines.iter().zip(&measured).enumerate() {
                let color = match (headline.color, index) {
                    (HeadlineColor::Text, _) | (HeadlineColor::Both, 0) => palette.text,
                    (HeadlineColor::Accent, _) | (HeadlineColor::Both, _) => palette.accent,
                };
                let width = width * size / 100.0;
                let left = match align(variant.preset) {
                    Align::Left => area.left,
                    Align::Center => area.left + (area.width() - width) / 2.0,
                };
                // parley puts the glyphs below the line's top; the line box is size * 1.2 tall.
                let origin = [left, top - size * 0.08];
                shapes.extend(outlined(
                    line,
                    origin,
                    size,
                    font,
                    opaque(color),
                    headline.style,
                ));
                top += line_height;
            }
            groups.push((Affine::IDENTITY, shapes));
        }
        for sticker in &variant.stickers {
            groups.push(sticker_group(sticker, theme));
        }
        groups
    }
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    Center,
}

fn align(preset: ThumbnailPreset) -> Align {
    match preset {
        ThumbnailPreset::FaceRight | ThumbnailPreset::FaceLeft => Align::Left,
        ThumbnailPreset::BigFace | ThumbnailPreset::Screen => Align::Center,
    }
}

/// Where a preset puts the headline.
pub fn headline_box(preset: ThumbnailPreset) -> Rect {
    match preset {
        ThumbnailPreset::FaceRight => Rect::new(56.0, 64.0, 700.0, 656.0),
        ThumbnailPreset::FaceLeft => Rect::new(580.0, 64.0, 1224.0, 656.0),
        ThumbnailPreset::BigFace => Rect::new(56.0, 452.0, 1224.0, 688.0),
        ThumbnailPreset::Screen => Rect::new(56.0, 36.0, 1224.0, 300.0),
    }
}

/// The largest size, between the bounds of [`HEADLINE_SIZE`], at which every line fits `area`
/// across and all of them fit it down. `measured` is each line's width and height at 100 px.
pub fn fit(measured: &[(f32, f32)], area: Rect) -> f32 {
    let widest = measured.iter().map(|(w, _)| *w).fold(1.0, f32::max);
    let across = area.width() / widest * 100.0;
    let down = area.height() / (1.05 * measured.len().max(1) as f32);
    across.min(down).clamp(HEADLINE_SIZE.0, HEADLINE_SIZE.1)
}

/// A line of text set off by an outline (the text drawn around it in black) or a soft shadow.
fn outlined(
    text: &str,
    origin: [f32; 2],
    size: f32,
    font: Font,
    color: Rgba,
    style: HeadlineStyle,
) -> Vec<Shape> {
    let at = |dx: f32, dy: f32, color| Shape::Text {
        origin: [origin[0] + dx, origin[1] + dy],
        text: text.to_owned(),
        size,
        font,
        color,
    };
    let mut shapes = Vec::new();
    match style {
        HeadlineStyle::Outline => {
            let width = (size * 0.075).max(4.0);
            for step in 0..16 {
                let angle = step as f32 / 16.0 * std::f32::consts::TAU;
                shapes.push(at(
                    angle.cos() * width,
                    angle.sin() * width,
                    Rgba([0.0, 0.0, 0.0, 1.0]),
                ));
            }
        }
        HeadlineStyle::Shadow => {
            for (offset, alpha) in [(0.09, 0.22), (0.065, 0.3), (0.04, 0.45)] {
                shapes.push(at(
                    size * offset * 0.6,
                    size * offset,
                    Rgba([0.0, 0.0, 0.0, alpha]),
                ));
            }
        }
    }
    shapes.push(at(0.0, 0.0, color));
    shapes
}

/// The face of a variant: a cutout standing on the bottom edge, or a framed square.
fn face(variant: &Variant, theme: &Theme, sources: &Sources) -> Vec<Shape> {
    let scale = f32::from(variant.face_size) / 100.0;
    let side = match variant.preset {
        ThumbnailPreset::FaceRight | ThumbnailPreset::BigFace | ThumbnailPreset::Screen => 1.0,
        ThumbnailPreset::FaceLeft => -1.0,
    };
    let white = opaque(theme.palette.text);
    match (variant.face, &sources.cutout, &sources.camera) {
        (FaceStyle::Off, ..) => Vec::new(),
        (FaceStyle::Cutout, Some(cutout), _) => {
            let (height, center) = match variant.preset {
                ThumbnailPreset::FaceRight | ThumbnailPreset::FaceLeft => (HEIGHT * 0.98, 0.74),
                ThumbnailPreset::BigFace | ThumbnailPreset::Screen => (HEIGHT * 0.62, 0.82),
            };
            let height = height * scale;
            let width = height * cutout.person.width as f32 / cutout.person.height as f32;
            let x = WIDTH / 2.0 + side * (center - 0.5) * WIDTH;
            let rect = Rect::new(x - width / 2.0, HEIGHT - height, x + width / 2.0, HEIGHT);
            vec![
                tinted(&cutout.outline, rect),
                image(&cutout.person, rect, Rect::UNIT, 0.0),
            ]
        }
        // Without a person in the frame a cutout shows framed, so the face is never lost.
        (FaceStyle::Cutout | FaceStyle::Framed, _, Some(camera)) => {
            let height = match variant.preset {
                ThumbnailPreset::FaceRight | ThumbnailPreset::FaceLeft => HEIGHT * 0.66,
                ThumbnailPreset::BigFace | ThumbnailPreset::Screen => HEIGHT * 0.42,
            } * scale;
            let (cx, cy) = match variant.preset {
                ThumbnailPreset::FaceRight | ThumbnailPreset::FaceLeft => {
                    (WIDTH / 2.0 + side * 0.24 * WIDTH, HEIGHT / 2.0)
                }
                ThumbnailPreset::BigFace | ThumbnailPreset::Screen => {
                    (WIDTH - 40.0 - height / 2.0, HEIGHT - 40.0 - height / 2.0)
                }
            };
            let rect = Rect::new(
                cx - height / 2.0,
                cy - height / 2.0,
                cx + height / 2.0,
                cy + height / 2.0,
            );
            let radius = height * 0.12;
            let focus = sources.cutout.as_ref().map(|c| c.crop);
            vec![
                Shape::Shadow {
                    rect,
                    radius,
                    blur: 40.0,
                    color: Rgba([0.0, 0.0, 0.0, 0.55]),
                },
                square(camera, rect, radius, focus),
                Shape::Outline {
                    rect,
                    radius,
                    width: 8.0,
                    color: white,
                },
            ]
        }
        (FaceStyle::Cutout | FaceStyle::Framed, _, None) => Vec::new(),
    }
}

fn sticker_group(sticker: &Sticker, theme: &Theme) -> Group {
    let (x, y, size) = (
        f32::from(sticker.x),
        f32::from(sticker.y),
        f32::from(sticker.size),
    );
    let turn = Affine::translate((f64::from(x), f64::from(y)))
        * Affine::rotate(f64::from(sticker.turn).to_radians())
        * Affine::translate((-f64::from(x), -f64::from(y)));
    let accent = opaque(theme.palette.accent);
    let shapes = match sticker.shape {
        StickerShape::Ring => {
            let width = (size * 0.075).max(6.0);
            let radius = size / 2.0 - width / 2.0;
            vec![
                Shape::Ring {
                    center: [x, y],
                    radius,
                    width: width + 10.0,
                    color: Rgba([0.0, 0.0, 0.0, 0.7]),
                },
                Shape::Ring {
                    center: [x, y],
                    radius,
                    width,
                    color: accent,
                },
            ]
        }
        StickerShape::Arrow => {
            // The arrow points right; its height is `size`, its length twice that.
            let rect = Rect::new(x - size, y - size / 2.0, x + size, y + size / 2.0);
            vec![image(&arrow(theme.palette.accent), rect, Rect::UNIT, 0.0)]
        }
        StickerShape::Emoji => sticker
            .emoji
            .as_deref()
            .and_then(crate::overlay::pop::animated)
            .map(|lottie| {
                let (w, h) = lottie.size();
                let scale = size / h as f32;
                vec![Shape::Lottie {
                    origin: [x - w as f32 * scale / 2.0, y - size / 2.0],
                    scale,
                    elapsed: shot_core::Micros(0),
                    opacity: 1.0,
                    font: Font::Sans,
                    lottie,
                }]
            })
            .unwrap_or_default(),
    };
    (turn, shapes)
}

/// The bold arrow sticker in `color` with a dark outline, drawn once per color.
fn arrow(color: Rgba) -> Picture {
    type Drawn = std::sync::Mutex<Vec<([u8; 3], Picture)>>;
    static DRAWN: OnceLock<Drawn> = OnceLock::new();
    let key = color.0.map(|c| (c * 255.0).round() as u8);
    let key = [key[0], key[1], key[2]];
    let mut drawn = DRAWN.get_or_init(Default::default).lock().expect("arrows");
    if let Some((_, picture)) = drawn.iter().find(|(k, _)| *k == key) {
        return picture.clone();
    }
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="200" viewBox="0 0 400 200">
<path d="M16 76 H252 V22 L384 100 L252 178 V124 H16 Z" fill="#{:02x}{:02x}{:02x}"
 stroke="#000000" stroke-width="14" stroke-linejoin="round"/></svg>"##,
        key[0], key[1], key[2]
    );
    let tree = resvg::usvg::Tree::from_data(svg.as_bytes(), &resvg::usvg::Options::default())
        .expect("the arrow is valid SVG");
    let mut pixmap = resvg::tiny_skia::Pixmap::new(400, 200).expect("has a size");
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    let mut data = pixmap.take();
    // tiny-skia holds premultiplied alpha; the drawing takes straight alpha.
    for pixel in data.as_chunks_mut::<4>().0 {
        let alpha = u16::from(pixel[3]);
        if alpha > 0 && alpha < 255 {
            for c in &mut pixel[..3] {
                *c = (u16::from(*c) * 255 / alpha).min(255) as u8;
            }
        }
    }
    let picture = Picture {
        rgba: Arc::new(data),
        width: 400,
        height: 200,
    };
    drawn.push((key, picture.clone()));
    picture
}

fn opaque(color: Rgba) -> Rgba {
    let [r, g, b, _] = color.0;
    Rgba([r, g, b, 1.0])
}

fn image(picture: &Picture, rect: Rect, crop: Rect, radius: f32) -> Shape {
    Shape::Image {
        rect,
        radius,
        image: picture.image(),
        crop,
        opacity: 1.0,
    }
}

/// The outline silhouette, white as drawn: it is the text color of the brand's light themes and
/// reads on every backdrop.
fn tinted(outline: &Picture, rect: Rect) -> Shape {
    image(outline, rect, Rect::UNIT, 0.0)
}

/// `picture` covering `rect`, cropped around its center.
fn cover(picture: &Picture, rect: Rect, radius: f32) -> Shape {
    let source = picture.width as f32 / picture.height as f32;
    let target = rect.width() / rect.height();
    let crop = if source > target {
        let w = target / source;
        Rect::new((1.0 - w) / 2.0, 0.0, (1.0 + w) / 2.0, 1.0)
    } else {
        let h = source / target;
        Rect::new(0.0, (1.0 - h) / 2.0, 1.0, (1.0 + h) / 2.0)
    };
    image(picture, rect, crop, radius)
}

/// A square of `picture` in `rect`, centered on `focus` (the person) when known.
fn square(picture: &Picture, rect: Rect, radius: f32, focus: Option<Rect>) -> Shape {
    let aspect = picture.width as f32 / picture.height as f32;
    // A square of the frame's full height, as a fraction of its width and height.
    let (w, h) = if aspect >= 1.0 {
        (1.0 / aspect, 1.0)
    } else {
        (1.0, aspect)
    };
    let center = focus.map_or(0.5, |f| (f.left + f.right) / 2.0);
    let left = (center - w / 2.0).clamp(0.0, 1.0 - w);
    let top = (1.0 - h) / 2.0;
    image(
        picture,
        rect,
        Rect::new(left, top, left + w, top + h),
        radius,
    )
}

/// The outline silhouette of a cutout: its alpha grown by `radius` pixels, in white.
pub fn silhouette(person: &Picture, radius: u32) -> Picture {
    let (w, h) = (person.width as usize, person.height as usize);
    let alpha: Vec<u8> = person
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p[3])
        .collect();
    let r = radius as usize;
    // A square grown by a separable max filter: rows, then columns.
    let mut rows = vec![0u8; w * h];
    for y in 0..h {
        let line = &alpha[y * w..(y + 1) * w];
        for x in 0..w {
            let (from, to) = (x.saturating_sub(r), (x + r + 1).min(w));
            rows[y * w + x] = line[from..to].iter().copied().max().unwrap_or(0);
        }
    }
    let mut grown = vec![0u8; w * h];
    for x in 0..w {
        for y in 0..h {
            let (from, to) = (y.saturating_sub(r), (y + r + 1).min(h));
            grown[y * w + x] = (from..to).map(|yy| rows[yy * w + x]).max().unwrap_or(0);
        }
    }
    let rgba = grown.iter().flat_map(|&a| [255, 255, 255, a]).collect();
    Picture {
        rgba: Arc::new(rgba),
        width: person.width,
        height: person.height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::Id;
    use shot_core::thumbnail::Headline;

    fn picture(width: u32, height: u32, rgba: [u8; 4]) -> Picture {
        Picture {
            rgba: Arc::new(rgba.repeat((width * height) as usize)),
            width,
            height,
        }
    }

    #[test]
    fn the_headline_takes_the_largest_size_that_fits_its_box() {
        let area = Rect::new(0.0, 0.0, 600.0, 400.0);
        // One line 300 px wide at 100 px fits across at 200 px.
        assert_eq!(fit(&[(300.0, 120.0)], area), 200.0);
        // Two short lines fit across at more, so the height decides.
        let two = fit(&[(100.0, 120.0), (120.0, 120.0)], area);
        assert!((two - 400.0 / 2.1).abs() < 0.01);
        // A very long line still reads: it never drops under the smallest size.
        assert_eq!(fit(&[(5000.0, 120.0)], area), HEADLINE_SIZE.0);
    }

    #[test]
    fn a_silhouette_grows_the_person_by_the_outline() {
        let mut rgba = vec![0u8; 9 * 9 * 4];
        rgba[(4 * 9 + 4) * 4 + 3] = 255;
        let person = Picture {
            rgba: Arc::new(rgba),
            width: 9,
            height: 9,
        };
        let grown = silhouette(&person, 2);
        let alpha = |x: usize, y: usize| grown.rgba[(y * 9 + x) * 4 + 3];
        assert_eq!((alpha(2, 2), alpha(6, 6), alpha(4, 4)), (255, 255, 255));
        assert_eq!((alpha(1, 4), alpha(4, 7)), (0, 0));
    }

    #[test]
    fn a_variant_renders_to_pixels_at_any_size() {
        let theme = Theme::default();
        let mut variant = Variant::new(Id::new("a").unwrap(), ThumbnailPreset::BigFace);
        variant.stickers.push(Sticker {
            shape: StickerShape::Arrow,
            emoji: None,
            x: 640,
            y: 360,
            size: 200,
            turn: 90,
        });
        let renderer = Renderer::new().unwrap();
        let still = Painter::default()
            .render(&renderer, &variant, &theme, &Sources::default(), (320, 180))
            .unwrap();
        assert_eq!(still.rgba.len(), 320 * 180 * 4);
        let pixel = |x: usize, y: usize| &still.rgba[(y * 320 + x) * 4..(y * 320 + x) * 4 + 4];
        let [r, g, _, _] = theme
            .palette
            .background
            .0
            .map(|c| (c * 255.0).round() as u8);
        // The corner is the brand background dimmed by 10 percent; the center is the arrow,
        // turned to point down, in the accent.
        let corner = pixel(2, 2);
        assert!(
            corner[0].abs_diff((f32::from(r) * 0.9) as u8) <= 2,
            "{corner:?}"
        );
        assert!(corner[1].abs_diff((f32::from(g) * 0.9) as u8) <= 2 && corner[3] == 255);
        let accent = theme
            .palette
            .accent
            .0
            .map(|c| (c * 255.0 * 0.9).round() as u8);
        let center = pixel(160, 90);
        assert!(
            center[1].abs_diff(accent[1]) <= 30,
            "{center:?} against {accent:?}"
        );
    }

    #[test]
    fn a_variant_draws_back_to_front_and_falls_back_without_its_pictures() {
        let theme = Theme::default();
        let mut variant = Variant::new(Id::new("a").unwrap(), ThumbnailPreset::FaceRight);
        variant.headline = Some(Headline {
            lines: vec!["Ship on Friday".into()],
            color: HeadlineColor::Accent,
            style: HeadlineStyle::Outline,
        });
        variant.stickers.push(Sticker {
            shape: StickerShape::Ring,
            emoji: None,
            x: 300,
            y: 300,
            size: 200,
            turn: 0,
        });
        let mut painter = Painter::default();
        // No frame: the brand background, dimmed, no face, the headline and the ring.
        let bare = painter.groups(&variant, &theme, &Sources::default());
        assert_eq!(bare.len(), 4);
        assert!(bare[1].1.is_empty(), "no face without a camera");
        assert_eq!(bare[2].1.len(), 17, "16 outline passes and the fill");
        let person = picture(40, 80, [200, 150, 120, 255]);
        let sources = Sources {
            screen: Some(picture(16, 9, [20, 20, 20, 255])),
            camera: Some(picture(16, 9, [90, 90, 90, 255])),
            cutout: Some(Cutout {
                outline: silhouette(&person, 4),
                person,
                crop: Rect::new(0.4, 0.1, 0.6, 1.0),
            }),
        };
        let full = painter.groups(&variant, &theme, &sources);
        assert_eq!(full[0].1.len(), 3, "brand ground, screen, dim");
        let [Shape::Image { rect, .. }, Shape::Image { .. }] = full[1].1.as_slice() else {
            panic!("the cutout and its outline: {:?}", full[1].1.len());
        };
        assert_eq!(rect.bottom, HEIGHT, "the cutout stands on the bottom edge");
        assert!(rect.left > WIDTH / 2.0, "face-right puts it on the right");
    }
}
