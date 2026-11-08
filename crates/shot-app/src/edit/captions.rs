//! The Captions panel, in the Picture workspace beside the stage and in the Text workspace in
//! place of the inspector: the caption lines of the chapter in time, editable in place, split
//! and joined, and the caption style with its look. While it is open the preview draws the
//! captions as a Short burns them in.
//!
//! A line's new text becomes a caption fix in `cut.kdl` for the words that changed, with a word
//! of context on each side so it does not change the same word elsewhere; a split or a join
//! becomes a line break there. The style is the video's `captions` node in `format.kdl`; Use
//! for every video moves the look into the channel's brand.

use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, Div, Entity, FontWeight, Hsla, ScrollHandle, SharedString, Stateful,
    Subscription, Window, div, prelude::*, px, rgb,
};
use gpui_component::input::{Escape, Input, InputEvent, InputState};
use shot_core::{
    CaptionBack, CaptionBreak, CaptionCase, CaptionDesign, CaptionFix, CaptionFont, CaptionLook,
    CaptionPosition, CaptionStyle, Color, Cut, Emphasis, Micros, Transcript,
};
use shot_render::captions::{Cue, Design, SourceWord, burned_lines};

use super::EditScreen;
use crate::engine::PreviewCaptions;
use crate::folder::VideoFolder;
use crate::model::{FPS, timecode};
use crate::theme::*;
use crate::ui::{self, Segment, icon, kbd, label};

/// A caption preset's name in the UI.
pub fn style_name(style: CaptionStyle) -> &'static str {
    match style {
        CaptionStyle::WordPop => "Word pop",
        CaptionStyle::Clean => "Clean",
        CaptionStyle::Karaoke => "Karaoke",
        CaptionStyle::Bold => "Bold",
        CaptionStyle::Off => "Off",
    }
}

/// What the panel shows and the line being edited.
#[derive(Default)]
pub struct CaptionsState {
    /// The panel is open, and the preview draws the captions.
    pub open: bool,
    /// The burned-in lines of the edit, read while the panel is open.
    lines: Vec<Vec<SourceWord>>,
    /// The line picked for its tools, by index into `lines`.
    picked: Option<usize>,
    editing: Option<Editing>,
    scroll: ScrollHandle,
    note: Option<String>,
}

struct Editing {
    line: usize,
    input: Entity<InputState>,
    _events: Subscription,
}

impl CaptionsState {
    /// A caption line's text field has the keys.
    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }
}

/// The effective style of the main cut: its own, else the format's.
fn style_of(folder: &VideoFolder) -> CaptionStyle {
    folder
        .project
        .cut
        .captions
        .unwrap_or(folder.project.format.captions)
}

/// The brand's caption look, from the lock of a migrated video.
fn brand_look(folder: &VideoFolder) -> CaptionLook {
    folder
        .lock()
        .map(|lock| lock.brand.captions.clone())
        .unwrap_or_default()
}

/// The design the edit's captions burn in with: the preset, the brand's look, the video's.
fn design_of(folder: &VideoFolder) -> CaptionDesign {
    brand_look(folder)
        .merge(&folder.project.format.caption_look)
        .design(style_of(folder))
}

impl EditScreen {
    /// Reads the caption lines while the panel is open and returns what the preview draws.
    pub(super) fn preview_captions(&mut self, cx: &App) -> Option<Arc<PreviewCaptions>> {
        if !self.captions.open {
            self.captions.lines.clear();
            return None;
        }
        let folder = self.folder.read(cx);
        let theme = folder.theme.as_ref().ok()?;
        let design = Design::new(style_of(folder), &folder.project.format.caption_look, theme);
        let cut = &folder.project.cut;
        self.captions.lines = burned_lines(
            &self.map.plan,
            &folder.transcripts,
            &cut.caption_fixes,
            &cut.caption_breaks,
            usize::from(design.design.words),
        );
        if self
            .captions
            .picked
            .is_some_and(|line| line >= self.captions.lines.len())
        {
            self.captions.picked = None;
        }
        let cues = self
            .captions
            .lines
            .iter()
            .map(|line| Cue {
                words: line.iter().map(|w| w.word.clone()).collect(),
            })
            .collect();
        Some(Arc::new(PreviewCaptions { design, cues }))
    }

