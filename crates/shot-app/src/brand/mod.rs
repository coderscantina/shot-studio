//! The channel's theme window: every part of `brand/brand.kdl` as controls in sections (colors,
//! fonts, logo, camera, motion, captions, sounds, intro and outro, end card), a generator that
//! proposes looks from one accent or the logo, the channel's saved themes ([0029]) and its
//! overlay presets, beside a live preview of a real take drawn by `shot-render` in the theme.
//!
//! Every control writes the files at once through [`store::Themes`]; the window keeps its own
//! undo history of the brand and theme files (⌘Z, ⌘⇧Z). Typing in one field is one step.
//! Opened from the board window, the studio (⌘⇧B, or ⌘⇧O on the overlay presets) and by
//! [`open_overlay_preset`].
//!
//! [0029]: ../../../../docs/decisions/0029-several-themes-per-channel.md

mod fields;
mod generate;
pub mod overlay;
mod sections;
mod stage;
mod store;

use std::path::{Path, PathBuf};

use gpui::{
    AnyWindowHandle, App, Bounds, Context, Entity, FocusHandle, Focusable, FontWeight, Global,
    KeyBinding, Menu, MenuItem, SharedString, Subscription, TitlebarOptions, Window, WindowBounds,
    WindowOptions, actions, div, point, prelude::*, px, size,
};
use gpui_component::Root;
use shot_core::{Brand, Color, Id, Micros, Overlay, OverlayPreset, ParamValue, TimeRange};
use shot_render::overlay as templates;

use self::fields::{Inputs, Sliders};
use self::generate::Proposal;
use self::overlay::{Commit, Draft, OverlayEditor};
use self::stage::{Sample, Scene, Stage};
use self::store::Themes;
use crate::sound::Audition;
use crate::theme::*;

actions!(brand, [OpenTheme, OpenOverlays]);

/// ⌘⇧B opens the channel theme, ⌘⇧O its overlay presets, from a studio or board window.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-shift-b", OpenTheme, None),
        KeyBinding::new("cmd-shift-o", OpenOverlays, None),
    ]);
}

/// The Channel menu with both.
pub fn menu() -> Menu {
    Menu::new("Channel").items([
        MenuItem::action("Theme…", OpenTheme),
        MenuItem::action("Overlay presets…", OpenOverlays),
    ])
}

/// A part of the theme window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Generate,
    Colors,
    Fonts,
    Logo,
    Camera,
    Motion,
    Captions,
    Sounds,
    Bookends,
    EndCard,
    Overlays,
}

impl Section {
    const ALL: [Self; 11] = [
        Self::Generate,
        Self::Colors,
        Self::Fonts,
        Self::Logo,
        Self::Camera,
        Self::Motion,
        Self::Captions,
        Self::Sounds,
        Self::Bookends,
        Self::EndCard,
        Self::Overlays,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Generate => "Generate",
            Self::Colors => "Colors",
            Self::Fonts => "Fonts",
            Self::Logo => "Logo",
            Self::Camera => "Camera",
            Self::Motion => "Motion",
            Self::Captions => "Captions",
            Self::Sounds => "Sounds",
            Self::Bookends => "Intro and outro",
            Self::EndCard => "End card",
            Self::Overlays => "Overlay presets",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Generate => "spark",
            Self::Colors | Self::Logo => "picture",
            Self::Fonts | Self::Captions => "text",
            Self::Camera => "camera",
            Self::Motion => "play",
            Self::Sounds => "music",
            Self::Bookends | Self::EndCard => "timeline",
            Self::Overlays => "layout",
        }
    }

    /// The moment of the preview that shows what the section changes.
    fn moment(self) -> Moment {
        match self {
            Self::Generate | Self::Colors => Moment::LowerThird,
            Self::Fonts => Moment::ChapterBand,
            Self::Camera | Self::Captions => Moment::Captions,
            Self::Motion | Self::Sounds | Self::Overlays => Moment::LinkCard,
            Self::Logo | Self::Bookends | Self::EndCard => Moment::EndCard,
        }
    }
}

/// A stretch of the preview scene that shows one part of the brand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    ChapterBand,
    LowerThird,
    LinkCard,
    Captions,
    EndCard,
}

