//! The theme window's sidebar (themes and sections) and the controls of each section. Every
//! control changes the selected theme's look through [`ThemeEditor::change`], which writes the
//! files at once.

use std::path::{Path, PathBuf};

use gpui::{
    Context, Div, FontWeight, PathPromptOptions, SharedString, Window, div, prelude::*, px,
};
use gpui_component::switch::Switch;
use shot_core::{
    Bookend, BundledFont, Camera, CameraShape, Font, FormatKind, Id, Micros, Phase, PresetName,
    Shadow, SoundEffect, SoundKey,
};

use super::fields::{self, button, chip, group, heading, parse_seconds, row, seconds};
use super::generate::{self, Palette};
use super::{Key, Moment, Section, ThemeEditor, hex, parse_hex, store, swatch};
use crate::theme::*;
use crate::ui::{Segment, icon, segmented};

const FONTS: [(BundledFont, &str); 4] = [
    (BundledFont::Geist, "Geist"),
    (BundledFont::Inter, "Inter"),
    (BundledFont::GeistMono, "Geist Mono"),
    (BundledFont::JetbrainsMono, "JetBrains Mono"),
];

const PRESETS: [(PresetName, &str); 4] = [
    (PresetName::Reactive, "Reactive"),
    (PresetName::Editorial, "Editorial"),
    (PresetName::Launch, "Launch"),
    (PresetName::Calm, "Calm"),
];

const FORMATS: [(FormatKind, &str); 4] = [
    (FormatKind::Reaction, "Reaction"),
    (FormatKind::Opinion, "Opinion"),
    (FormatKind::ProductTour, "Product tour"),
    (FormatKind::Tutorial, "Tutorial"),
];

const KNOBS: [(&str, &str); 4] = [
    ("tempo", "How fast parts come in"),
    ("bounce", "How far they overshoot"),
    ("density", "How far they travel, tilt and scale"),
    ("stagger", "The delay between parts"),
];

fn preset_of(name: PresetName) -> shot_render::motion::Preset {
    use shot_render::motion::Preset;
    match name {
        PresetName::Reactive => Preset::Reactive,
        PresetName::Editorial => Preset::Editorial,
        PresetName::Launch => Preset::Launch,
        PresetName::Calm => Preset::Calm,
    }
}

fn name_of(preset: shot_render::motion::Preset) -> PresetName {
    use shot_render::motion::Preset;
    match preset {
        Preset::Reactive => PresetName::Reactive,
        Preset::Editorial => PresetName::Editorial,
        Preset::Launch => PresetName::Launch,
        Preset::Calm => PresetName::Calm,
    }
}

/// The sounds the brand can name: the channel's `brand/sounds/`, then the bundled effects.
fn sounds(repo: &Path) -> Vec<Id> {
    let mut names: Vec<Id> = std::fs::read_dir(repo.join("brand/sounds"))
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let path = e.ok()?.path();
            let ext = path.extension()?.to_str()?;
            shot_audio::sound::EXTENSIONS
                .contains(&ext)
                .then(|| Id::new(path.file_stem()?.to_str()?))
                .flatten()
        })
        .collect();
    names.sort();
    for bundled in shot_audio::sound::BUNDLED {
        if bundled.kind == shot_audio::sound::Kind::Sfx
            && let Some(name) = Id::new(bundled.name)
            && !names.contains(&name)
        {
            names.push(name);
        }
    }
    names
}

fn phase_name(phase: Option<Phase>) -> &'static str {
    match phase {
        None => "start",
        Some(phase) => phase.as_str(),
    }
}