    /// Opens or closes the panel. It opens in the Picture workspace unless Text is on show.
    pub(super) fn toggle_captions(&mut self, cx: &mut Context<Self>) {
        self.captions.open = !self.captions.open;
        self.captions.editing = None;
        self.captions.note = None;
        if self.captions.open && self.workspace != super::Workspace::Text {
            self.set_workspace(super::Workspace::Picture, cx);
        }
        self.rebuild(cx);
        cx.notify();
    }

    /// Changes the style and look of the video's captions in `format.kdl`. A new preset starts
    /// from the brand's look; the cut's own caption style gives way to it.
    fn set_design(&mut self, change: impl FnOnce(&mut CaptionDesign), cx: &mut Context<Self>) {
        let result = self.folder.update(cx, |folder, cx| {
            let before = design_of(folder);
            let mut design = before;
            change(&mut design);
            let mut format = folder.project.format.clone();
            if design.style != before.style {
                format.captions = design.style;
                format.caption_look = CaptionLook::default();
            } else {
                // Every field the person set, against the preset with the brand's look.
                let over_brand = brand_look(folder).design(design.style);
                format.caption_look = format
                    .caption_look
                    .merge(&CaptionLook::of(&design).diff(&CaptionLook::of(&over_brand)));
                set_changed(&mut format.caption_look, &before, &design);
            }
            let cut = folder.project.cut.clone();
            let result = folder.as_one_change(|folder| {
                folder.write_format(format)?;
                if cut.captions.is_some() {
                    folder.write_cut(Cut {
                        captions: None,
                        ..cut
                    })?;
                }
                Ok(())
            });
            cx.notify();
            result
        });
        self.wrote(result, cx);
    }

    /// Drops the video's look, back to the preset with the brand's.
    fn reset_look(&mut self, cx: &mut Context<Self>) {
        let result = self.folder.update(cx, |folder, cx| {
            let mut format = folder.project.format.clone();
            format.caption_look = CaptionLook::default();
            let result = folder.write_format(format);
            cx.notify();
            result
        });
        self.wrote(result, cx);
    }

    /// Moves the video's look into the channel's brand, for every video.
    fn look_for_channel(&mut self, cx: &mut Context<Self>) {
        let result = self.folder.update(cx, |folder, cx| {
            let look = folder.project.format.caption_look.clone();
            let result = folder.set_brand_captions(look);
            cx.notify();
            result
        });
        if result.is_ok() {
            self.captions.note = Some("Every video of the channel now uses this look".into());
        }
        self.wrote(result, cx);
    }

    fn pick_line(&mut self, line: usize, cx: &mut Context<Self>) {
        self.captions.picked = Some(line);
        if self
            .captions
            .editing
            .as_ref()
            .is_some_and(|e| e.line != line)
        {
            self.captions.editing = None;
        }
        let from = self
            .captions
            .lines
            .get(line)
            .and_then(|l| l.first())
            .map(|w| w.word.from);
        if let Some(from) = from {
            self.set_playing(false);
            self.seek(from.0 * u64::from(FPS) / 1_000_000);
        }
        cx.notify();
    }

    /// Edits the picked line's text in place. Enter keeps it, Esc leaves it.
    pub(super) fn edit_caption_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(line) = self.captions.picked else {
            return;
        };
        let Some(words) = self.captions.lines.get(line) else {
            return;
        };
        let text = line_text(words);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(text));
        let events = cx.subscribe_in(&input, window, |this, input, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let text = input.read(cx).value().to_string();
                this.commit_caption_line(&text, window, cx);
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.captions.editing = Some(Editing {
            line,
            input,
            _events: events,
        });
        cx.notify();
    }

    fn cancel_caption_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.captions.editing = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Writes the caption fix that turns the line being edited into `text`.
    fn commit_caption_line(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = self.captions.editing.take() else {
            return;
        };
        window.focus(&self.focus, cx);
        let Some(words) = self.captions.lines.get(editing.line).cloned() else {
            return;
        };
        let folder = self.folder.read(cx);
        let fix = words.first().and_then(|first| {
            let transcript = folder.transcripts.get(&first.take)?;
            fix_for(&words, transcript, text)
        });
        match fix {
            Some(Ok(fix)) => {
                self.captions.note = None;
                self.edit(cx, |cut| Ok(with_fix(cut, fix)));
            }
            Some(Err(note)) => {
                self.captions.note = Some(note.into());
                cx.notify();
            }
            None => cx.notify(),
        }
    }

    /// Starts a new line at word `word` of line `line`.
    fn split_line(&mut self, line: usize, word: usize, cx: &mut Context<Self>) {
        let Some(at) = self.captions.lines.get(line).and_then(|l| l.get(word)) else {
            return;
        };
        let (take, at) = (at.take.clone(), at.at);
        self.edit(cx, |cut| Ok(with_break(cut, take, at, true)));
    }

    /// Joins line `line` with the next one.
    fn join_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let Some(next) = self.captions.lines.get(line + 1).and_then(|l| l.first()) else {
            return;
        };
        let (take, at) = (next.take.clone(), next.at);
        self.edit(cx, |cut| Ok(with_break(cut, take, at, false)));
    }
}

