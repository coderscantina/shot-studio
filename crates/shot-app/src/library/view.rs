//! How the Library window looks: sections on the left, the search field over the results in
//! the middle, the selected item with its license, source and actions on the right.

use std::time::Duration;

use gpui::{
    AnyElement, Context, Div, Focusable, FontWeight, Hsla, ObjectFit, Render, SharedString,
    Stateful, Window, div, img, prelude::*, px, relative,
};
use gpui_component::input::Input;
use shot_library::{Asset, Media};

use super::items::{Item, Section};
use super::*;
use crate::theme::*;
use crate::ui::{self, ActionLook, kbd, label, mono};

/// The image grid's tiles.
pub const TILE_WIDTH: f32 = 176.;
pub const TILE_GAP: f32 = 10.;

/// The child of the list that shows item `index` of `visible`: section headers come before
/// each section's first item when the list is `grouped`.
pub fn child_index(visible: &[Item], index: usize, grouped: bool) -> usize {
    if !grouped {
        return index;
    }
    let headers = visible[..=index.min(visible.len().saturating_sub(1))]
        .iter()
        .enumerate()
        .filter(|(i, item)| *i == 0 || visible[i - 1].section() != item.section())
        .count();
    index + headers
}

fn chip(text: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .px(px(7.))
        .py(px(1.))
        .rounded(px(6.))
        .bg(KBD)
        .text_size(TEXT_XS)
        .text_color(KBD_TEXT)
        .child(text.into())
}

fn seconds(duration: Duration) -> String {
    let s = duration.as_secs_f32();
    if s < 60.0 {
        format!("{s:.1} s")
    } else {
        format!("{}:{:02}", s as u64 / 60, s as u64 % 60)
    }
}

impl LibraryWindow {
    fn thumb_path(&self, item: &Item) -> Option<std::path::PathBuf> {
        match item {
            Item::Asset(asset) if asset.section == shot_library::Section::Images => {
                asset.path.clone()
            }
            Item::Online(hit) => match self.thumbs.get(&hit.id) {
                Some(Fetch::Ready(path)) => Some(path.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// The large preview of an image: the full file once fetched, else the thumbnail.
    fn large_path(&self, item: &Item) -> Option<std::path::PathBuf> {
        match item {
            Item::Online(hit) => match self.files.get(&hit.id) {
                Some(Fetch::Ready(path)) => Some(path.clone()),
                _ => self.thumb_path(item),
            },
            _ => self.thumb_path(item),
        }
    }

    fn progress(&self, item: &Item) -> Option<&Progress> {
        match item {
            Item::Online(hit) => self.downloads.get(&hit.id),
            _ => None,
        }
    }

    fn is_playing(&self, item: &Item) -> bool {
        let key = item.key();
        self.listening.as_ref() == Some(&key)
            || self
                .audition
                .as_ref()
                .and_then(Audition::playing)
                .is_some_and(|id| id == audition_id(&key))
    }

    fn select(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(key);
        self.note = None;
        window.focus(&self.list_focus, cx);
        cx.notify();
    }

    fn title_bar(&self, cx: &App) -> Div {
        let target = self.target(cx);
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
                    .child(self.title.clone()),
            )
            .child(div().flex_1())
            .child(match target {
                Some(target) => div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child("Places into")
                    .child(div().text_color(TEXT).child(target.title))
                    .child(if target.selected {
                        "at the selected words,"
                    } else {
                        "at the playhead,"
                    })
                    .child(mono(target.at).text_color(TEXT)),
                None => div()
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child("Press ⌘L in a video's studio to place into it"),
            })
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> Div {
        let keys = ["⌘0", "⌘1", "⌘2", "⌘3", "⌘4", "⌘5"];
        let rows = Section::ALL.into_iter().zip(keys).map(|(section, keys)| {
            let on = self.section == section;
            let count = self.count(section);
            div()
                .id(SharedString::from(format!("section-{}", section.label())))
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(36.))
                .px(px(10.))
                .rounded(RADIUS_SM)
                .cursor_pointer()
                .text_size(TEXT_SM)
                .text_color(if on { STRONG } else { BODY })
                .when(on, |this| this.bg(RAISED))
                .when(!on, |this| this.hover(|s| s.bg(CARD)))
                .child(ui::icon(
                    section.icon(),
                    ICON,
                    if on { ACCENT } else { MUTED },
                ))
                .child(div().flex_1().child(section.label()))
                .child(
                    div()
                        .font_family(MONO)
                        .text_size(TEXT_XS)
                        .text_color(if count == 0 { FAINT } else { MUTED })
                        .child(count.to_string()),
                )
                .child(kbd(keys))
                .on_click(cx.listener(move |this, _, _, cx| this.show(section, cx)))
        });
        let credits = self.credits.len();
        ui::pane()
            .w(px(232.))
            .flex_none()
            .flex()
            .flex_col()
            .p(px(8.))
            .gap(px(2.))
            .children(rows)
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .p(px(10.))
                    .rounded(RADIUS_MD)
                    .bg(CARD)
                    .child(label("Credits"))
                    .child(
                        div()
                            .text_size(TEXT_SM)
                            .text_color(BODY)
                            .child(match credits {
                                0 => "The cut uses no downloads yet".to_owned(),
                                n => format!(
                                    "The cut uses {} to credit in the description",
                                    plural(n, "download")
                                ),
                            }),
                    )
                    .children(self.credits.iter().take(4).map(|c| {
                        div()
                            .text_size(TEXT_XS)
                            .text_color(MUTED)
                            .truncate()
                            .child(format!("{}, {}", c.title, c.license_name()))
                    }))
                    .child(
                        ui::proposal_action(
                            "copy-credits",
                            "Copy credits",
                            Some("copy"),
                            Some("⌘⇧C"),
                            ActionLook::Plain,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.copy_credits(&CopyCredits, window, cx)
                        })),
                    ),
            )
    }

