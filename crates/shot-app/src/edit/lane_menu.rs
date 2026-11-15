//! The Timeline lanes' right-click menu. A right-click selects the word or picks the block
//! under it first, like Finder, so the menu is built from the selection or the pick. Items with
//! a key dispatch the key's action: they show the key and do what it does.

use gpui::{App, Context, Entity, Window, prelude::*, px};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use shot_core::{Id, Instance, Layout, Placed, TakeAt};
use shot_render::plan::frame_time;

use super::parts::layout_name;
use super::timeline::{MenuOn, Pick, ZOOM_SCALES, picked_text, removed_words, words};
use super::{
    AddZoom, BuildLayout, DeleteSelection, EditScreen, EditSwitch, Layout1, Layout2, Layout3,
    Layout4, PickSound, RestoreSelection, ZoomToSelection, music,
};
use crate::model::{FPS, seconds};

/// What the right-click menu is about, with what it shows.
enum Subject {
    Words {
        title: String,
        kept: bool,
        removed: bool,
        layout: Option<Layout>,
    },
    Zoom {
        at: TakeAt,
        index: usize,
        scale: u16,
        title: String,
    },
    Overlay {
        at: TakeAt,
        index: usize,
        title: String,
    },
    Layout {
        title: String,
        layout: Layout,
    },
    Cut {
        title: String,
        removed: String,
    },
    Music,
    Lock(String, Option<(Id, Instance)>),
    /// An empty spot, with the playhead's layout and whether the cut has music.
    Empty {
        layout: Option<Layout>,
        music: bool,
    },
}

/// What the menu shows, read from the selection or the pick.
fn subject(this: &EditScreen, cx: &App) -> Subject {
    let layout_at = |frame: u64| this.map.span_at(frame).map(|s| s.layout);
    let empty = || Subject::Empty {
        layout: layout_at(this.playhead),
        music: this.folder.read(cx).project.cut.music.is_some(),
    };
    match this.lanes.menu.clone() {
        Some(MenuOn::Selection) => {
            let Some((_, range, kept, removed)) = this.selected() else {
                return empty();
            };
            let count = this.selection.map_or(0, |s| s.words().count());
            Subject::Words {
                title: format!("{count} {}, {}", words(count), seconds(range.duration())),
                kept,
                removed,
                layout: this.selection_frames().and_then(|f| layout_at(f.start)),
            }
        }
        Some(MenuOn::Pick) => match this.lanes.picked {
            Some(Pick::Placed(at, placed)) => match picked_text(this, at, placed, cx) {
                Some((title, scale)) => match placed {
                    Placed::Zoom(index) => Subject::Zoom {
                        at,
                        index,
                        scale,
                        title,
                    },
                    Placed::Overlay(index) => Subject::Overlay { at, index, title },
                },
                None => empty(),
            },
            Some(Pick::Layout(at, range)) => {
                let run = this
                    .map
                    .layout_runs()
                    .into_iter()
                    .find(|r| r.at == at && r.range == range);
                match run {
                    Some(run) => Subject::Layout {
                        title: format!(
                            "{}, {}",
                            layout_name(run.layout),
                            seconds(frame_time(run.frames.end - run.frames.start, FPS))
                        ),
                        layout: run.layout,
                    },
                    None => empty(),
                }
            }
            Some(Pick::Cut(at, range)) => Subject::Cut {
                title: format!("Cut, {}", seconds(range.duration())),
                removed: removed_words(this, at, range),
            },
            Some(Pick::Music) => Subject::Music,
            None => empty(),
        },
        Some(MenuOn::Lock(title, lock)) => Subject::Lock(title, lock),
        Some(MenuOn::Empty) | None => empty(),
    }
}