fn line_text(words: &[SourceWord]) -> String {
    words
        .iter()
        .map(|w| w.word.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Sets in `look` every field `after` changed from `before`, so a change to the preset's own
/// value is kept against a brand that sets another.
fn set_changed(look: &mut CaptionLook, before: &CaptionDesign, after: &CaptionDesign) {
    fn set<T: PartialEq + Copy>(slot: &mut Option<T>, before: T, after: T) {
        if before != after {
            *slot = Some(after);
        }
    }
    set(&mut look.font, before.font, after.font);
    set(&mut look.size, before.size, after.size);
    set(&mut look.weight, before.weight, after.weight);
    set(&mut look.color, before.color, after.color);
    if before.highlight != after.highlight {
        look.highlight = after.highlight;
    }
    set(&mut look.back, before.back, after.back);
    set(&mut look.position, before.position, after.position);
    set(&mut look.nudge, before.nudge, after.nudge);
    set(&mut look.words, before.words, after.words);
    set(&mut look.case, before.case, after.case);
}

/// `cut` with `fix`, replacing a fix for the same heard words.
fn with_fix(cut: &Cut, fix: CaptionFix) -> Cut {
    let mut cut = cut.clone();
    let heard = fix.heard.to_lowercase();
    match cut
        .caption_fixes
        .iter_mut()
        .find(|f| f.heard.to_lowercase() == heard)
    {
        Some(existing) => existing.said = fix.said,
        None => cut.caption_fixes.push(fix),
    }
    cut
}

/// `cut` with a split or join at the word of `take` starting at `at`, replacing one there.
fn with_break(cut: &Cut, take: shot_core::Id, at: Micros, split: bool) -> Cut {
    let mut cut = cut.clone();
    cut.caption_breaks
        .retain(|b| !(b.take == take && b.at == at));
    cut.caption_breaks.push(CaptionBreak { take, at, split });
    cut.caption_breaks
        .sort_by(|a, b| (&a.take, a.at).cmp(&(&b.take, b.at)));
    cut
}

/// The fix that makes `line` read `text`: the transcript words under the changed words and a
/// word of context on each side, heard, become the context and the new words, said. `None` when
/// nothing changed; an error when the change cannot be a fix.
fn fix_for(
    line: &[SourceWord],
    transcript: &Transcript,
    text: &str,
) -> Option<Result<CaptionFix, &'static str>> {
    let old: Vec<&str> = line.iter().map(|w| w.word.text.as_str()).collect();
    let new: Vec<&str> = text.split_whitespace().collect();
    let (start, old_end, new_end) = changed(&old, &new)?;
    if new.is_empty() {
        return Some(Err(
            "A line keeps at least one word. Cut the words in Text instead",
        ));
    }
    // A word of context on each side, from the same take.
    let take = &line[start.min(line.len() - 1)].take;
    let first = start.saturating_sub(1);
    let last = (old_end + 1).min(line.len()).max(first + 1);
    if line[first..last].iter().any(|w| &w.take != take) {
        return Some(Err("Edit the words of one take at a time"));
    }
    let from = line[first].at;
    let end = &line[last - 1];
    let to = Micros(end.at.0 + (end.word.to.0 - end.word.from.0).max(1));
    let heard: Vec<&str> = transcript
        .words()
        .iter()
        .filter(|w| from <= w.range.from() && w.range.from() < to)
        .map(|w| w.text.as_str())
        .collect();
    let Some(last_heard) = heard.last() else {
        return Some(Err("These words have no transcript to fix"));
    };
    let trailing: String = {
        let mut t: Vec<char> = last_heard
            .chars()
            .rev()
            .take_while(|c| !c.is_alphanumeric())
            .collect();
        t.reverse();
        t.into_iter().collect()
    };
    let before = old[first..start].iter();
    let after = old[old_end..last].iter();
    let mut said: Vec<String> = before
        .chain(&new[start..new_end])
        .chain(after)
        .map(|w| (*w).to_owned())
        .collect();
    // The fix adds the last heard word's punctuation back.
    if let Some(end) = said.last_mut()
        && !trailing.is_empty()
        && let Some(stripped) = end.strip_suffix(&trailing)
    {
        *end = stripped.to_owned();
    }
    let said = said.join(" ");
    if said.trim().is_empty() {
        return Some(Err(
            "A line keeps at least one word. Cut the words in Text instead",
        ));
    }
    Some(Ok(CaptionFix {
        heard: heard.join(" "),
        said,
    }))
}

/// The changed part of `old` against `new`: its start, its end in `old` and in `new`, from the
/// common prefix and suffix. `None` when they are the same.
fn changed(old: &[&str], new: &[&str]) -> Option<(usize, usize, usize)> {
    if old == new {
        return None;
    }
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    Some((prefix, old.len() - suffix, new.len() - suffix))
}

// The panel.

/// Text colors offered for the words.
const COLORS: [(u32, &str); 4] = [
    (0xFFFFFF, "White"),
    (0xF5F0E6, "Warm white"),
    (0xFFD84D, "Yellow"),
    (0x0A0B0D, "Black"),
];
/// Colors offered for the word being said, after the brand's accent.
const HIGHLIGHTS: [(u32, &str); 5] = [
    (0xC8F25A, "Lime"),
    (0xFFD84D, "Yellow"),
    (0x5AA9FF, "Blue"),
    (0xFF6BB5, "Pink"),
    (0xFF9B3D, "Orange"),
];

fn color(hex: u32) -> Color {
    let [_, r, g, b] = hex.to_be_bytes();
    Color([r, g, b])
}

fn hex(color: Color) -> u32 {
    let [r, g, b] = color.0;
    u32::from_be_bytes([0, r, g, b])
}

/// The panel, scrolling as a whole.
pub fn panel(this: &EditScreen, cx: &mut Context<EditScreen>) -> Stateful<Div> {
    let folder = this.folder.read(cx);
    let design = design_of(folder);
    let channel = folder.is_migrated() && this.channel.is_some();
    let custom = !folder.project.format.caption_look.is_empty();
    let accent = folder.theme.as_ref().map_or(0xC8F25A, |t| {
        let [r, g, b, _] = t.palette.accent.0;
        let byte = |v: f32| (v * 255.).round() as u8;
        u32::from_be_bytes([0, byte(r), byte(g), byte(b)])
    });
    let screen = cx.entity();
    let header = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(icon("text", ICON, TEXT))
        .child(div().font_weight(FontWeight::SEMIBOLD).child("Captions"))
        .child(div().flex_1())
        .child(
            div()
                .id("captions-close")
                .flex()
                .items_center()
                .justify_center()
                .size(px(26.))
                .rounded(RADIUS_SM)
                .cursor_pointer()
                .hover(|s| s.bg(RAISED))
                .child(icon("close", ICON_SM, MUTED))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_captions(cx))),
        );
    let about = label(
        "Burned into Shorts and X clips. The main video ships them as a captions file, and the preview shows them while this panel is open.",
    );
    let presets = div().flex().flex_wrap().gap(px(6.)).children(
        [
            CaptionStyle::WordPop,
            CaptionStyle::Clean,
            CaptionStyle::Karaoke,
            CaptionStyle::Bold,
            CaptionStyle::Off,
        ]
        .into_iter()
        .map(|style| preset_tile(style, design.style == style, accent, cx)),
    );
    let mut body = div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .px(px(14.))
        .py(px(14.))
        .child(header)
        .child(about)
        .child(section("Style").child(presets));
    if design.style != CaptionStyle::Off {
        body = body.child(look(&design, accent, &screen)).child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .when(custom, |row| {
                    row.child(
                        small_button("captions-reset", "Back to the preset")
                            .on_click(cx.listener(|this, _, _, cx| this.reset_look(cx))),
                    )
                })
                .when(channel && custom, |row| {
                    row.child(
                        small_button("captions-channel", "Use for every video")
                            .on_click(cx.listener(|this, _, _, cx| this.look_for_channel(cx))),
                    )
                }),
        );
    }
    body = body
        .children(
            this.captions
                .note
                .clone()
                .map(|n| label(n).text_color(WARN)),
        )
        .child(lines(this, cx));
    div()
        .id("captions-panel")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(&this.captions.scroll)
        .on_action(cx.listener(|this, _: &Escape, window, cx| this.cancel_caption_edit(window, cx)))
        .child(body)
}