impl Moment {
    const ALL: [Self; 5] = [
        Self::ChapterBand,
        Self::LowerThird,
        Self::LinkCard,
        Self::Captions,
        Self::EndCard,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::ChapterBand => "Chapter band",
            Self::LowerThird => "Lower third",
            Self::LinkCard => "Link card",
            Self::Captions => "Camera and captions",
            Self::EndCard => "End card",
        }
    }

    /// Its stretch of the scene, in microseconds.
    fn span(self) -> (u64, u64) {
        match self {
            Self::ChapterBand => (0, 3_500_000),
            Self::LowerThird => (3_500_000, 7_500_000),
            Self::LinkCard => (7_500_000, 11_500_000),
            Self::Captions => (11_500_000, stage::CAPTIONS_UNTIL.0),
            Self::EndCard => (stage::CAPTIONS_UNTIL.0, 19_500_000),
        }
    }

    /// The frame to rest on: its overlay fully in.
    fn rest(self) -> Micros {
        Micros(self.span().0 + 2_200_000)
    }
}

/// The length of the theme preview scene.
const SCENE: Micros = Micros(19_500_000);

/// A control of the theme window, for coalescing undo and keeping inputs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    Color(&'static str),
    Seed,
    Feel(shot_core::PresetName, &'static str),
    Corners,
    Border,
    EndCard,
    Rename,
}

/// The channel's `brand/brand.kdl` as written, empty when it cannot be read: its overlay
/// presets and templates for the Edit screen.
pub fn channel_brand(repo: &Path) -> Brand {
    Themes::open(repo).map(|t| t.brand).unwrap_or_default()
}

/// The open theme windows by channel.
#[derive(Default)]
struct Windows(Vec<(PathBuf, AnyWindowHandle, gpui::WeakEntity<ThemeEditor>)>);

impl Global for Windows {}

/// Opens the theme window of the channel at `repo` on `section`, or brings its open one to the
/// front on it.
pub fn open(repo: &Path, section: Section, cx: &mut App) -> Result<(), String> {
    open_with(repo, cx, move |editor, cx| editor.show_section(section, cx))
}

/// Opens the channel's overlay presets on `preset`, or on a new preset of `template`: what the
/// Library calls to edit an overlay.
pub fn open_overlay_preset(
    repo: &Path,
    preset: Option<Id>,
    template: Option<Id>,
    cx: &mut App,
) -> Result<(), String> {
    open_with(repo, cx, move |editor, cx| {
        editor.show_section(Section::Overlays, cx);
        match (preset, template) {
            (Some(name), _) => editor.select_preset(Some(name), cx),
            (None, Some(template)) => editor.new_preset(&template, cx),
            (None, None) => {}
        }
    })
}

fn open_with(
    repo: &Path,
    cx: &mut App,
    then: impl FnOnce(&mut ThemeEditor, &mut Context<ThemeEditor>) + 'static,
) -> Result<(), String> {
    let repo = crate::studio::key(repo);
    let open = cx.try_global::<Windows>().and_then(|windows| {
        windows
            .0
            .iter()
            .find(|(r, ..)| *r == repo)
            .and_then(|(_, window, editor)| Some((*window, editor.upgrade()?)))
    });
    let mut then = Some(then);
    if let Some((window, editor)) = open
        && window
            .update(cx, |_, window, cx| {
                window.activate_window();
                if let Some(then) = then.take() {
                    editor.update(cx, then);
                }
            })
            .is_ok()
    {
        return Ok(());
    }
    let themes = Themes::open(&repo)?;
    let bounds = Bounds::centered(None, size(px(1500.), px(940.)), cx);
    let mut opened = None;
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(1280.), px(800.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("{} theme", crate::board::channel::name(&repo)).into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(16.), px(16.))),
                }),
                inactive_frame_interval: None,
                ..Default::default()
            },
            |window, cx| {
                let editor = cx.new(|cx| ThemeEditor::new(themes, window, cx));
                if let Some(then) = then.take() {
                    editor.update(cx, then);
                }
                opened = Some(editor.downgrade());
                cx.new(|cx| Root::new(editor, window, cx))
            },
        )
        .map_err(|e| e.to_string())?;
    if let Some(editor) = opened {
        let windows = cx.default_global::<Windows>();
        windows
            .0
            .retain(|(r, _, e)| *r != repo && e.upgrade().is_some());
        windows.0.push((repo, window.into(), editor));
    }
    cx.activate(true);
    Ok(())
}

/// How many changes undo can step back through.
const HISTORY: usize = 200;

