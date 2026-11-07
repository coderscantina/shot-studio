//! Design tokens from `docs/design.md`, the embedded fonts and icons, and gpui-component's
//! theme restyled with them. Dark only for now; tokens are named by role so a light theme can
//! swap the values without touching screens.

use std::borrow::Cow;

use gpui::{App, AssetSource, Pixels, Rgba, SharedString, px};
use gpui_component::{Theme, ThemeMode};

const fn hex(rgb: u32) -> Rgba {
    hexa(rgb, 1.0)
}

const fn hexa(rgb: u32, a: f32) -> Rgba {
    Rgba {
        r: ((rgb >> 16) & 0xff) as f32 / 255.0,
        g: ((rgb >> 8) & 0xff) as f32 / 255.0,
        b: (rgb & 0xff) as f32 / 255.0,
        a,
    }
}

/// The window behind every pane.
pub const FRAME: Rgba = hex(0x08090b);
/// Panes.
pub const SURFACE: Rgba = hex(0x111316);
/// Cards and the active item inside a pane.
pub const RAISED: Rgba = hex(0x181b20);
/// Cards inside a pane that are not selected, such as the Record screen's tracks.
pub const CARD: Rgba = hex(0x15171b);
/// Popovers, toolbars and the active segment.
pub const FLOAT: Rgba = hex(0x23272e);
/// Wells sunk into a pane: segmented controls, lanes, the preview stage.
pub const WELL: Rgba = hex(0x0b0c0f);
pub const TEXT: Rgba = hex(0xe9ecf1);
pub const STRONG: Rgba = hex(0xf5f7fa);
/// Transcript body text.
pub const BODY: Rgba = hex(0xc3c9d3);
pub const MUTED: Rgba = hex(0x8b93a0);
pub const FAINT: Rgba = hex(0x4a505a);
/// Actions and data.
pub const ACCENT: Rgba = hex(0xc8f25a);
pub const ON_ACCENT: Rgba = hex(0x0a0b0d);
/// Cuts.
pub const CUT: Rgba = hex(0xff6b63);
pub const CUT_TEXT: Rgba = hex(0xff9a94);
pub const CUT_TINT: Rgba = hexa(0xff6b63, 0.12);
/// Recording.
pub const RECORD: Rgba = hex(0xff5a52);
pub const RECORD_TEXT: Rgba = hex(0xff7a72);
/// The zoom lane.
pub const ZOOM: Rgba = hex(0x5aa9ff);
pub const ZOOM_TEXT: Rgba = hex(0xa9d0ff);
pub const ZOOM_TINT: Rgba = hexa(0x5aa9ff, 0.3);
/// Keyboard hints.
pub const KBD: Rgba = hex(0x262a31);
pub const KBD_TEXT: Rgba = hex(0xa3abb8);
/// The hovered or selected item on a float surface.
pub const FLOAT_ON: Rgba = hex(0x30353e);
/// Lane blocks and chapter blocks.
pub const BLOCK: Rgba = hex(0x2a303b);
pub const BLOCK_ON: Rgba = hex(0x3a404b);
pub const WAVE: Rgba = hex(0x3c4352);
pub const SELECTION: Rgba = hexa(0xc8f25a, 0.18);

pub const RADIUS_LG: Pixels = px(14.);
pub const RADIUS_MD: Pixels = px(10.);
pub const RADIUS_SM: Pixels = px(8.);

/// Body text. Geist Mono only for timecodes, durations, code and file names.
pub const SANS: &str = "Geist";
pub const MONO: &str = "Geist Mono";
pub const TEXT_BASE: Pixels = px(15.);
pub const TEXT_SM: Pixels = px(13.);
pub const TEXT_XS: Pixels = px(12.);

/// Icon size in buttons, nav and lists; inside tags it is 14.
pub const ICON: Pixels = px(18.);
pub const ICON_SM: Pixels = px(14.);

/// Embedded icons, `icons/<name>.svg` from the Hugeicons free set (MIT, see `assets/icons`).
pub struct Assets;

macro_rules! embedded {
    ($($name:literal),+ $(,)?) => {
        fn embedded(path: &str) -> Option<&'static [u8]> {
            match path {
                $(concat!("icons/", $name, ".svg") => {
                    Some(include_bytes!(concat!("../assets/icons/", $name, ".svg")))
                })+
                _ => None,
            }
        }
    };
}

embedded!(
    "alert",
    "camera",
    "check",
    "chevron-down",
    "cursor",
    "cut",
    "display",
    "edit",
    "export",
    "layout",
    "mic",
    "pause",
    "picture",
    "play",
    "record",
    "restore",
    "system-audio",
    "text",
    "timeline",
    "zoom",
);

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(embedded(path).map(Cow::Borrowed))
    }

    fn list(&self, _path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

/// An icon's asset path.
pub fn icon_path(name: &str) -> SharedString {
    format!("icons/{name}.svg").into()
}

/// Loads Geist and Geist Mono and restyles gpui-component with the tokens.
pub fn init(cx: &mut App) {
    let fonts: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(include_bytes!("../assets/fonts/Geist-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/Geist-Medium.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/Geist-SemiBold.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/Geist-Bold.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/GeistMono-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/GeistMono-Medium.ttf")),
    ];
    cx.text_system()
        .add_fonts(fonts)
        .expect("the embedded fonts load");

    gpui_component::init(cx);
    Theme::change(ThemeMode::Dark, None, cx);
    // `update` carries the color edits into the component tokens buttons read.
    Theme::update(cx, |theme| {
        theme.font_family = SANS.into();
        theme.font_size = TEXT_BASE;
        theme.mono_font_family = MONO.into();
        theme.radius = RADIUS_SM;
        theme.radius_lg = RADIUS_LG;
        theme.shadow = false;
        let colors = &mut theme.colors;
        colors.background = FRAME.into();
        colors.foreground = TEXT.into();
        colors.border = SURFACE.into();
        colors.muted = RAISED.into();
        colors.muted_foreground = MUTED.into();
        colors.popover = FLOAT.into();
        colors.popover_foreground = TEXT.into();
        colors.primary = ACCENT.into();
        colors.primary_hover = hex(0xd6f77e).into();
        colors.primary_active = hex(0xb4dc4c).into();
        colors.primary_foreground = ON_ACCENT.into();
        colors.secondary = RAISED.into();
        colors.secondary_hover = FLOAT.into();
        colors.secondary_active = FLOAT_ON.into();
        colors.secondary_foreground = TEXT.into();
        colors.button = RAISED.into();
        colors.button_hover = FLOAT.into();
        colors.button_active = FLOAT_ON.into();
        colors.button_foreground = TEXT.into();
        colors.button_primary = ACCENT.into();
        colors.button_primary_hover = hex(0xd6f77e).into();
        colors.button_primary_active = hex(0xb4dc4c).into();
        colors.button_primary_foreground = ON_ACCENT.into();
        colors.button_danger = RECORD.into();
        colors.button_danger_hover = hex(0xff7068).into();
        colors.button_danger_active = hex(0xe84a42).into();
        colors.button_danger_foreground = ON_ACCENT.into();
        colors.danger = RECORD.into();
        colors.danger_foreground = ON_ACCENT.into();
        colors.accent = FLOAT.into();
        colors.accent_foreground = STRONG.into();
        colors.ring = ACCENT.into();
        colors.progress_bar = ACCENT.into();
        colors.selection = SELECTION.into();
        colors.title_bar = FRAME.into();
        colors.title_bar_border = FRAME.into();
    });
}