fn section(name: &'static str) -> Div {
    div().flex().flex_col().gap(px(8.)).child(label(name))
}

fn small_button(id: &'static str, text: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(28.))
        .px(px(10.))
        .rounded(RADIUS_SM)
        .bg(RAISED)
        .text_size(TEXT_SM)
        .cursor_pointer()
        .hover(|s| s.bg(FLOAT_ON))
        .child(text)
}

/// A preset as a tile with a sample of how it marks the word being said.
fn preset_tile(
    style: CaptionStyle,
    on: bool,
    accent: u32,
    cx: &mut Context<EditScreen>,
) -> Stateful<Div> {
    let preset = CaptionDesign::preset(style);
    let mark = preset.highlight.map_or(accent, hex);
    let word = |text: &'static str| div().px(px(2.)).child(text);
    let sample = div()
        .h(px(34.))
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .bg(rgb(0x2a2f37))
        .text_size(px(13.))
        .line_height(px(18.))
        .map(|sample| match style {
            CaptionStyle::Off => sample.text_color(FAINT).child("No captions"),
            CaptionStyle::Bold => sample
                .font_weight(FontWeight::BLACK)
                .child(word("SHIP").text_color(rgb(0xffffff)))
                .child(word("IT").text_color(rgb(mark))),
            _ => sample.child(
                div()
                    .flex()
                    .px(px(4.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(gpui::rgba(0x0a0b0dc7))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xffffff))
                    .map(|line| match preset.emphasis {
                        Emphasis::Pill => line.child(word("ship")).child(
                            word("it")
                                .rounded(px(3.))
                                .bg(rgb(mark))
                                .text_color(rgb(0x0a0b0d)),
                        ),
                        Emphasis::Fill => line
                            .child(word("ship").text_color(rgb(mark)))
                            .child(word("it")),
                        _ => line.child(word("ship")).child(word("it")),
                    }),
            ),
        });
    div()
        .id(SharedString::from(format!("caption-preset-{style}")))
        .w(px(96.))
        .flex()
        .flex_col()
        .gap(px(6.))
        .p(px(6.))
        .rounded(RADIUS_SM)
        .border_1()
        .border_color(if on {
            Hsla::from(ACCENT)
        } else {
            Hsla::from(RAISED)
        })
        .bg(if on { FLOAT } else { RAISED })
        .cursor_pointer()
        .hover(|s| s.bg(FLOAT_ON))
        .map(ui::keyboard_reachable)
        .border_color(if on {
            Hsla::from(ACCENT)
        } else {
            Hsla::from(RAISED)
        })
        .child(sample)
        .child(
            div()
                .text_size(TEXT_XS)
                .font_weight(FontWeight::MEDIUM)
                .text_color(if on { STRONG } else { MUTED })
                .child(style_name(style)),
        )
        .on_click(cx.listener(move |this, _, _, cx| this.set_design(|d| d.style = style, cx)))
}