pub struct ThemeEditor {
    themes: Themes,
    /// The theme the controls edit, active or not.
    selected: Id,
    section: Section,
    moment: Moment,
    history: Vec<Themes>,
    undone: Vec<Themes>,
    /// The control of the last change, which the next change of the same control joins.
    last: Option<Key>,
    stage: Entity<Stage>,
    sample: Option<Sample>,
    /// The generator's proposals and the one on the preview, not written yet.
    proposals: Vec<Proposal>,
    trial: Option<usize>,
    seed: String,
    /// The overlay preset in the editor, with its editor.
    preset: Option<(Option<Id>, Entity<OverlayEditor>)>,
    /// The sound key being picked in Sounds.
    picking: Option<shot_core::SoundKey>,
    audition: Option<Audition>,
    renaming: bool,
    inputs: Inputs<Key>,
    sliders: Sliders<Key>,
    note: Option<SharedString>,
    error: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl ThemeEditor {
    fn new(themes: Themes, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let sample = Sample::find(&themes.repo);
        let seed = generate::Palette::of(&themes.brand.colors).accent;
        let mut editor = Self {
            selected: themes.active(),
            themes,
            section: Section::Colors,
            moment: Moment::LowerThird,
            history: Vec::new(),
            undone: Vec::new(),
            last: None,
            stage: cx.new(|_| Stage::new()),
            sample,
            proposals: generate::propose(seed),
            trial: None,
            seed: hex(seed),
            preset: None,
            picking: None,
            audition: None,
            renaming: false,
            inputs: Inputs::default(),
            sliders: Sliders::default(),
            note: None,
            error: None,
            focus: cx.focus_handle(),
            _subscriptions: Vec::new(),
        };
        editor.refresh(cx);
        editor
    }

    /// The look the controls show.
    fn look(&self) -> Brand {
        self.themes
            .look(&self.selected)
            .unwrap_or_else(|| store::look_of(&self.themes.brand))
    }

    /// The look the preview shows: the one on trial from the generator, else the selected one.
    fn shown_look(&self) -> Brand {
        let mut look = self.look();
        if let Some(proposal) = self.trial.and_then(|i| self.proposals.get(i)) {
            look.colors = proposal.colors.clone();
            look.fonts = proposal.fonts.clone();
        }
        look
    }

    /// The brand file a video would read with the shown look, for drawing and checks.
    fn shown_brand(&self) -> Result<Brand, String> {
        let brand = self
            .themes
            .with_look(&self.themes.active(), &self.shown_look())
            .brand;
        let format = self
            .sample
            .as_ref()
            .map_or(shot_core::FormatKind::Tutorial, Sample::format);
        stage::resolved(&self.themes.repo, &brand, format)
    }

    fn show_section(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        self.picking = None;
        self.renaming = false;
        if section != Section::Generate {
            self.trial = None;
        }
        if section == Section::Overlays && self.preset.is_none() {
            let first = self
                .themes
                .brand
                .overlay_presets
                .first()
                .map(|p| p.name.clone());
            self.select_preset(first, cx);
        }
        self.moment = section.moment();
        self.refresh(cx);
    }

    fn show_moment(&mut self, moment: Moment, cx: &mut Context<Self>) {
        self.moment = moment;
        self.stage
            .update(cx, |stage, cx| stage.seek(moment.rest(), cx));
        cx.notify();
    }

    /// Plays the moment's overlay coming in.
    fn play_moment(&mut self, cx: &mut Context<Self>) {
        let (from, _) = self.moment.span();
        let rest = self.moment.rest();
        self.stage.update(cx, |stage, cx| {
            stage.play(
                Micros(from.saturating_sub(300_000)),
                Micros(from + 2_400_000),
                rest,
                cx,
            )
        });
    }

    /// The overlays of the preview scene, in the shown look.
    fn scene_overlays(&self) -> Vec<Overlay> {
        let text = |s: &str| ParamValue::Text(s.to_owned());
        let at = |m: Moment| {
            let (from, to) = m.span();
            TimeRange::new(Micros(from), Micros(to)).expect("a moment has a length")
        };
        let lower_third = self
            .themes
            .brand
            .overlay_presets
            .iter()
            .find(|p| p.template.as_str() == templates::lower_third::TEMPLATE)
            .map(|p| p.params.clone())
            .unwrap_or_else(|| {
                vec![
                    (id("text"), text("Your name")),
                    (
                        id("subtitle"),
                        text(&crate::board::channel::name(&self.themes.repo)),
                    ),
                ]
            });
        vec![
            Overlay {
                template: id(templates::chapter_band::TEMPLATE),
                range: at(Moment::ChapterBand),
                params: Vec::new(),
            },
            Overlay {
                template: id(templates::lower_third::TEMPLATE),
                range: at(Moment::LowerThird),
                params: lower_third,
            },
            Overlay {
                template: id(templates::link_card::TEMPLATE),
                range: at(Moment::LinkCard),
                params: vec![(
                    id("url"),
                    text("https://github.com/coderscantina/shot-studio"),
                )],
            },
            Overlay {
                template: id(templates::end_card::TEMPLATE),
                range: at(Moment::EndCard),
                params: Vec::new(),
            },
        ]
    }

    /// Draws the preview again in the shown look and updates the preset editor.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let brand = self.shown_brand();
        let scene = match (&self.sample, &brand) {
            (None, _) => {
                Err("Record a take in this channel to see the theme on a real frame".to_owned())
            }
            (Some(sample), brand) => {
                let project = sample.project(&sample.video.title, SCENE, self.scene_overlays());
                let theme = brand.clone().and_then(|brand| {
                    stage::theme(&self.themes.repo, &brand, sample.format(), &project.cut)
                });
                Ok(Scene {
                    dir: sample.dir.clone(),
                    project,
                    theme,
                    captions: true,
                })
            }
        };
        let rest = self.moment.rest();
        self.stage
            .update(cx, |stage, cx| stage.set_scene(scene, rest, cx));
        self.refresh_preset(cx);
        cx.notify();
    }

