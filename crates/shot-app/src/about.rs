//! The About window and the app menu that opens it: the app's license, then the credits for
//! every bundled asset, vendored library and Rust crate. Assets and libraries come from
//! `sources/manifest.json`, crates from `assets/crates.txt`, which `scripts/licenses.sh`
//! generates with the full texts in `THIRD-PARTY-LICENSES.md`.

use gpui::{
    App, Bounds, Context, FontWeight, KeyBinding, Menu, MenuItem, TitlebarOptions, Window,
    WindowBounds, WindowOptions, actions, div, point, prelude::*, px, size,
};
use serde::Deserialize;

use crate::theme::*;
use crate::ui::{label, pane};

actions!(shot_app, [About, Quit]);

#[derive(Deserialize)]
struct Manifest {
    sources: Vec<Source>,
}

/// A pinned source from `sources/manifest.json`.
#[derive(Deserialize)]
struct Source {
    name: String,
    license: String,
    attribution: Option<String>,
    repository: String,
    contributes: String,
    /// Set on sources that are Rust crates, which are credited with the other crates.
    #[serde(rename = "crate")]
    krate: Option<String>,
}

/// The crates under one license, as a comma separated list.
struct CrateGroup {
    license: &'static str,
    crates: &'static str,
}

struct Credits {
    sources: Vec<Source>,
    crates: Vec<CrateGroup>,
}

fn credits() -> Credits {
    let manifest: Manifest = serde_json::from_str(include_str!("../../../sources/manifest.json"))
        .expect("sources/manifest.json parses");
    let mut sources: Vec<Source> = manifest
        .sources
        .into_iter()
        .filter(|source| source.krate.is_none())
        .collect();
    // Sources whose license asks for a named attribution come first.
    sources.sort_by_key(|source| source.attribution.is_none());
    Credits {
        sources,
        crates: include_str!("../assets/crates.txt")
            .lines()
            .filter_map(|line| line.split_once(": "))
            .map(|(license, crates)| CrateGroup { license, crates })
            .collect(),
    }
}

/// Sets the app menu with About and Quit (⌘Q). Call once at startup.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &About, cx| open(cx));
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
    cx.set_menus([Menu::new("Shot Studio").items([
        MenuItem::action("About Shot Studio", About),
        MenuItem::separator(),
        MenuItem::action("Quit Shot Studio", Quit),
    ])]);
}

/// Opens the About window, or brings the open one to the front. Needs [`crate::theme::init`].
pub fn open(cx: &mut App) {
    if let Some(window) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<AboutView>())
    {
        window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        return;
    }
    let bounds = Bounds::centered(None, size(px(680.), px(760.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(520.), px(480.))),
            titlebar: Some(TitlebarOptions {
                title: Some("About Shot Studio".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(16.), px(16.))),
            }),
            ..Default::default()
        },
        |_, cx| cx.new(|_| AboutView { credits: credits() }),
    )
    .ok();
    cx.activate(true);
}

pub struct AboutView {
    credits: Credits,
}

impl AboutView {
    fn source(source: &Source) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .py(px(10.))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(TEXT)
                            .child(source.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                            .child(source.license.clone()),
                    ),
            )
            .when_some(source.attribution.clone(), |this, attribution| {
                this.child(div().text_color(BODY).child(attribution))
            })
            .child(
                div()
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child(source.contributes.clone()),
            )
            .child(
                div()
                    .font_family(MONO)
                    .text_size(TEXT_XS)
                    .text_color(FAINT)
                    .child(source.repository.clone()),
            )
    }

    fn crates(group: &CrateGroup) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .py(px(8.))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(TEXT)
                    .child(group.license),
            )
            .child(
                div()
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child(group.crates),
            )
    }
}

impl Render for AboutView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let version = env!("CARGO_PKG_VERSION").replace('-', ".");
        div()
            .size_full()
            .bg(FRAME)
            .font_family(SANS)
            .text_size(TEXT_BASE)
            .pt(px(44.))
            .px(px(12.))
            .pb(px(12.))
            .child(
                pane()
                    .id("about")
                    .size_full()
                    .overflow_y_scroll()
                    .px(px(24.))
                    .py(px(20.))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(10.))
                            .child(
                                div()
                                    .text_size(px(22.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(STRONG)
                                    .child("Shot Studio"),
                            )
                            .child(
                                div()
                                    .font_family(MONO)
                                    .text_size(TEXT_SM)
                                    .text_color(MUTED)
                                    .child(version),
                            ),
                    )
                    .child(
                        div()
                            .pt(px(4.))
                            .text_color(BODY)
                            .child("MIT License, Coder's Cantina. Source and full license texts at github.com/coderscantina/shot-studio."),
                    )
                    .child(div().pt(px(20.)).child(label("Bundled assets and libraries")))
                    .children(self.credits.sources.iter().map(Self::source))
                    .child(div().pt(px(20.)).child(label("Rust crates")))
                    .children(self.credits.crates.iter().map(Self::crates))
                    .child(
                        div()
                            .pt(px(16.))
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                            .child("Every license text is in THIRD-PARTY-LICENSES.md, in the app's Contents/Resources."),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credits_carry_the_emoji_attribution_and_every_pinned_crate() {
        let credits = credits();
        let noto = credits
            .sources
            .iter()
            .find(|source| source.name == "Noto Animated Emoji")
            .expect("the Noto emoji are credited");
        assert_eq!(
            noto.attribution.as_deref(),
            Some("Noto Animated Emoji by Google, CC BY 4.0")
        );
        let manifest: Manifest =
            serde_json::from_str(include_str!("../../../sources/manifest.json")).unwrap();
        for pinned in manifest
            .sources
            .iter()
            .filter_map(|source| source.krate.as_deref())
        {
            let name = pinned.split_whitespace().next().unwrap();
            assert!(
                credits
                    .crates
                    .iter()
                    .any(|group| group.crates.split(", ").any(|krate| krate == name)),
                "{name} is missing from assets/crates.txt"
            );
        }
    }
}