/// The look's controls, each writing one field.
fn look(design: &CaptionDesign, accent: u32, screen: &Entity<EditScreen>) -> Div {
    let set = |change: fn(&mut CaptionDesign, i32), value: i32| {
        let screen = screen.clone();
        move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
            screen.update(cx, |this, cx| this.set_design(|d| change(d, value), cx));
        }
    };
    let segments = |id: &'static str,
                    options: Vec<(
        &'static str,
        bool,
        Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App)>,
    )>| {
        ui::segmented(
            id,
            WELL,
            options
                .into_iter()
                .map(|(text, on, click)| Segment::new(text, on, click))
                .collect(),
        )
    };
    let font = segments(
        "caption-font",
        [CaptionFont::Heading, CaptionFont::Body, CaptionFont::Mono]
            .into_iter()
            .enumerate()
            .map(|(i, f)| {
                let text = match f {
                    CaptionFont::Heading => "Heading",
                    CaptionFont::Body => "Body",
                    CaptionFont::Mono => "Mono",
                };
                let click: Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App)> = Box::new(set(
                    |d, i| {
                        d.font =
                            [CaptionFont::Heading, CaptionFont::Body, CaptionFont::Mono][i as usize]
                    },
                    i as i32,
                ));
                (text, design.font == f, click)
            })
            .collect(),
    );
    let weight = segments(
        "caption-weight",
        [500u16, 600, 700, 800, 900]
            .into_iter()
            .map(|w| {
                let text: &'static str = match w {
                    500 => "500",
                    600 => "600",
                    700 => "700",
                    800 => "800",
                    _ => "900",
                };
                let click: Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App)> =
                    Box::new(set(|d, w| d.weight = w as u16, i32::from(w)));
                (text, design.weight == w, click)
            })
            .collect(),
    );
    let back = segments(
        "caption-back",
        [
            (CaptionBack::Box, "Box"),
            (CaptionBack::Outline, "Outline"),
            (CaptionBack::None, "None"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (b, text))| {
            let click: Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App)> = Box::new(set(
                |d, i| {
                    d.back = [CaptionBack::Box, CaptionBack::Outline, CaptionBack::None][i as usize]
                },
                i as i32,
            ));
            (text, design.back == b, click)
        })
        .collect(),
    );
    let position = segments(
        "caption-position",
        [
            (CaptionPosition::Bottom, "Bottom"),
            (CaptionPosition::Middle, "Middle"),
            (CaptionPosition::Top, "Top"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (p, text))| {
            let click: Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App)> = Box::new(set(
                |d, i| {
                    d.position = [
                        CaptionPosition::Bottom,
                        CaptionPosition::Middle,
                        CaptionPosition::Top,
                    ][i as usize]
                },
                i as i32,
            ));
            (text, design.position == p, click)
        })
        .collect(),
    );
    let case = segments(
        "caption-case",
        [
            (CaptionCase::AsSaid, "As said"),
            (CaptionCase::Upper, "UPPER"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (c, text))| {
            let click: Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App)> = Box::new(set(
                |d, i| d.case = [CaptionCase::AsSaid, CaptionCase::Upper][i as usize],
                i as i32,
            ));
            (text, design.case == c, click)
        })
        .collect(),
    );
    let size = stepper(
        "caption-size",
        format!("{}%", design.size),
        (design.size > 50).then(|| set(|d, _| d.size = d.size.saturating_sub(10).max(50), 0)),
        (design.size < 200).then(|| set(|d, _| d.size = (d.size + 10).min(200), 0)),
    );
    let nudge = stepper(
        "caption-nudge",
        match design.nudge {
            0 => "Nudge 0".to_owned(),
            n if n < 0 => format!("Up {}%", -n),
            n => format!("Down {n}%"),
        },
        (design.nudge > -30).then(|| set(|d, _| d.nudge = (d.nudge - 2).max(-30), 0)),
        (design.nudge < 30).then(|| set(|d, _| d.nudge = (d.nudge + 2).min(30), 0)),
    );
    let words = stepper(
        "caption-words",
        match design.words {
            1 => "1 word".to_owned(),
            n => format!("{n} words"),
        },
        (design.words > 1).then(|| set(|d, _| d.words -= 1, 0)),
        (design.words < 12).then(|| set(|d, _| d.words += 1, 0)),
    );
    let swatches = |id: &'static str,
                    options: Vec<(u32, &'static str, Option<Color>)>,
                    current: Option<Color>,
                    apply: fn(&mut CaptionDesign, Option<Color>)| {
        div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .children(
                options
                    .into_iter()
                    .enumerate()
                    .map(|(i, (shown, name, value))| {
                        let screen = screen.clone();
                        let on = current == value;
                        div()
                            .id((id, i))
                            .size(px(24.))
                            .rounded_full()
                            .border_2()
                            .border_color(if on {
                                Hsla::from(STRONG)
                            } else {
                                Hsla::from(RAISED)
                            })
                            .bg(rgb(shown))
                            .cursor_pointer()
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(name).build(window, cx)
                            })
                            .on_click(move |_, _, cx| {
                                screen
                                    .update(cx, |this, cx| this.set_design(|d| apply(d, value), cx))
                            })
                    }),
            )
    };
    let colors = swatches(
        "caption-color",
        COLORS
            .iter()
            .map(|(c, name)| (*c, *name, Some(color(*c))))
            .collect(),
        Some(design.color),
        |d, c| d.color = c.unwrap_or(d.color),
    );
    let highlights = swatches(
        "caption-highlight",
        std::iter::once((accent, "Brand accent", None))
            .chain(
                HIGHLIGHTS
                    .iter()
                    .map(|(c, name)| (*c, *name, Some(color(*c)))),
            )
            .collect(),
        design.highlight,
        |d, c| d.highlight = c,
    );
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(section("Font").child(font).child(weight))
        .child(section("Size").child(size))
        .child(section("Text color").child(colors))
        .when(design.emphasis != Emphasis::None, |look| {
            look.child(section("Word being said").child(highlights))
        })
        .child(section("Behind the words").child(back))
        .child(section("Position").child(position).child(nudge))
        .child(section("Words per line").child(words))
        .child(section("Case").child(case))
}