/// The menu for what the right-click selected or picked.
pub(super) fn context_menu(
    menu: PopupMenu,
    screen: &Entity<EditScreen>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let this = screen.read(cx);
    let menu = menu.action_context(this.focus.clone()).min_w(px(240.));
    let subject = subject(this, cx);
    let choices = this.layout_choices(cx);
    match subject {
        Subject::Words {
            title,
            kept,
            removed,
            layout,
        } => menu
            .label(title)
            .when(kept, |m| m.item(keyed("Cut", DeleteSelection)))
            .when(removed, |m| m.item(keyed("Restore", RestoreSelection)))
            .when(kept, |m| m.item(keyed("Zoom", AddZoom)))
            .submenu("Layout", window, cx, {
                let (screen, choices) = (screen.clone(), choices.clone());
                move |m, _, _| layout_items(m, layout, &choices, &screen)
            })
            .item(sound_item(screen))
            .separator()
            .item(play_item(screen))
            .item(keyed("Zoom lanes to selection", ZoomToSelection)),
        Subject::Zoom {
            at,
            index,
            scale,
            title,
        } => {
            let menu = ZOOM_SCALES
                .into_iter()
                .chain((!ZOOM_SCALES.contains(&scale)).then_some(scale))
                .fold(menu.label(title), |menu, option| {
                    let screen = screen.clone();
                    menu.item(
                        PopupMenuItem::new(format!("{option}%"))
                            .checked(option == scale)
                            .on_click(move |_, _, cx| {
                                screen.update(cx, |this, cx| {
                                    this.set_zoom_scale(at, index, option, cx)
                                })
                            }),
                    )
                });
            menu.separator()
                .item(play_item(screen))
                .item(keyed("Remove", DeleteSelection))
        }
        Subject::Overlay { at, index, title } => {
            super::overlays::menu_items(menu.label(title), screen, at, index, window, cx)
                .separator()
                .item(play_item(screen))
                .item(keyed("Remove", DeleteSelection))
        }
        Subject::Layout { title, layout } => {
            layout_items(menu.label(title), Some(layout), &choices, screen)
                .item(keyed("Edit the switch…", EditSwitch))
                .separator()
                .item(play_item(screen))
        }
        Subject::Cut { title, removed } => menu
            .label(title)
            .when(!removed.is_empty(), |m| m.label(shortened(&removed, 48)))
            .item(keyed("Restore", RestoreSelection))
            .separator()
            .item(play_item(screen)),
        Subject::Music => music::menu(menu, screen, window, cx),
        Subject::Lock(title, lock) => {
            let menu = menu.label(title).label("From the style rules").separator();
            let menu = match lock {
                Some((take, instance)) => {
                    let screen = screen.clone();
                    menu.item(PopupMenuItem::new("Remove").on_click(move |_, _, cx| {
                        screen.update(cx, |this, cx| this.remove_instance(&take, &instance, cx))
                    }))
                }
                None => menu,
            };
            menu.item(play_item(screen))
        }
        Subject::Empty { layout, music } => {
            let menu = menu
                .submenu("Set layout here", window, cx, {
                    let (screen, choices) = (screen.clone(), choices.clone());
                    move |m, _, _| layout_items(m, layout, &choices, &screen)
                })
                .item(sound_item(screen));
            let menu = if music {
                menu
            } else {
                let screen = screen.clone();
                menu.submenu("Add music", window, cx, move |m, _, _| {
                    music::sources(m, &screen, None)
                })
            };
            menu.separator().item(play_item(screen))
        }
    }
}

/// A menu item that dispatches `action`, so it shows and does what its key does.
fn keyed(text: &'static str, action: impl gpui::Action) -> PopupMenuItem {
    PopupMenuItem::new(text).action(Box::new(action))
}

/// The layouts on 1 to 4, then the other built-in and channel layouts, `current` checked, and
/// the layout builder: a new layout, or this channel layout to edit.
fn layout_items(
    menu: PopupMenu,
    current: Option<Layout>,
    (keys, more): &(Vec<Layout>, Vec<Layout>),
    screen: &Entity<EditScreen>,
) -> PopupMenu {
    let menu = keys.iter().enumerate().fold(menu, |menu, (key, layout)| {
        let item = PopupMenuItem::new(layout_name(*layout)).checked(current == Some(*layout));
        menu.item(match key {
            0 => item.action(Box::new(Layout1)),
            1 => item.action(Box::new(Layout2)),
            2 => item.action(Box::new(Layout3)),
            _ => item.action(Box::new(Layout4)),
        })
    });
    let menu = more.iter().fold(
        menu.when(!more.is_empty(), |m| m.separator()),
        |menu, &layout| {
            let screen = screen.clone();
            menu.item(
                PopupMenuItem::new(layout_name(layout))
                    .checked(current == Some(layout))
                    .on_click(move |_, _, cx| {
                        screen.update(cx, |this, cx| this.set_layout_to(layout, cx))
                    }),
            )
        },
    );
    let menu = menu
        .separator()
        .item(PopupMenuItem::new("New layout…").action(Box::new(BuildLayout)));
    match current {
        Some(layout @ Layout::Custom(_)) => {
            let screen = screen.clone();
            menu.item(
                PopupMenuItem::new("Edit layout…").on_click(move |_, window, cx| {
                    screen.update(cx, |this, cx| this.open_builder(Some(layout), window, cx))
                }),
            )
        }
        _ => menu,
    }
}

/// Opens the sound picker for the selection or the playhead. S toggles the picker, so this
/// item shows S but only opens it.
fn sound_item(screen: &Entity<EditScreen>) -> PopupMenuItem {
    let screen = screen.clone();
    PopupMenuItem::new("Add sound here")
        .action(Box::new(PickSound))
        .on_click(move |_, _, cx| screen.update(cx, |this, cx| this.add_sound_here(cx)))
}

fn play_item(screen: &Entity<EditScreen>) -> PopupMenuItem {
    let screen = screen.clone();
    PopupMenuItem::new("Play from here")
        .on_click(move |_, _, cx| screen.update(cx, |this, cx| this.play_from_here(cx)))
}

/// `text` cut to `most` characters with an ellipsis.
fn shortened(text: &str, most: usize) -> String {
    match text.char_indices().nth(most) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_is_cut_with_an_ellipsis() {
        assert_eq!(shortened("one two three", 7), "one two…");
        assert_eq!(shortened("short", 7), "short");
    }
}