    fn search_bar(&self, cx: &mut Context<Self>) -> Div {
        let online = self.section.online();
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .h(px(40.))
                            .px(px(12.))
                            .rounded(RADIUS_MD)
                            .bg(WELL)
                            .child(ui::icon("search", ICON, MUTED))
                            .child(
                                div()
                                    .flex_1()
                                    .child(Input::new(&self.input).appearance(false)),
                            )
                            .child(kbd("⌘F")),
                    )
                    .when_some(online, |this, _| {
                        let local = cx.listener(|this, _, _, cx| this.set_online(false, cx));
                        let web = cx.listener(|this, _, _, cx| this.set_online(true, cx));
                        this.child(
                            ui::segmented(
                                "scope",
                                WELL,
                                vec![
                                    ui::Segment::new("On disk", !self.online, local).icon("folder"),
                                    ui::Segment::new("Openverse", self.online, web).icon("search"),
                                ],
                            )
                            .w(px(250.)),
                        )
                        .child(kbd("tab"))
                    }),
            )
            .when(self.showing_online().is_some(), |this| {
                this.child(div().text_size(TEXT_XS).text_color(MUTED).child(
                    "Searches go to Openverse (api.openverse.org) as you type. Every \
                             result allows monetized videos: CC0, public domain or CC BY.",
                ))
            })
    }

    fn empty(&self) -> Option<Div> {
        if !self.visible().is_empty() {
            return None;
        }
        let quoted = format!("\u{201c}{}\u{201d}", self.query);
        Some(match (self.showing_online(), &self.search()) {
            (Some(media), _) if self.query.is_empty() => ui::empty_state(
                "search",
                "Search Openverse",
                match media {
                    Media::Audio => {
                        "Type a sound, like whoosh, applause or keyboard. Space plays a result, Enter downloads it into the channel."
                    }
                    Media::Image => {
                        "Type a meme or a picture, like facepalm or rocket. Space shows it large, Enter downloads it into the channel."
                    }
                },
            ),
            (Some(_), Online::Searching | Online::Idle) => {
                ui::empty_state("search", "Searching Openverse", quoted)
            }
            (Some(_), Online::Failed { offline: true, .. }) => ui::empty_state(
                "offline",
                "You're offline",
                "Openverse results show here once you're connected. What's on disk is one Tab away.",
            ),
            (Some(_), Online::Failed { message, .. }) => ui::empty_state(
                "alert",
                "Openverse didn't answer",
                format!("{message}. Press Enter in the search field to try again."),
            ),
            (Some(_), Online::Ready) => ui::empty_state(
                "search",
                "Nothing on Openverse",
                format!("No sounds or images allowed in monetized videos match {quoted}"),
            ),
            (None, _) => ui::empty_state(
                "search",
                format!("Nothing matches {quoted}"),
                match self.section.online() {
                    Some(_) => "Press Tab to search Openverse for it.",
                    None => "Try another word, or look in Everything (⌘0).",
                },
            ),
        })
    }

    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(empty) = self.empty() {
            return empty.into_any_element();
        }
        let visible = self.visible();
        let selected = self.selected_item().map(|i| i.key());
        let grouped = self.section == Section::All;
        let grid = self.section == Section::Images;
        let mut list = div().flex().gap(px(if grid { TILE_GAP } else { 2. }));
        list = if grid {
            list.flex_row().flex_wrap().content_start()
        } else {
            list.flex_col()
        };
        for (index, item) in visible.iter().enumerate() {
            if grouped && (index == 0 || visible[index - 1].section() != item.section()) {
                list = list.child(
                    label(item.section().section_label())
                        .pt(px(if index == 0 { 4. } else { 14. }))
                        .pb(px(4.))
                        .px(px(10.)),
                );
            }
            let on = selected.as_ref() == Some(&item.key());
            list = list.child(if grid {
                self.tile(index, item, on, cx).into_any_element()
            } else {
                self.row(index, item, on, cx).into_any_element()
            });
        }
        if self.loading_more() {
            list = list.child(
                div()
                    .w_full()
                    .py(px(12.))
                    .text_center()
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child("Loading more…"),
            );
        }
        if grid {
            list = list.child(ui::measure(self.grid.clone()).absolute().size_full());
        }
        ui::scrolling_list("library-list", &self.scroll, list.relative().pr(px(4.)))
            .into_any_element()
    }

    fn lead(&self, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        if item.audible() {
            let playing = self.is_playing(item);
            let key = item.key();
            let clicked = item.clone();
            return div()
                .id(SharedString::from(format!("play-{key}")))
                .flex_none()
                .size(px(32.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .bg(if playing { ACCENT } else { FLOAT })
                .hover(|s| s.opacity(0.85))
                .child(ui::icon(
                    if playing { "pause" } else { "play" },
                    ICON_SM,
                    if playing { ON_ACCENT } else { TEXT },
                ))
                .on_click(cx.listener(move |this, _, _, cx| this.play(&clicked, cx)))
                .into_any_element();
        }
        let well = div()
            .flex_none()
            .size(px(32.))
            .rounded(RADIUS_SM)
            .bg(WELL)
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center();
        match (self.thumb_path(item), item) {
            (Some(path), _) => well
                .child(img(path).size_full().object_fit(ObjectFit::Cover))
                .into_any_element(),
            (None, Item::Emoji(emoji)) => well.text_size(px(18.)).child(*emoji).into_any_element(),
            (None, item) => well
                .child(ui::icon(item.section().icon(), ICON_SM, MUTED))
                .into_any_element(),
        }
    }

    fn row(&self, index: usize, item: &Item, on: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let key = item.key();
        let progress = self.progress(item).cloned();
        let downloaded = matches!(item, Item::Asset(a) if a.credit.is_some());
        div()
            .id(("library-row", index))
            .flex()
            .flex_col()
            .rounded(RADIUS_SM)
            .cursor_pointer()
            .when(on, |this| this.bg(FLOAT_ON))
            .when(!on, |this| this.hover(|s| s.bg(RAISED)))
            .child(
                div()
                    .h(px(52.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(self.lead(item, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(180.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(TEXT_SM)
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if on { STRONG } else { TEXT })
                                    .child(item.title()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(TEXT_XS)
                                    .text_color(MUTED)
                                    .child(item.about()),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink(1.)
                            .min_w_0()
                            .max_w(px(200.))
                            .overflow_hidden()
                            .flex()
                            .gap(px(4.))
                            .children(item.tags().into_iter().take(2).map(chip)),
                    )
                    .when_some(item.duration(), |this, d| {
                        this.child(
                            mono(seconds(d))
                                .w(px(52.))
                                .text_right()
                                .text_size(TEXT_XS)
                                .text_color(MUTED),
                        )
                    })
                    .child(chip(item.license()).bg(RAISED).text_color(BODY))
                    .child(
                        div()
                            .w(px(92.))
                            .truncate()
                            .text_size(TEXT_XS)
                            .text_color(MUTED)
                            .child(item.source()),
                    )
                    .child(div().w(px(18.)).flex_none().when(downloaded, |this| {
                        this.child(ui::icon("check", ICON_SM, ACCENT))
                    })),
            )
            .when_some(progress, |this, progress| this.child(bar(&progress)))
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    this.select(key.clone(), window, cx);
                    if event.click_count() > 1
                        && let Some(item) = this.selected_item()
                    {
                        this.place(&item, false, cx);
                    }
                }),
            )
    }

    fn tile(&self, index: usize, item: &Item, on: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let key = item.key();
        let progress = self.progress(item).cloned();
        let downloaded = matches!(item, Item::Asset(a) if a.credit.is_some());
        div()
            .id(("library-tile", index))
            .w(px(TILE_WIDTH))
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(6.))
            .rounded(RADIUS_MD)
            .cursor_pointer()
            .bg(if on { FLOAT_ON } else { CARD })
            .border_1()
            .border_color(if on {
                Hsla::from(ACCENT)
            } else {
                gpui::transparent_black()
            })
            .hover(|s| s.bg(RAISED))
            .child(
                div()
                    .relative()
                    .h(px(118.))
                    .w_full()
                    .rounded(RADIUS_SM)
                    .bg(WELL)
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(match self.thumb_path(item) {
                        Some(path) => img(path)
                            .size_full()
                            .object_fit(ObjectFit::Cover)
                            .into_any_element(),
                        None => ui::icon("image", ICON, FAINT).into_any_element(),
                    })
                    .when(downloaded, |this| {
                        this.child(
                            div()
                                .absolute()
                                .top(px(6.))
                                .right(px(6.))
                                .size(px(22.))
                                .rounded_full()
                                .bg(ACCENT)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(ui::icon("check", ICON_SM, ON_ACCENT)),
                        )
                    }),
            )
            .child(
                div()
                    .truncate()
                    .text_size(TEXT_XS)
                    .font_weight(FontWeight::MEDIUM)
                    .child(item.title()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(chip(item.license()))
                    .child(
                        div()
                            .truncate()
                            .text_size(TEXT_XS)
                            .text_color(MUTED)
                            .child(item.source()),
                    ),
            )
            .when_some(progress, |this, progress| this.child(bar(&progress)))
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    this.select(key.clone(), window, cx);
                    if event.click_count() > 1
                        && let Some(item) = this.selected_item()
                    {
                        this.play(&item, cx);
                    }
                }),
            )
    }

    fn inspector(&self, cx: &mut Context<Self>) -> Div {
        let pane = ui::pane()
            .w(px(330.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.));
        let Some(item) = self.selected_item() else {
            return pane.child(
                div()
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child("Pick something to see its license and where it comes from"),
            );
        };
        let preview = div()
            .h(px(186.))
            .w_full()
            .rounded(RADIUS_MD)
            .bg(WELL)
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center();
        let preview = match (self.large_path(&item), &item) {
            (Some(path), _) if item.still() => {
                preview.child(img(path).size_full().object_fit(ObjectFit::Contain))
            }
            (_, Item::Emoji(emoji)) => preview.text_size(px(64.)).child(*emoji),
            (_, item) if item.audible() => preview.child(self.lead(item, cx)).child(
                div()
                    .ml(px(12.))
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child(item.duration().map_or("Space plays it".into(), seconds)),
            ),
            (_, item) => preview.child(ui::icon(item.section().icon(), px(36.), FAINT)),
        };
        let credit = match &item {
            Item::Asset(asset) => asset.credit.clone(),
            Item::Online(hit) => Some(hit.credit()),
            _ => None,
        };
        let source_url = credit.as_ref().map(|c| c.source.clone());
        let file = match &item {
            Item::Asset(Asset {
                path: Some(path), ..
            }) => Some(self.folders.shown(path)),
            _ => None,
        };
        let mut actions = div().flex().flex_wrap().gap(px(6.));
        let online = matches!(item, Item::Online(_));
        if item.audible() {
            actions = actions.child(
                action(
                    "listen",
                    if self.is_playing(&item) {
                        "Stop"
                    } else {
                        "Play"
                    },
                    if self.is_playing(&item) {
                        "pause"
                    } else {
                        "play"
                    },
                    "Space",
                )
                .on_click(cx.listener(|this, _, window, cx| this.listen(&Listen, window, cx))),
            );
        }
        if item.still() {
            actions = actions.child(
                action(
                    "large",
                    if self.large { "Smaller" } else { "Larger" },
                    "image",
                    "Space",
                )
                .on_click(cx.listener(|this, _, window, cx| this.listen(&Listen, window, cx))),
            );
        }
        let downloading = self.progress(&item).is_some();
        if online {
            let (text, icon, look) = if downloading {
                ("Stop", "close", ActionLook::Plain)
            } else {
                ("Download", "download", ActionLook::Accent)
            };
            actions = actions.child(
                ui::proposal_action("download", text, Some(icon), Some("D"), look).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.download_selected(&Download, window, cx)
                    }),
                ),
            );
        }
        let placeable = !matches!(item, Item::Template(_))
            && !matches!(
                item,
                Item::Effect(items::EffectItem::Transition(_) | items::EffectItem::Captions(_))
            );
        if placeable {
            let text = match item.section() {
                Section::Images => "Pop in",
                Section::Music => "Use as music",
                _ => "Place",
            };
            actions = actions.child(
                ui::proposal_action(
                    "place",
                    text,
                    None,
                    Some("P"),
                    if online {
                        ActionLook::Plain
                    } else {
                        ActionLook::Accent
                    },
                )
                .on_click(
                    cx.listener(|this, _, window, cx| this.place_selected(&Place, window, cx)),
                ),
            );
        }
        if item.still() {
            actions = actions.child(
                ui::proposal_action("cutaway", "Cutaway", None, Some("B"), ActionLook::Plain)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.cutaway_selected(&Cutaway, window, cx)
                    })),
            );
        }
        if let Item::Template(spec) = &item {
            let name = spec.name;
            actions = actions.child(
                ui::proposal_action(
                    "edit-overlay",
                    "Edit",
                    Some("edit"),
                    Some("P"),
                    ActionLook::Plain,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.edit_overlay(name, cx))),
            );
        }
        pane.child(preview)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(item.title()),
                    )
                    .child(
                        div()
                            .text_size(TEXT_SM)
                            .text_color(BODY)
                            .child(item.about()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(chip(item.license()).bg(RAISED).text_color(TEXT))
                    .child(
                        div()
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                            .child(format!("via {}", item.source())),
                    )
                    .when_some(source_url, |this, url| {
                        this.child(div().flex_1()).child(
                            div()
                                .id("source-link")
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .cursor_pointer()
                                .text_size(TEXT_SM)
                                .text_color(MUTED)
                                .hover(|s| s.text_color(TEXT))
                                .child(ui::icon("link", ICON_SM, MUTED))
                                .child("Source")
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                    }),
            )
            .when_some(
                credit.filter(|c| c.license.needs_attribution()),
                |this, c| {
                    this.child(
                        div()
                            .p(px(10.))
                            .rounded(RADIUS_SM)
                            .bg(CARD)
                            .text_size(TEXT_XS)
                            .text_color(BODY)
                            .child(format!(
                                "Needs a credit in the description: {}",
                                c.attribution
                            )),
                    )
                },
            )
            .when(!item.tags().is_empty(), |this| {
                this.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .children(item.tags().into_iter().take(10).map(chip)),
                )
            })
            .when_some(file, |this, file| {
                this.child(mono(file).text_size(TEXT_XS).text_color(MUTED))
            })
            .when_some(self.progress(&item).cloned(), |this, progress| {
                let text = match progress.fraction() {
                    Some(f) => format!("Downloading, {:.0}%", f * 100.0),
                    None => "Downloading…".to_owned(),
                };
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .text_size(TEXT_SM)
                        .child(text)
                        .child(bar(&progress)),
                )
            })
            .child(actions)
            .child(div().flex_1())
            .when_some(self.note.clone(), |this, note| {
                this.child(
                    div()
                        .p(px(10.))
                        .rounded(RADIUS_SM)
                        .bg(if note.error {
                            Hsla::from(RECORD).opacity(0.1)
                        } else {
                            Hsla::from(CARD)
                        })
                        .text_size(TEXT_SM)
                        .text_color(if note.error { RECORD_TEXT } else { BODY })
                        .child(note.text),
                )
            })
    }
}