/// A value between a minus and a plus button, each off at its limit.
fn stepper(
    id: &'static str,
    value: String,
    less: Option<impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static>,
    more: Option<impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static>,
) -> Div {
    let button =
        |index: usize,
         text: &'static str,
         click: Option<Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App)>>| {
            let on = click.is_some();
            div()
                .id((id, index))
                .flex()
                .items_center()
                .justify_center()
                .size(px(28.))
                .rounded(px(7.))
                .text_color(if on { TEXT } else { FAINT })
                .when(on, |b| b.cursor_pointer().hover(|s| s.bg(FLOAT)))
                .child(text)
                .when_some(click, |b, click| b.on_click(click))
        };
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .p(px(3.))
        .rounded(RADIUS_MD)
        .bg(WELL)
        .child(button(
            0,
            "−",
            less.map(|f| Box::new(f) as Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App)>),
        ))
        .child(
            div()
                .flex_1()
                .text_center()
                .text_size(TEXT_SM)
                .font_weight(FontWeight::MEDIUM)
                .child(value),
        )
        .child(button(
            1,
            "+",
            more.map(|f| Box::new(f) as Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App)>),
        ))
}

/// The lines of the chapter on show, the one at the playhead lit; the picked line shows its
/// tools.
fn lines(this: &EditScreen, cx: &mut Context<EditScreen>) -> Div {
    let frames = this
        .map
        .chapters
        .get(this.chapter)
        .map_or(0..0, |c| c.frames.clone());
    let frame_of = |t: Micros| t.0 * u64::from(FPS) / 1_000_000;
    let here = this.captions.lines.iter().enumerate().filter(|(_, l)| {
        l.first()
            .is_some_and(|w| frames.contains(&frame_of(w.word.from)))
    });
    let mut list = div().flex().flex_col().gap(px(2.));
    let mut count = 0;
    for (index, line) in here {
        count += 1;
        let from = frame_of(line[0].word.from);
        let to = frame_of(line[line.len() - 1].word.to);
        let now = from <= this.playhead && this.playhead < to.max(from + 1);
        let picked = this.captions.picked == Some(index);
        let editing = this.captions.editing.as_ref().filter(|e| e.line == index);
        let mut row = div()
            .id(("caption-line", index))
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(10.))
            .py(px(8.))
            .rounded(RADIUS_SM)
            .when(picked, |r| r.bg(RAISED))
            .when(!picked, |r| r.hover(|s| s.bg(RAISED)).cursor_pointer())
            .on_click(cx.listener(move |this, _, _, cx| this.pick_line(index, cx)))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .child(
                        ui::mono(timecode(from))
                            .text_size(TEXT_XS)
                            .text_color(if now { ACCENT } else { FAINT }),
                    )
                    .child(match editing {
                        Some(editing) => div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&editing.input))
                            .into_any_element(),
                        None => div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(14.))
                            .line_height(px(20.))
                            .text_color(if now || picked { STRONG } else { BODY })
                            .child(line_text(line))
                            .into_any_element(),
                    }),
            );
        if picked {
            row = row.child(line_tools(this, index, line, editing.is_some(), cx));
        }
        list = list.child(row);
    }
    section("Lines in this chapter")
        .child(match count {
            0 => label("No caption lines here. Lines come from the transcript of each take.")
                .into_any_element(),
            _ => list.into_any_element(),
        })
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .pt(px(4.))
                .child(ui::hint("↩", "Edit the picked line"))
                .child(ui::hint("esc", "Leave the text as it was")),
        )
}