    // Writing and undo.

    /// Writes `next` and makes it the state. A change of the same control as the last change
    /// joins its undo step.
    fn commit(&mut self, next: Themes, key: Option<Key>, cx: &mut Context<Self>) {
        if next == self.themes {
            return;
        }
        if let Err(error) = next.write(&self.themes) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        if key.is_none() || key != self.last {
            if self.history.len() == HISTORY {
                self.history.remove(0);
            }
            self.history.push(self.themes.clone());
        }
        self.undone.clear();
        self.last = key;
        self.themes = next;
        self.after_write(cx);
    }

    fn after_write(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        if !self.themes.names().contains(&self.selected) {
            self.selected = self.themes.active();
        }
        crate::studio::brand_changed(&self.themes.repo, cx);
        self.refresh(cx);
    }

    fn step_history(&mut self, undo: bool, cx: &mut Context<Self>) {
        let (from, to) = if undo {
            (&mut self.history, &mut self.undone)
        } else {
            (&mut self.undone, &mut self.history)
        };
        let Some(state) = from.pop() else {
            return;
        };
        if let Err(error) = state.write(&self.themes) {
            from.push(state);
            self.error = Some(error);
            cx.notify();
            return;
        }
        to.push(std::mem::replace(&mut self.themes, state));
        self.last = None;
        self.after_write(cx);
    }