fn action(
    id: &'static str,
    text: &'static str,
    icon: &'static str,
    keys: &'static str,
) -> Stateful<Div> {
    ui::proposal_action(id, text, Some(icon), Some(keys), ActionLook::Plain)
}

/// A thin progress bar, full width.
fn bar(progress: &Progress) -> Div {
    let fraction = progress.fraction().unwrap_or(0.1).clamp(0.02, 1.0);
    div()
        .mx(px(10.))
        .mb(px(6.))
        .h(px(3.))
        .rounded_full()
        .bg(WELL)
        .child(
            div()
                .h_full()
                .w(relative(fraction))
                .rounded_full()
                .bg(ACCENT),
        )
}

impl Section {
    fn section_label(self) -> &'static str {
        self.label()
    }
}

impl Render for LibraryWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .key_context(WINDOW)
            .bg(FRAME)
            .text_color(TEXT)
            .font_family(SANS)
            .text_size(TEXT_BASE)
            .on_action(cx.listener(Self::focus_search))
            .on_action(cx.listener(Self::toggle_online))
            .on_action(cx.listener(Self::copy_credits))
            .on_action(cx.listener(|this, _: &ShowAll, _, cx| this.show(Section::All, cx)))
            .on_action(cx.listener(|this, _: &ShowSounds, _, cx| this.show(Section::Sounds, cx)))
            .on_action(cx.listener(|this, _: &ShowMusic, _, cx| this.show(Section::Music, cx)))
            .on_action(cx.listener(|this, _: &ShowImages, _, cx| this.show(Section::Images, cx)))
            .on_action(
                cx.listener(|this, _: &ShowOverlays, _, cx| this.show(Section::Overlays, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowEffects, _, cx| this.show(Section::Effects, cx)))
            .on_action(cx.listener(|_, _: &CloseLibrary, window, _| window.remove_window()))
            // Down in the search field moves into the results.
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "down"
                    && !this.list_focus.is_focused(window)
                    && this.input.read(cx).focus_handle(cx).is_focused(window)
                {
                    this.focus_list(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(self.title_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(8.))
                    .px(px(8.))
                    .pb(px(8.))
                    .child(self.sidebar(cx))
                    .child(
                        ui::pane()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(12.))
                            .p(px(12.))
                            .child(self.search_bar(cx))
                            .child(
                                div()
                                    .id("library-results")
                                    .flex_1()
                                    .min_h_0()
                                    .flex()
                                    .flex_col()
                                    .key_context(LIST)
                                    .track_focus(&self.list_focus)
                                    .on_action(cx.listener(|this, _: &SelectNext, _, cx| {
                                        let by = this.columns() as isize;
                                        this.step(by, cx)
                                    }))
                                    .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| {
                                        let by = this.columns() as isize;
                                        this.step(-by, cx)
                                    }))
                                    .on_action(
                                        cx.listener(|this, _: &SelectRight, _, cx| {
                                            this.step(1, cx)
                                        }),
                                    )
                                    .on_action(
                                        cx.listener(|this, _: &SelectLeft, _, cx| {
                                            this.step(-1, cx)
                                        }),
                                    )
                                    .on_action(cx.listener(Self::listen))
                                    .on_action(cx.listener(Self::primary))
                                    .on_action(cx.listener(Self::download_selected))
                                    .on_action(cx.listener(Self::place_selected))
                                    .on_action(cx.listener(Self::cutaway_selected))
                                    .child(self.list(cx)),
                            ),
                    )
                    .child(self.inspector(cx)),
            )
    }
}