/// Edit, split before a word, and join with the next line.
fn line_tools(
    this: &EditScreen,
    index: usize,
    line: &[SourceWord],
    editing: bool,
    cx: &mut Context<EditScreen>,
) -> Div {
    let has_next = index + 1 < this.captions.lines.len();
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .pl(px(58.))
        .when(!editing, |tools| {
            tools.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(
                        small_button("caption-edit", "Edit text")
                            .child(div().pl(px(8.)).child(kbd("↩")))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit_caption_line(window, cx)
                            })),
                    )
                    .when(has_next, |row| {
                        row.child(
                            small_button("caption-join", "Join with next line").on_click(
                                cx.listener(move |this, _, _, cx| this.join_line(index, cx)),
                            ),
                        )
                    }),
            )
        })
        .when(line.len() > 1, |tools| {
            tools.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(4.))
                    .child(label("New line from").pr(px(4.)))
                    .children(line.iter().enumerate().skip(1).map(|(word, w)| {
                        div()
                            .id(("caption-split", word))
                            .px(px(6.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .bg(FLOAT)
                            .text_size(TEXT_SM)
                            .cursor_pointer()
                            .hover(|s| s.bg(FLOAT_ON))
                            .child(w.word.text.clone())
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.split_line(index, word, cx)),
                            )
                    })),
            )
        })
}