    /// Changes the selected theme's look.
    fn change(
        &mut self,
        key: Option<Key>,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut Brand),
    ) {
        let mut look = self.look();
        change(&mut look);
        let next = self.themes.with_look(&self.selected, &look);
        self.commit(next, key, cx);
    }

    // Themes.

    fn select_theme(&mut self, name: Id, cx: &mut Context<Self>) {
        self.selected = name;
        self.trial = None;
        self.renaming = false;
        self.last = None;
        self.refresh(cx);
    }

    fn manage(
        &mut self,
        result: Result<Themes, String>,
        select: Option<Id>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(next) => {
                self.commit(next, None, cx);
                if let Some(name) = select {
                    self.select_theme(name, cx);
                }
            }
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    fn new_theme(&mut self, cx: &mut Context<Self>) {
        let name = self.themes.unique(&id("new-theme"));
        let result = self.themes.create(&name, &Brand::default());
        self.manage(result, Some(name), cx);
        self.show_section(Section::Generate, cx);
    }

    fn duplicate_theme(&mut self, cx: &mut Context<Self>) {
        let name = self.themes.unique(&self.selected);
        let result = self.themes.create(&name, &self.look());
        self.manage(result, Some(name), cx);
    }

    fn delete_theme(&mut self, cx: &mut Context<Self>) {
        let result = self.themes.delete(&self.selected);
        let active = self.themes.active();
        self.manage(result, Some(active), cx);
    }

    fn activate_theme(&mut self, cx: &mut Context<Self>) {
        let result = self.themes.activate(&self.selected);
        let name = self.selected.clone();
        self.manage(result, Some(name), cx);
        self.note = Some(
            format!(
                "{} is the channel's theme now",
                store::display(&self.selected)
            )
            .into(),
        );
    }

    fn renamed(
        this: &mut Self,
        _: &Key,
        text: &str,
        done: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !done {
            return;
        }
        this.renaming = false;
        match store::slug(text) {
            Some(to) => {
                let result = this.themes.rename(&this.selected, &to);
                this.manage(result, Some(to), cx);
            }
            None => {
                this.error = Some("A theme name needs a letter".into());
                cx.notify();
            }
        }
    }

    // Overlay presets.

    /// The preset editor's preview source: the channel's sample in the shown look.
    fn preset_preview(&self) -> overlay::Preview {
        let brand = self.shown_brand();
        let format = self
            .sample
            .as_ref()
            .map_or(shot_core::FormatKind::Tutorial, Sample::format);
        let theme = brand.clone().and_then(|brand| {
            stage::theme(
                &self.themes.repo,
                &brand,
                format,
                &shot_core::Cut::default(),
            )
        });
        overlay::Preview {
            sample: self.sample.clone(),
            brand: brand.unwrap_or_default(),
            theme,
            chapter: self
                .sample
                .as_ref()
                .map_or_else(|| "Chapter".to_owned(), |s| s.video.title.clone()),
        }
    }

    fn select_preset(&mut self, name: Option<Id>, cx: &mut Context<Self>) {
        let Some(name) = name else {
            self.preset = None;
            cx.notify();
            return;
        };
        let Some(preset) = self
            .themes
            .brand
            .overlay_presets
            .iter()
            .find(|p| p.name == name)
        else {
            return;
        };
        let draft = draft_of(preset);
        self.open_preset_editor(Some(name), draft, cx);
    }

    fn open_preset_editor(&mut self, name: Option<Id>, draft: Draft, cx: &mut Context<Self>) {
        let preview = self.preset_preview();
        match &self.preset {
            Some((_, editor)) => {
                editor.update(cx, |editor, cx| editor.set_draft(draft, Some(preview), cx));
                self.preset = Some((name, editor.clone()));
            }
            None => {
                let editor = cx.new(|cx| OverlayEditor::new(draft, preview, cx));
                self._subscriptions
                    .push(cx.subscribe(&editor, |this, _, Commit(draft), cx| {
                        this.write_preset(draft.clone(), cx);
                    }));
                self.preset = Some((name, editor));
            }
        }
        cx.notify();
    }

    /// A new preset of `template` with example values, written at once.
    fn new_preset(&mut self, template: &Id, cx: &mut Context<Self>) {
        let names: Vec<Id> = self
            .themes
            .brand
            .overlay_presets
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let name = (1..)
            .map(|n| match n {
                1 => template.clone(),
                n => id(&format!("{template}-{n}")),
            })
            .find(|n| !names.contains(n))
            .expect("some number is free");
        let draft = overlay::starter(Some(name.clone()), template);
        self.write_preset(draft, cx);
        self.select_preset(Some(name), cx);
    }

    /// Writes `draft` as the preset the editor shows, renaming it when its name changed.
    fn write_preset(&mut self, draft: Draft, cx: &mut Context<Self>) {
        let Some(name) = draft.name.clone() else {
            return;
        };
        let old = self.preset.as_ref().and_then(|(n, _)| n.clone());
        if old.as_ref() != Some(&name)
            && self
                .themes
                .brand
                .overlay_presets
                .iter()
                .any(|p| p.name == name)
        {
            self.error = Some(format!("There is a preset called {name} already"));
            cx.notify();
            return;
        }
        let preset = OverlayPreset {
            name: name.clone(),
            template: draft.template,
            length: draft.length,
            params: draft.params,
        };
        let next = self.themes.with_channel(|brand| {
            brand
                .overlay_presets
                .retain(|p| Some(&p.name) != old.as_ref() && p.name != name);
            brand.overlay_presets.push(preset);
            brand.overlay_presets.sort_by(|a, b| a.name.cmp(&b.name));
        });
        if let Some((selected, _)) = &mut self.preset {
            *selected = Some(name);
        }
        self.commit(next, None, cx);
    }

    fn delete_preset(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.preset.as_ref().and_then(|(n, _)| n.clone()) else {
            return;
        };
        let next = self.themes.with_channel(|brand| {
            brand.overlay_presets.retain(|p| p.name != name);
        });
        self.preset = None;
        self.commit(next, None, cx);
        let first = self
            .themes
            .brand
            .overlay_presets
            .first()
            .map(|p| p.name.clone());
        self.select_preset(first, cx);
    }

    /// Shows the preset as the files have it, after a write or an undo.
    fn refresh_preset(&mut self, cx: &mut Context<Self>) {
        let Some((Some(name), editor)) = &self.preset else {
            return;
        };
        let editor = editor.clone();
        let found = self
            .themes
            .brand
            .overlay_presets
            .iter()
            .find(|p| p.name == *name);
        let Some(draft) = found.map(draft_of) else {
            self.preset = None;
            return;
        };
        let preview = self.preset_preview();
        editor.update(cx, |editor, cx| editor.set_draft(draft, Some(preview), cx));
    }
}