impl ThemeEditor {
    /// The themes of the channel and the sections.
    pub(super) fn sidebar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active = self.themes.active();
        let mut themes = div().flex().flex_col().gap(px(2.));
        for name in self.themes.names() {
            let selected = name == self.selected;
            let is_active = name == active;
            let entry = if selected && self.renaming {
                let state = self.inputs.get(
                    Key::Rename,
                    &store::display(&name),
                    "Theme name",
                    window,
                    cx,
                    Self::renamed,
                );
                fields::input(&state, false).into_any_element()
            } else {
                let pick = name.clone();
                crate::ui::keyboard_reachable(
                    div()
                        .id(SharedString::from(format!("theme-{name}")))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .h(px(30.))
                        .px(px(10.))
                        .rounded(RADIUS_SM)
                        .bg(if selected { RAISED } else { SURFACE })
                        .hover(|s| s.bg(RAISED))
                        .cursor_pointer()
                        .text_size(TEXT_SM)
                        .child(swatch(
                            Palette::of(&self.themes.look(&name).unwrap_or_default().colors).accent,
                            12.,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .text_color(if selected { STRONG } else { BODY })
                                .child(store::display(&name)),
                        )
                        .when(is_active, |row| {
                            row.child(div().text_size(TEXT_XS).text_color(ACCENT).child("Active"))
                        })
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.select_theme(pick.clone(), cx)),
                        ),
                )
                .into_any_element()
            };
            themes = themes.child(entry);
        }
        let selected_active = self.selected == active;
        let actions = div()
            .flex()
            .flex_wrap()
            .gap(px(4.))
            .child(
                button("theme-new", "New")
                    .on_click(cx.listener(|this, _, _, cx| this.new_theme(cx))),
            )
            .child(
                button("theme-duplicate", "Duplicate")
                    .on_click(cx.listener(|this, _, _, cx| this.duplicate_theme(cx))),
            )
            .child(
                button("theme-rename", "Rename").on_click(cx.listener(|this, _, _, cx| {
                    this.renaming = true;
                    cx.notify();
                })),
            )
            .when(!selected_active, |row| {
                row.child(
                    button("theme-activate", "Make active")
                        .text_color(ACCENT)
                        .on_click(cx.listener(|this, _, _, cx| this.activate_theme(cx))),
                )
                .child(
                    button("theme-delete", "Delete")
                        .text_color(CUT_TEXT)
                        .on_click(cx.listener(|this, _, _, cx| this.delete_theme(cx))),
                )
            });
        let sections = Section::ALL.iter().map(|&section| {
            let on = self.section == section;
            crate::ui::keyboard_reachable(
                div()
                    .id(SharedString::from(format!("section-{}", section.title())))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(32.))
                    .px(px(10.))
                    .rounded(RADIUS_SM)
                    .bg(if on { RAISED } else { SURFACE })
                    .hover(|s| s.bg(RAISED))
                    .cursor_pointer()
                    .text_size(TEXT_SM)
                    .text_color(if on { STRONG } else { BODY })
                    .child(icon(
                        section.icon(),
                        ICON_SM,
                        if on { ACCENT } else { MUTED },
                    ))
                    .child(section.title())
                    .on_click(cx.listener(move |this, _, _, cx| this.show_section(section, cx))),
            )
        });
        crate::ui::pane()
            .id("theme-sidebar")
            .w(px(232.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(12.))
            .py(px(16.))
            .overflow_y_scroll()
            .child(crate::ui::label("Themes").px(px(10.)))
            .child(themes)
            .child(actions)
            .child(crate::ui::label("Sections").px(px(10.)).pt(px(12.)))
            .children(sections)
    }

    pub(super) fn section_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let editing = format!("Editing {}", store::display(&self.selected));
        let body = match self.section {
            Section::Generate => self.generate_section(window, cx),
            Section::Colors => self.colors_section(window, cx),
            Section::Fonts => self.fonts_section(cx),
            Section::Logo => self.logo_section(cx),
            Section::Camera => self.camera_section(window, cx),
            Section::Motion => self.motion_section(window, cx),
            Section::Captions => self.captions_section(),
            Section::Sounds => self.sounds_section(cx),
            Section::Bookends => self.bookends_section(cx),
            Section::EndCard => self.end_card_section(window, cx),
            Section::Overlays => self.overlays_section(window, cx),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .when(self.section != Section::Overlays, |d| {
                d.child(div().text_size(TEXT_XS).text_color(MUTED).child(editing))
            })
            .child(body)
    }

    // Generate.

    fn seed_typed(
        this: &mut Self,
        _: &Key,
        text: &str,
        _: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        this.seed = text.to_owned();
        if let Some(color) = parse_hex(text) {
            this.proposals = generate::propose(color);
            this.trial = None;
            this.refresh(cx);
        }
        cx.notify();
    }

    fn accent_from_logo(&mut self, cx: &mut Context<Self>) {
        let Some(logo) = self.look().logo else {
            self.error = Some("This theme has no logo yet. Add one under Logo".into());
            cx.notify();
            return;
        };
        let brand = shot_core::Brand {
            logo: Some(logo),
            ..Default::default()
        };
        let theme = super::stage::theme(
            &self.themes.repo,
            &brand,
            FormatKind::Tutorial,
            &shot_core::Cut::default(),
        );
        let accent = theme.ok().and_then(|t| {
            t.logo
                .as_ref()
                .and_then(|l| generate::accent_of(l.image.data.data()))
        });
        match accent {
            Some(accent) => {
                self.seed = hex(accent);
                self.proposals = generate::propose(accent);
                self.trial = None;
                self.note = Some(format!("Found {} in the logo", hex(accent)).into());
                self.refresh(cx);
            }
            None => {
                self.error =
                    Some("The logo has no color to start from. Type an accent instead".into());
                cx.notify();
            }
        }
    }

    fn try_proposal(&mut self, index: usize, cx: &mut Context<Self>) {
        self.trial = (self.trial != Some(index)).then_some(index);
        self.refresh(cx);
    }

    fn use_proposal(&mut self, cx: &mut Context<Self>) {
        let Some(proposal) = self.trial.and_then(|i| self.proposals.get(i)).cloned() else {
            return;
        };
        self.trial = None;
        self.change(None, cx, |look| {
            look.colors = proposal.colors;
            look.fonts = proposal.fonts;
        });
        self.note = Some(format!("{} written to the theme", proposal.name).into());
    }

    fn generate_section(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let seed = self.seed.clone();
        let refused = parse_hex(&seed).is_none();
        let state = self
            .inputs
            .get(Key::Seed, &seed, "#rrggbb", window, cx, Self::seed_typed);
        let cards = self.proposals.iter().enumerate().map(|(i, proposal)| {
            let p = Palette::of(&proposal.colors);
            let on = self.trial == Some(i);
            let font = |f: &Option<Font>| match f {
                Some(Font::Bundled(b)) => FONTS.iter().find(|(x, _)| x == b).map_or("", |(_, n)| n),
                _ => "",
            };
            let fonts = format!(
                "{} and {}",
                font(&proposal.fonts.heading),
                font(&proposal.fonts.body)
            );
            crate::ui::keyboard_reachable(
                div()
                    .id(("proposal", i))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .p(px(10.))
                    .rounded(RADIUS_MD)
                    .border_1()
                    .border_color(if on { ACCENT } else { RAISED })
                    .bg(RAISED)
                    .hover(|s| s.bg(FLOAT))
                    .cursor_pointer()
                    .child(
                        div()
                            .h(px(54.))
                            .rounded(RADIUS_SM)
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(10.))
                            .bg(gpui::rgb(rgb(p.background)))
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.))
                                    .px(px(8.))
                                    .py(px(6.))
                                    .rounded(px(6.))
                                    .bg(gpui::rgb(rgb(p.card)))
                                    .child(
                                        div()
                                            .h(px(3.))
                                            .w(px(28.))
                                            .rounded(px(2.))
                                            .bg(gpui::rgb(rgb(p.accent))),
                                    )
                                    .child(
                                        div()
                                            .text_size(TEXT_XS)
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(gpui::rgb(rgb(p.text)))
                                            .child("Lower third"),
                                    ),
                            )
                            .child(
                                div()
                                    .px(px(8.))
                                    .py(px(2.))
                                    .rounded(px(10.))
                                    .bg(gpui::rgb(rgb(p.accent)))
                                    .text_size(TEXT_XS)
                                    .text_color(gpui::rgb(rgb(p.background)))
                                    .child("Pop"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_size(TEXT_SM)
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(proposal.name),
                            )
                            .child(div().flex_1())
                            .children(
                                [p.accent, p.text, p.card, p.background].map(|c| swatch(c, 12.)),
                            ),
                    )
                    .child(div().text_size(TEXT_XS).text_color(MUTED).child(fonts))
                    .on_click(cx.listener(move |this, _, _, cx| this.try_proposal(i, cx))),
            )
        });
        let grid = div().grid().grid_cols(2).gap(px(8.)).children(cards);
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(heading(
                "Generate",
                "Four looks from one accent color or your logo. Click one to try it on the preview, then use it and fine-tune.",
            ))
            .child(row(
                "Accent",
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .children(parse_hex(&seed).map(|c| swatch(c, 20.)))
                    .child(fields::input(&state, refused).flex_1()),
            ))
            .child(
                div().pl(px(100.)).child(
                    button("from-logo", "From the logo")
                        .on_click(cx.listener(|this, _, _, cx| this.accent_from_logo(cx))),
                ),
            )
            .child(grid)
            .when(self.trial.is_some(), |d| {
                d.child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            button("use-proposal", "Use this look")
                                .bg(ACCENT)
                                .text_color(ON_ACCENT)
                                .hover(|s| s.bg(ACCENT))
                                .on_click(cx.listener(|this, _, _, cx| this.use_proposal(cx))),
                        )
                        .child(button("back-to-theme", "Back to the theme").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.trial = None;
                                this.refresh(cx);
                            },
                        ))),
                )
            })
    }

    // Colors.

    fn color_typed(
        this: &mut Self,
        key: &Key,
        text: &str,
        _: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Key::Color(role) = key else {
            return;
        };
        let value = match text.trim() {
            "" => None,
            t => match parse_hex(t) {
                Some(color) => Some(color),
                None => return,
            },
        };
        let role = *role;
        this.change(Some(key.clone()), cx, |look| {
            let c = &mut look.colors;
            *match role {
                "accent" => &mut c.accent,
                "text" => &mut c.text,
                "card" => &mut c.card,
                _ => &mut c.background,
            } = value;
        });
    }

    fn colors_section(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let look = self.look();
        let palette = Palette::of(&look.colors);
        let warnings = generate::warnings(&look.colors);
        let roles = [
            (
                "accent",
                "Accent",
                look.colors.accent,
                palette.accent,
                "Labels, rules, pills and the lit caption word",
            ),
            (
                "text",
                "Text",
                look.colors.text,
                palette.text,
                "Words on cards and titles",
            ),
            (
                "card",
                "Card",
                look.colors.card,
                palette.card,
                "Lower thirds, link cards and bands",
            ),
            (
                "background",
                "Background",
                look.colors.background,
                palette.background,
                "The end card, titles and dips",
            ),
        ];
        let mut out = div().flex().flex_col().gap(px(10.)).child(heading(
            "Colors",
            "The four colors every template draws in. Empty keeps the default, shown in its swatch.",
        ));
        for (role, title, set, shown, about) in roles {
            let key = Key::Color(role);
            let value = set.map(hex).unwrap_or_default();
            let typed_invalid = false;
            let state = self
                .inputs
                .get(key, &value, "default", window, cx, Self::color_typed);
            let typed = state.read(cx).value().to_string();
            let refused =
                typed_invalid || (!typed.trim().is_empty() && parse_hex(&typed).is_none());
            out = out.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(row(
                        title,
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(swatch(shown, 24.))
                            .child(fields::input(&state, refused).flex_1()),
                    ))
                    .child(
                        div()
                            .pl(px(100.))
                            .text_size(TEXT_XS)
                            .text_color(MUTED)
                            .child(about),
                    )
                    .children(warnings.iter().filter(|w| w.on == role).map(|w| {
                        div()
                            .pl(px(100.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(TEXT_XS)
                            .text_color(WARN)
                            .child(icon("alert", px(12.), WARN))
                            .child(w.message.clone())
                    })),
            );
        }
        out
    }

    // Fonts.

    fn fonts_section(&mut self, cx: &mut Context<Self>) -> Div {
        let look = self.look();
        let roles: [(&'static str, &str, Option<Font>); 3] = [
            ("heading", "Headings", look.fonts.heading.clone()),
            ("body", "Body", look.fonts.body.clone()),
            ("mono", "Code", look.fonts.mono.clone()),
        ];
        let mut out = div().flex().flex_col().gap(px(10.)).child(heading(
            "Fonts",
            "The bundled OFL fonts, or a font file of the channel. Headings are bold, body medium.",
        ));
        for (index, (role, title, current)) in roles.into_iter().enumerate() {
            let default = if role == "mono" {
                BundledFont::GeistMono
            } else {
                BundledFont::Geist
            };
            let shown = match &current {
                Some(Font::Bundled(font)) => Some(*font),
                Some(Font::File(_)) => None,
                None => Some(default),
            };
            let file = match &current {
                Some(Font::File(path)) => Some(path.rsplit('/').next().unwrap_or(path).to_owned()),
                _ => None,
            };
            let chips = div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(choice(
                    FONT_IDS[index],
                    &FONTS,
                    shown,
                    cx,
                    move |look, font| set_font(look, role, Some(Font::Bundled(font))),
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            button(
                                SharedString::from(format!("font-{role}-file")),
                                file.clone().unwrap_or_else(|| "Font file…".into()),
                            )
                            .when(file.is_some(), |b| b.text_color(ACCENT))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.pick_file("Use as font", "fonts", cx, move |look, path| {
                                        set_font(look, role, Some(Font::File(path)))
                                    })
                                },
                            )),
                        )
                        .when(current.is_none(), |d| {
                            d.child(div().text_size(TEXT_XS).text_color(MUTED).child("default"))
                        })
                        .when(current.is_some(), |d| {
                            d.child(
                                button(SharedString::from(format!("font-{role}-reset")), "Default")
                                    .text_color(MUTED)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.change(None, cx, |look| set_font(look, role, None))
                                    })),
                            )
                        }),
                );
            out = out.child(group(title)).child(chips);
        }
        out
    }

    // Logo.

    fn logo_section(&mut self, cx: &mut Context<Self>) -> Div {
        let logo = self.look().logo;
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(heading(
                "Logo",
                "An SVG or PNG. It opens the generated intro title and sits on the end card.",
            ))
            .child(row(
                "File",
                div()
                    .font_family(MONO)
                    .text_size(TEXT_SM)
                    .text_color(if logo.is_some() { BODY } else { MUTED })
                    .child(logo.clone().unwrap_or_else(|| "None".into())),
            ))
            .child(
                div()
                    .pl(px(100.))
                    .flex()
                    .gap(px(6.))
                    .child(button("logo-choose", "Choose…").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.pick_file("Use as logo", "", cx, |look, path| {
                                look.logo = Some(path)
                            })
                        },
                    )))
                    .when(logo.is_some(), |d| {
                        d.child(button("logo-generate", "Generate a look from it").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.show_section(Section::Generate, cx);
                                this.accent_from_logo(cx);
                            }),
                        ))
                        .child(
                            button("logo-remove", "Remove").text_color(MUTED).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.change(None, cx, |l| l.logo = None)
                                }),
                            ),
                        )
                    }),
            )
    }

    // Camera.

    fn slid(this: &mut Self, key: &Key, value: f32, done: bool, cx: &mut Context<Self>) {
        let value = value.round();
        let key = key.clone();
        this.change(Some(key.clone()), cx, |look| match key {
            Key::Corners | Key::Border => {
                let camera = look.camera.get_or_insert(DEFAULT_CAMERA);
                let v = value as u16;
                if key == Key::Corners {
                    camera.corners = v;
                } else {
                    camera.border = v;
                }
            }
            Key::Feel(name, knob) => {
                let preset = preset_entry(look, name);
                let v = Some(value as u8);
                match knob {
                    "tempo" => preset.tempo = v,
                    "bounce" => preset.bounce = v,
                    "density" => preset.density = v,
                    _ => preset.stagger = v,
                }
            }
            _ => {}
        });
        if done {
            this.last = None;
        }
    }

    fn camera_section(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let camera = self.look().camera;
        let mut out = div().flex().flex_col().gap(px(10.)).child(heading(
            "Camera",
            "How the camera is framed beside the screen. Without a frame it keeps the layout's corners.",
        ));
        let Some(camera) = camera else {
            return out.child(
                button("camera-frame", "Frame the camera").on_click(cx.listener(
                    |this, _, _, cx| this.change(None, cx, |l| l.camera = Some(DEFAULT_CAMERA)),
                )),
            );
        };
        let shapes = [
            (CameraShape::Rounded, "Rounded"),
            (CameraShape::Circle, "Circle"),
            (CameraShape::Square, "Square"),
        ];
        let shadows = [
            (Shadow::None, "None"),
            (Shadow::Soft, "Soft"),
            (Shadow::Hard, "Hard"),
        ];
        let corners = self.sliders.get(
            Key::Corners,
            f32::from(camera.corners),
            (0., 120., 1.),
            window,
            cx,
            Self::slid,
        );
        let border = self.sliders.get(
            Key::Border,
            f32::from(camera.border),
            (0., 24., 1.),
            window,
            cx,
            Self::slid,
        );
        out = out
            .child(row(
                "Shape",
                choice("camera-shape", &shapes, Some(camera.shape), cx, |l, shape| {
                    if let Some(c) = &mut l.camera {
                        c.shape = shape;
                    }
                }),
            ))
            .child(row("Corners", fields::slider(&corners, "Corners", format!("{} px", camera.corners))))
            .child(row("Border", fields::slider(&border, "Border", format!("{} px", camera.border))))
            .child(row(
                "Shadow",
                choice("camera-shadow", &shadows, Some(camera.shadow), cx, |l, shadow| {
                    if let Some(c) = &mut l.camera {
                        c.shadow = shadow;
                    }
                }),
            ))
            .child(div().pl(px(100.)).text_size(TEXT_XS).text_color(MUTED).child("Corners and border in pixels of the 4K output; the border is in the accent"))
            .child(
                div().pl(px(100.)).child(
                    button("camera-unframe", "Keep the layout's corners")
                        .text_color(MUTED)
                        .on_click(cx.listener(|this, _, _, cx| this.change(None, cx, |l| l.camera = None))),
                ),
            );
        out
    }

    // Motion.

    fn motion_section(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let look = self.look();
        let mut out = div().flex().flex_col().gap(px(10.)).child(heading(
            "Motion",
            "The preset each format moves in, and how each preset feels. Play the entrance to see a card come in.",
        ));
        for (index, (format, title)) in FORMATS.into_iter().enumerate() {
            let current = look.motion.get(format);
            let default = name_of(shot_render::theme::default_preset(format));
            let chips = choice(
                MOTION_IDS[index],
                &PRESETS,
                Some(current.unwrap_or(default)),
                cx,
                move |l, preset| {
                    set_motion(&mut l.motion, format, (preset != default).then_some(preset))
                },
            );
            out = out.child(row(title, chips));
        }
        let tuned = self
            .sample
            .as_ref()
            .map(|s| {
                look.motion
                    .get(s.format())
                    .unwrap_or(name_of(shot_render::theme::default_preset(s.format())))
            })
            .unwrap_or(PresetName::Calm);
        for (preset, name) in PRESETS {
            let own = preset_of(preset).feel();
            let set = look.presets.iter().find(|p| p.name == preset).cloned();
            let value = |knob: &str| -> u8 {
                let set = set.as_ref().and_then(|p| match knob {
                    "tempo" => p.tempo,
                    "bounce" => p.bounce,
                    "density" => p.density,
                    _ => p.stagger,
                });
                set.unwrap_or(match knob {
                    "tempo" => own.tempo,
                    "bounce" => own.bounce,
                    "density" => own.density,
                    _ => own.stagger,
                })
            };
            let mut block = div().flex().flex_col().gap(px(4.)).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(group(format!("{name} feel")))
                    .when(preset == tuned, |d| {
                        d.child(
                            div()
                                .pt(px(10.))
                                .text_size(TEXT_XS)
                                .text_color(ACCENT)
                                .child("on the preview"),
                        )
                    })
                    .child(div().flex_1())
                    .when(set.is_some(), |d| {
                        d.child(
                            button(SharedString::from(format!("feel-reset-{name}")), "Reset")
                                .text_color(MUTED)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.change(None, cx, |l| {
                                        l.presets.retain(|p| p.name != preset)
                                    })
                                })),
                        )
                    }),
            );
            for (knob, about) in KNOBS {
                let v = value(knob);
                let slider = self.sliders.get(
                    Key::Feel(preset, knob),
                    f32::from(v),
                    (0., 100., 1.),
                    window,
                    cx,
                    Self::slid,
                );
                block = block.child(row(knob, fields::slider(&slider, about, v.to_string())));
            }
            let beat = shot_audio::beats::beat_sync(preset, set.as_ref().and_then(|p| p.beat_sync));
            block = block.child(row(
                "beat sync",
                Switch::new(SharedString::from(format!("beat-{name}")))
                    .checked(beat)
                    .accessibility_label("Snap entrances to the music's beat")
                    .on_click(cx.listener(move |this, on: &bool, _, cx| {
                        let on = *on;
                        this.change(None, cx, |l| preset_entry(l, preset).beat_sync = Some(on))
                    })),
            ));
            out = out.child(block);
        }
        out.child(
            button("motion-play", "Play a card entering").on_click(cx.listener(
                |this, _, _, cx| {
                    this.moment = Moment::LinkCard;
                    this.play_moment(cx);
                },
            )),
        )
    }

    // Captions.

    fn captions_section(&self) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(heading(
                "Captions",
                "How burned-in captions look over the camera and screen.",
            ))
            .child(
                div()
                    .text_size(TEXT_SM)
                    .text_color(BODY)
                    .child(super::stage::captions_note()),
            )
    }

    // Sounds.

    fn audition(&mut self, sound: Id, cx: &mut Context<Self>) {
        let library = shot_audio::sound::Library::new(
            &self.themes.repo,
            Some(&self.themes.repo.join("brand")),
        );
        let Some(source) = library.find(&sound) else {
            self.error = Some(format!("{sound} is not in the sound folders any more"));
            cx.notify();
            return;
        };
        let audition = self
            .audition
            .get_or_insert_with(crate::sound::Audition::spawn);
        if audition.playing().as_ref() == Some(&sound) {
            audition.stop();
        } else {
            audition.play(sound, source);
        }
        cx.notify();
    }

    fn sounds_section(&mut self, cx: &mut Context<Self>) -> Div {
        let look = self.look();
        let mut out = div().flex().flex_col().gap(px(10.)).child(heading(
            "Sounds",
            "The sound each automatic effect plays, at its start or at a phase of its move.",
        ));
        for &effect in SoundEffect::ALL {
            let mut chips = div().flex().flex_wrap().gap(px(4.));
            for phase in std::iter::once(None).chain(Phase::ALL.iter().copied().map(Some)) {
                let key = SoundKey { effect, phase };
                let sound = look
                    .sounds
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, s)| s.clone());
                let on = self.picking == Some(key);
                let text = match &sound {
                    Some(s) => format!("{} {s}", phase_name(phase)),
                    None => phase_name(phase).to_owned(),
                };
                chips = chips.child(
                    chip(
                        SharedString::from(format!("sound-{}", key.to_text())),
                        text,
                        on || sound.is_some(),
                    )
                    .when(sound.is_none() && !on, |c| c.text_color(MUTED))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.picking = (this.picking != Some(key)).then_some(key);
                        cx.notify();
                    })),
                );
            }
            out = out.child(group(effect.as_str().to_owned())).child(chips);
            if let Some(key) = self.picking.filter(|k| k.effect == effect) {
                out = out.child(self.sound_picker(key, cx));
            }
        }
        out
    }

    fn sound_picker(&mut self, key: SoundKey, cx: &mut Context<Self>) -> Div {
        let playing = self
            .audition
            .as_ref()
            .and_then(crate::sound::Audition::playing);
        let mut list = div()
            .flex()
            .flex_wrap()
            .gap(px(4.))
            .p(px(8.))
            .rounded(RADIUS_MD)
            .bg(FLOAT);
        list = list.child(chip("sound-none", "None", false).on_click(cx.listener(
            move |this, _, _, cx| {
                this.picking = None;
                this.change(None, cx, |l| l.sounds.retain(|(k, _)| *k != key));
            },
        )));
        for sound in sounds(&self.themes.repo) {
            let (listen, pick) = (sound.clone(), sound.clone());
            let lit = playing.as_ref() == Some(&sound);
            list =
                list.child(
                    div()
                        .flex()
                        .items_center()
                        .rounded(RADIUS_SM)
                        .bg(RAISED)
                        .child(
                            div()
                                .id(SharedString::from(format!("listen-{sound}")))
                                .size(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|s| s.bg(FLOAT_ON))
                                .child(icon(
                                    if lit { "pause" } else { "play" },
                                    ICON_SM,
                                    if lit { ACCENT } else { MUTED },
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.audition(listen.clone(), cx)
                                })),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("pick-{sound}")))
                                .h(px(28.))
                                .flex()
                                .items_center()
                                .pr(px(10.))
                                .cursor_pointer()
                                .text_size(TEXT_SM)
                                .hover(|s| s.text_color(STRONG))
                                .child(sound.to_string())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let pick = pick.clone();
                                    this.picking = None;
                                    this.change(None, cx, move |l| {
                                        match l.sounds.binary_search_by_key(&key, |(k, _)| *k) {
                                            Ok(i) => l.sounds[i].1 = pick,
                                            Err(i) => l.sounds.insert(i, (key, pick)),
                                        }
                                    })
                                })),
                        ),
                );
        }
        list
    }

    // Intro and outro.

    fn bookends_section(&mut self, cx: &mut Context<Self>) -> Div {
        let look = self.look();
        let mut out = div().flex().flex_col().gap(px(10.)).child(heading(
            "Intro and outro",
            "A bumper video or Lottie in brand/media, a generated title with the logo, or both.",
        ));
        for (outro, title) in [(false, "Intro"), (true, "Outro")] {
            let bookend = if outro {
                look.outro.clone()
            } else {
                look.intro.clone()
            };
            let bumper = bookend.as_ref().and_then(|b| b.bumper.clone());
            let titled = bookend.as_ref().is_some_and(|b| b.title);
            let name = title.to_lowercase();
            out = out
                .child(group(title))
                .child(row(
                    "Bumper",
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .font_family(MONO)
                                .text_size(TEXT_SM)
                                .text_color(if bumper.is_some() { BODY } else { MUTED })
                                .child(bumper.clone().unwrap_or_else(|| "None".into())),
                        )
                        .child(
                            button(SharedString::from(format!("{name}-bumper")), "Choose…")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.pick_file(
                                        "Use as bumper",
                                        "media",
                                        cx,
                                        move |look, path| {
                                            let b = bookend_mut(look, outro);
                                            let title = b.as_ref().is_some_and(|b| b.title);
                                            *b = Some(Bookend {
                                                bumper: Some(path),
                                                title,
                                            });
                                        },
                                    )
                                })),
                        )
                        .when(bumper.is_some(), |d| {
                            d.child(
                                button(
                                    SharedString::from(format!("{name}-bumper-remove")),
                                    "Remove",
                                )
                                .text_color(MUTED)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.change(None, cx, |look| {
                                            let b = bookend_mut(look, outro);
                                            *b = b
                                                .take()
                                                .filter(|b| b.title)
                                                .map(|b| Bookend { bumper: None, ..b });
                                        })
                                    },
                                )),
                            )
                        }),
                ))
                .child(row(
                    "Title",
                    Switch::new(SharedString::from(format!("{name}-title")))
                        .checked(titled)
                        .accessibility_label("A generated title with the logo")
                        .on_click(cx.listener(move |this, on: &bool, _, cx| {
                            let on = *on;
                            this.change(None, cx, |look| {
                                let b = bookend_mut(look, outro);
                                let bumper = b.as_ref().and_then(|b| b.bumper.clone());
                                *b = (on || bumper.is_some())
                                    .then_some(Bookend { bumper, title: on });
                            })
                        })),
                ));
        }
        out.child(div().text_size(TEXT_XS).text_color(MUTED).child(
            "Bookends play in the export and the Edit preview; this preview shows the end card",
        ))
    }

    // End card.

    fn end_card_typed(
        this: &mut Self,
        key: &Key,
        text: &str,
        _: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = match text.trim() {
            "" => None,
            t => match parse_seconds(t).filter(|&t| t >= 1_000_000) {
                Some(t) => Some(Micros(t)),
                None => return,
            },
        };
        this.change(Some(key.clone()), cx, |look| look.end_card = value);
    }

    fn end_card_section(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let length = self
            .look()
            .end_card
            .map(|l| seconds(l.0))
            .unwrap_or_default();
        let state = self.inputs.get(
            Key::EndCard,
            &length,
            "none",
            window,
            cx,
            Self::end_card_typed,
        );
        let typed = state.read(cx).value().to_string();
        let refused =
            !typed.trim().is_empty() && parse_seconds(&typed).is_none_or(|t| t < 1_000_000);
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(heading(
                "End card",
                "The end screen over the last seconds: a headline, room for two videos and the subscribe button, in the brand's background.",
            ))
            .child(row("Length", fields::input(&state, refused)))
            .child(
                div()
                    .pl(px(100.))
                    .text_size(TEXT_XS)
                    .text_color(MUTED)
                    .child("Seconds, at least 1. Empty for no end card. YouTube allows 5 to 20"),
            )
    }

    // Overlay presets.

    fn overlays_section(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let selected = self.preset.as_ref().and_then(|(n, _)| n.clone());
        let presets = self.themes.brand.overlay_presets.clone();
        let mut list = div().flex().flex_col().gap(px(2.));
        for preset in &presets {
            let on = selected.as_ref() == Some(&preset.name);
            let name = preset.name.clone();
            list = list.child(crate::ui::keyboard_reachable(
                div()
                    .id(SharedString::from(format!("preset-{}", preset.name)))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(30.))
                    .px(px(10.))
                    .rounded(RADIUS_SM)
                    .bg(if on { RAISED } else { SURFACE })
                    .hover(|s| s.bg(RAISED))
                    .cursor_pointer()
                    .text_size(TEXT_SM)
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_color(if on { STRONG } else { BODY })
                            .child(preset.name.to_string()),
                    )
                    .child(
                        div()
                            .text_size(TEXT_XS)
                            .text_color(MUTED)
                            .child(preset.template.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_preset(Some(name.clone()), cx)
                    })),
            ));
        }
        let look = self.shown_brand().unwrap_or_default();
        let add = div().flex().flex_wrap().gap(px(4.)).children(
            super::overlay::templates(&look)
                .into_iter()
                .map(|template| {
                    let t = template.clone();
                    chip(
                        SharedString::from(format!("new-preset-{template}")),
                        format!("+ {template}"),
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.new_preset(&t, cx)))
                }),
        );
        let mut out = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(heading(
                "Overlay presets",
                "Overlays you place often, with their text and timing. The Edit screen places them from its Overlays pane. They belong to the channel, not to a theme.",
            ))
            .children(presets.is_empty().then(|| {
                div().text_size(TEXT_SM).text_color(MUTED).child("No presets yet. Start one from a template below.")
            }))
            .child(list)
            .child(group("New preset from a template"))
            .child(add);
        if let Some((_, editor)) = self.preset.clone() {
            let controls = editor.update(cx, |editor, cx| editor.controls(window, cx));
            out = out
                .child(div().h(px(1.)).my(px(6.)).bg(RAISED))
                .child(controls)
                .child(
                    button("preset-delete", "Delete preset")
                        .text_color(CUT_TEXT)
                        .on_click(cx.listener(|this, _, _, cx| this.delete_preset(cx))),
                );
        }
        out
    }

    // Files.

    /// Asks for a file, copies it into `brand/<folder>/` (the brand folder for an empty one) and
    /// sets it on the look with `set`, by its path in the repo.
    fn pick_file(
        &mut self,
        prompt: &'static str,
        folder: &'static str,
        cx: &mut Context<Self>,
        set: impl FnOnce(&mut shot_core::Brand, String) + 'static,
    ) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(prompt.into()),
        });
        let repo = self.themes.repo.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(source) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| match copy_into(&repo, folder, &source) {
                Ok(path) => this.change(None, cx, |look| set(look, path)),
                Err(error) => {
                    this.error = Some(error);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

/// Copies `source` into the channel's `brand/<folder>/` unless it is in the repo already, and
/// returns its path in the repo.
fn copy_into(repo: &Path, folder: &str, source: &Path) -> Result<String, String> {
    let repo = repo.canonicalize().unwrap_or_else(|_| repo.to_owned());
    let source = source
        .canonicalize()
        .map_err(|e| format!("{}: {e}", source.display()))?;
    if let Ok(inside) = source.strip_prefix(&repo) {
        return Ok(inside.to_string_lossy().into_owned());
    }
    let name = source.file_name().ok_or("That is not a file")?;
    let dir: PathBuf = repo.join("brand").join(folder);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let target = dir.join(name);
    std::fs::copy(&source, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    Ok(target
        .strip_prefix(&repo)
        .expect("copied into the repo")
        .to_string_lossy()
        .into_owned())
}

const FONT_IDS: [&str; 3] = ["font-heading", "font-body", "font-mono"];
const MOTION_IDS: [&str; 4] = [
    "motion-reaction",
    "motion-opinion",
    "motion-tour",
    "motion-tutorial",
];

/// One value of a few as the app's segmented control; picking one sets it on the look.
fn choice<T: Copy + PartialEq + 'static>(
    id: &'static str,
    options: &[(T, &'static str)],
    current: Option<T>,
    cx: &mut Context<ThemeEditor>,
    set: impl Fn(&mut shot_core::Brand, T) + Clone + 'static,
) -> gpui::Stateful<Div> {
    let entity = cx.entity();
    let segments = options
        .iter()
        .map(|&(value, label)| {
            let (entity, set) = (entity.clone(), set.clone());
            Segment::new(label, current == Some(value), move |_, _, cx| {
                let set = set.clone();
                entity.update(cx, |this, cx| {
                    this.change(None, cx, move |look| set(look, value))
                })
            })
        })
        .collect();
    segmented(id, WELL, segments)
}

const DEFAULT_CAMERA: Camera = Camera {
    shape: CameraShape::Rounded,
    corners: 24,
    border: 0,
    shadow: Shadow::Soft,
};

fn rgb(shot_core::Color([r, g, b]): shot_core::Color) -> u32 {
    u32::from_be_bytes([0, r, g, b])
}

fn set_font(look: &mut shot_core::Brand, role: &str, font: Option<Font>) {
    let f = &mut look.fonts;
    *match role {
        "heading" => &mut f.heading,
        "body" => &mut f.body,
        _ => &mut f.mono,
    } = font;
}

fn set_motion(motion: &mut shot_core::Motion, format: FormatKind, value: Option<PresetName>) {
    *match format {
        FormatKind::Reaction => &mut motion.reaction,
        FormatKind::Opinion => &mut motion.opinion,
        FormatKind::ProductTour => &mut motion.product_tour,
        FormatKind::Tutorial => &mut motion.tutorial,
    } = value;
}

/// The brand's feel entry of `name`, added empty if missing.
fn preset_entry(look: &mut shot_core::Brand, name: PresetName) -> &mut shot_core::Preset {
    let index = match look.presets.binary_search_by_key(&name, |p| p.name) {
        Ok(i) => i,
        Err(i) => {
            look.presets.insert(
                i,
                shot_core::Preset {
                    name,
                    tempo: None,
                    bounce: None,
                    density: None,
                    stagger: None,
                    beat_sync: None,
                },
            );
            i
        }
    };
    &mut look.presets[index]
}

fn bookend_mut(look: &mut shot_core::Brand, outro: bool) -> &mut Option<Bookend> {
    if outro {
        &mut look.outro
    } else {
        &mut look.intro
    }
}