/// The Text inspector's way in: the style in use and a button that opens the panel.
pub fn entry(this: &EditScreen, cx: &mut Context<EditScreen>) -> AnyElement {
    let style = style_of(this.folder.read(cx));
    div()
        .id("open-captions")
        .flex()
        .items_center()
        .gap(px(6.))
        .text_color(TEXT)
        .cursor_pointer()
        .hover(|s| s.text_color(STRONG))
        .child(style_name(style))
        .child(icon("edit", ICON_SM, MUTED))
        .on_click(cx.listener(|this, _, _, cx| this.toggle_captions(cx)))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{Id, TimeRange, Word};
    use shot_render::captions::CaptionWord;

    fn take() -> Id {
        Id::new("t01").unwrap()
    }

    /// A transcript and the line it shows, word for word, a word every 300 ms.
    fn line(words: &[&str]) -> (Transcript, Vec<SourceWord>) {
        let mut transcript = Transcript::new(take());
        let mut line = Vec::new();
        for (i, text) in words.iter().enumerate() {
            let from = i as u64 * 300_000;
            let range = TimeRange::new(Micros(from), Micros(from + 250_000)).unwrap();
            transcript
                .push(Word {
                    text: (*text).into(),
                    range,
                    confidence: 90,
                })
                .unwrap();
            line.push(SourceWord {
                take: take(),
                at: Micros(from),
                word: CaptionWord {
                    text: (*text).into(),
                    from: Micros(from),
                    to: Micros(from + 250_000),
                },
            });
        }
        (transcript, line)
    }

    #[test]
    fn an_edit_becomes_a_fix_with_a_word_of_context_on_each_side() {
        let (transcript, words) = line(&["we", "love", "lara", "vel.", "today"]);
        let fix = fix_for(&words, &transcript, "we love Laravel. today")
            .unwrap()
            .unwrap();
        assert_eq!(fix.heard, "love lara vel. today");
        assert_eq!(fix.said, "love Laravel. today");
        assert!(fix_for(&words, &transcript, "we love lara vel. today").is_none());
    }

    #[test]
    fn the_last_heard_words_punctuation_is_left_to_the_fix() {
        let (transcript, words) = line(&["ship", "it", "now."]);
        let fix = fix_for(&words, &transcript, "ship it today.")
            .unwrap()
            .unwrap();
        assert_eq!(fix.heard, "it now.");
        assert_eq!(fix.said, "it today", "the fix adds the period back");
        assert!(fix_for(&words, &transcript, "").unwrap().is_err());
    }

    #[test]
    fn a_split_replaces_a_join_at_the_same_word() {
        let cut = with_break(&Cut::default(), take(), Micros(300_000), false);
        let cut = with_break(&cut, take(), Micros(300_000), true);
        assert_eq!(
            cut.caption_breaks,
            [CaptionBreak {
                take: take(),
                at: Micros(300_000),
                split: true
            }]
        );
    }
}