fn draft_of(preset: &OverlayPreset) -> Draft {
    Draft {
        name: Some(preset.name.clone()),
        template: preset.template.clone(),
        params: preset.params.clone(),
        length: preset.length,
    }
}

fn id(text: &str) -> Id {
    Id::new(text).expect("an id")
}

/// `#rrggbb`.
pub fn hex(Color([r, g, b]): Color) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// A color typed as `#rrggbb` or `rrggbb`.
pub fn parse_hex(text: &str) -> Option<Color> {
    let digits = text.trim().trim_start_matches('#');
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    let [_, r, g, b] = value.to_be_bytes();
    Some(Color([r, g, b]))
}

/// A square of `color`.
pub fn swatch(Color([r, g, b]): Color, size: f32) -> gpui::Div {
    div()
        .size(px(size))
        .flex_none()
        .rounded(px(5.))
        .border_1()
        .border_color(FLOAT_ON)
        .bg(gpui::rgb(u32::from_be_bytes([0, r, g, b])))
}

impl Focusable for ThemeEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ThemeEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let controls = self.section_view(window, cx);
        div()
            .id("theme-window")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(FRAME)
            .text_color(TEXT)
            .font_family(SANS)
            .text_size(TEXT_BASE)
            .on_action(
                cx.listener(|this, _: &crate::studio::Undo, _, cx| this.step_history(true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &crate::studio::Redo, _, cx| this.step_history(false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenOverlays, _, cx| {
                    this.show_section(Section::Overlays, cx)
                }),
            )
            .child(self.title_bar())
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(2.))
                    .child(self.sidebar(window, cx))
                    .child(
                        crate::ui::pane()
                            .id("theme-controls")
                            .w(px(420.))
                            .flex_none()
                            .overflow_y_scroll()
                            .px(px(20.))
                            .py(px(18.))
                            .child(controls),
                    )
                    .child(self.preview_pane(cx)),
            )
    }
}

impl ThemeEditor {
    fn title_bar(&self) -> impl IntoElement {
        div()
            .h(px(46.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .pl(px(92.))
            .pr(px(16.))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(crate::board::channel::name(&self.themes.repo)),
            )
            .child(div().text_color(MUTED).child("Theme"))
            .child(div().flex_1())
            .children(
                self.note
                    .clone()
                    .map(|n| div().text_size(TEXT_SM).text_color(MUTED).child(n)),
            )
    }

    /// The preview with the moments to show and a play button.
    fn preview_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match (self.section, &self.preset) {
            (Section::Overlays, Some((_, editor))) => {
                editor.update(cx, |editor, cx| editor.preview(cx))
            }
            _ => div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(self.stage.clone())
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.))
                        .children(Moment::ALL.iter().map(|&moment| {
                            fields::chip(
                                SharedString::from(format!("moment-{}", moment.title())),
                                moment.title(),
                                self.moment == moment,
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.show_moment(moment, cx)),
                            )
                        }))
                        .child(div().flex_1())
                        .child(
                            fields::button("play-moment", "Play entrance")
                                .on_click(cx.listener(|this, _, _, cx| this.play_moment(cx))),
                        ),
                ),
        };
        let trying = self.trial.and_then(|i| self.proposals.get(i)).map(|p| {
            div().text_size(TEXT_SM).text_color(ACCENT).child(format!(
                "Trying {}. Use it to write it to the theme",
                p.name
            ))
        });
        crate::ui::pane()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(20.))
            .children(trying)
            .children(self.error.clone().map(crate::ui::alert))
            .child(body)
    }
}
