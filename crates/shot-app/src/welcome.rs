//! The welcome window: open a video folder or a channel, start a new one, or reopen a recent
//! one. It shows when the app starts without a folder and when the Dock icon is clicked with no
//! window open. Recent videos and channels are lists of this machine, in
//! `~/Library/Application Support/Shot Studio/recent.json` and `recent-channels.json`.

use std::fs;
use std::path::{Path, PathBuf};

use gpui::{
    App, Bounds, Context, Div, FontWeight, Hsla, PathPromptOptions, SharedString, TitlebarOptions,
    Window, WindowBounds, WindowOptions, actions, div, linear_color_stop, linear_gradient, point,
    prelude::*, px, size,
};
use shot_core::{FormatKind, Status, Video};

use crate::board::channel;
use crate::board::{FORMATS, format_color, format_name};
use crate::theme::*;
use crate::ui::{ActionLook, alert, icon, label, pane, proposal_action};

actions!(shot_app, [ShowWelcome, OpenVideo, NewChannel]);

/// How many recent folders each list keeps.
const RECENT: usize = 8;
const RECENT_VIDEOS: &str = "recent.json";
const RECENT_CHANNELS: &str = "recent-channels.json";

/// Registers the welcome actions. Call once at startup, with [`on_reopen`] on the app.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &ShowWelcome, cx| open(cx));
    cx.on_action(|_: &OpenVideo, cx| pick_folder(None, cx));
    cx.on_action(|_: &NewChannel, cx| new_channel(&home(), welcome_done, cx));
}

/// Opens the welcome window when the Dock icon is clicked with no window open.
pub fn on_reopen(cx: &mut App) {
    if cx.windows().is_empty() {
        open(cx);
    }
}

/// Opens the welcome window, or brings the open one to the front.
pub fn open(cx: &mut App) {
    if let Some(window) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<WelcomeView>())
    {
        window
            .update(cx, |welcome, window, cx| {
                welcome.recent = recent_videos();
                welcome.channels = recent_channels();
                cx.notify();
                window.activate_window();
            })
            .ok();
        return;
    }
    let bounds = Bounds::centered(None, size(px(820.), px(600.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(680.), px(480.))),
            titlebar: Some(TitlebarOptions {
                title: Some("Welcome to Shot Studio".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(16.), px(16.))),
            }),
            ..Default::default()
        },
        |_, cx| {
            cx.new(|_| WelcomeView {
                format: FormatKind::Tutorial,
                recent: recent_videos(),
                channels: recent_channels(),
                error: None,
            })
        },
    )
    .ok();
    cx.activate(true);
}

/// Opens `dir` and closes the welcome window, or shows why it did not open in it: a video
/// folder (one with `video.kdl`) in the studio, a git repo as a channel board.
fn open_folder(dir: &Path, cx: &mut App) {
    let opened = if dir.join("video.kdl").exists() {
        crate::studio::open(dir, cx)
    } else if channel::is_channel(dir) {
        crate::board::open_window(dir, cx)
    } else {
        Err(format!(
            "{} is not a video folder (it has no video.kdl) or a channel (the root of a git repo)",
            shown_path(dir)
        ))
    };
    match opened {
        Ok(()) => close(cx),
        Err(error) => show_error(error, cx),
    }
}

fn close(cx: &mut App) {
    if let Some(window) = welcome_window(cx) {
        window
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
}

/// Shows `error` in the welcome window, opening it if it is closed.
pub fn show_error(error: String, cx: &mut App) {
    open(cx);
    if let Some(window) = welcome_window(cx) {
        window
            .update(cx, |welcome, _, cx| {
                welcome.error = Some(error.into());
                cx.notify();
            })
            .ok();
    }
}

fn welcome_window(cx: &App) -> Option<gpui::WindowHandle<WelcomeView>> {
    cx.windows()
        .into_iter()
        .find_map(|window| window.downcast::<WelcomeView>())
}

/// Asks for a channel repo and opens its board. `done` hears whether it opened, not when the
/// person cancels.
pub fn pick_channel(done: impl FnOnce(Result<(), String>, &mut App) + 'static, cx: &mut App) {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Open channel".into()),
    });
    cx.spawn(async move |cx| {
        let Some(repo) = paths
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
            .and_then(|paths| paths.into_iter().next())
        else {
            return;
        };
        cx.update(|cx| {
            let opened = crate::board::open_window(&repo, cx);
            done(opened, cx);
        });
    })
    .detach();
}

/// Asks for a new folder in `parent`, starts a channel there and opens its board. `done` hears
/// whether it opened, not when the person cancels.
pub fn new_channel(
    parent: &Path,
    done: impl FnOnce(Result<(), String>, &mut App) + 'static,
    cx: &mut App,
) {
    let path = cx.prompt_for_new_path(parent, Some("my-channel"));
    cx.spawn(async move |cx| {
        let Some(repo) = path.await.ok().and_then(Result::ok).flatten() else {
            return;
        };
        cx.update(|cx| {
            let opened = channel::create(&repo).and_then(|()| crate::board::open_window(&repo, cx));
            done(opened, cx);
        });
    })
    .detach();
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Closes the welcome window once a channel opened, or shows why it did not.
fn welcome_done(result: Result<(), String>, cx: &mut App) {
    match result {
        Ok(()) => close(cx),
        Err(error) => show_error(error, cx),
    }
}

/// Asks for a video folder or a channel and opens it. With `format`, asks for a new folder
/// instead and creates a video of that format in it first.
fn pick_folder(format: Option<FormatKind>, cx: &mut App) {
    let picked = match format {
        None => {
            let paths = cx.prompt_for_paths(PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some("Open".into()),
            });
            cx.spawn(async move |_| {
                let paths = paths.await.ok()?.ok()??;
                paths.into_iter().next()
            })
        }
        Some(_) => {
            let path = cx.prompt_for_new_path(&home(), Some("new-video"));
            cx.spawn(async move |_| path.await.ok()?.ok()?)
        }
    };
    cx.spawn(async move |cx| {
        let Some(dir) = picked.await else {
            return;
        };
        cx.update(|cx| {
            let created = match format {
                Some(format) => create(&dir, format),
                None => Ok(()),
            };
            match created {
                Ok(()) => open_folder(&dir, cx),
                Err(error) => show_error(error, cx),
            }
        });
    })
    .detach();
}

/// Writes a new video folder at `dir`: `video.kdl` titled after the folder name, a `script.md`
/// with only the title, and the format's preset as `format.kdl`. Refuses a folder that already
/// has files in it.
pub fn create(dir: &Path, format: FormatKind) -> Result<(), String> {
    let occupied = fs::read_dir(dir)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    if occupied {
        return Err(format!(
            "{} already has files in it. Pick a new folder name.",
            dir.display()
        ));
    }
    let name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{}: not a folder name", dir.display()))?;
    let title = title_from(name);
    let video = Video {
        title: title.clone(),
        status: Status::Scripting,
        format,
        target_date: None,
    };
    let write = |file: &str, text: &str| {
        shot_fs::write(&dir.join(file), text)
            .map_err(|e| format!("{}: {e}", dir.join(file).display()))
    };
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    write("video.kdl", &video.to_kdl())?;
    write("script.md", &format!("# {title}\n"))?;
    write("format.kdl", &format.preset().to_kdl())
}

/// A title from a folder name: `why-we-ship_fast` becomes `Why we ship fast`.
fn title_from(name: &str) -> String {
    let words = name.replace(['-', '_'], " ");
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Untitled video".to_owned(),
    }
}

fn recent_path(file: &str) -> PathBuf {
    home()
        .join("Library/Application Support/Shot Studio")
        .join(file)
}

fn load_recent(file: &str) -> Vec<PathBuf> {
    fs::read_to_string(recent_path(file))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Puts `dir` first in the recent videos. Called by the studio after a folder opens.
pub fn remember(dir: &Path) {
    remember_in(RECENT_VIDEOS, dir);
}

/// Puts `repo` first in the recent channels. Called when a board window opens.
pub fn remember_channel(repo: &Path) {
    remember_in(RECENT_CHANNELS, repo);
}

fn remember_in(file: &str, dir: &Path) {
    let Ok(dir) = std::path::absolute(dir) else {
        return;
    };
    let list = remembered(load_recent(file), dir);
    let path = recent_path(file);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(
        path,
        serde_json::to_string_pretty(&list).expect("paths serialize"),
    );
}

/// `list` with `dir` moved to the front, at most [`RECENT`] long.
fn remembered(mut list: Vec<PathBuf>, dir: PathBuf) -> Vec<PathBuf> {
    list.retain(|path| *path != dir);
    list.insert(0, dir);
    list.truncate(RECENT);
    list
}

/// A recent folder that still holds a video.
struct Recent {
    dir: PathBuf,
    title: String,
    format: FormatKind,
}

fn recent_videos() -> Vec<Recent> {
    load_recent(RECENT_VIDEOS)
        .into_iter()
        .filter_map(|dir| {
            let video = Video::parse(&fs::read_to_string(dir.join("video.kdl")).ok()?).ok()?;
            Some(Recent {
                dir,
                title: video.title,
                format: video.format,
            })
        })
        .collect()
}

/// A recent channel that still opens as a board.
struct RecentChannel {
    repo: PathBuf,
    name: String,
    videos: usize,
}

fn recent_channels() -> Vec<RecentChannel> {
    load_recent(RECENT_CHANNELS)
        .into_iter()
        .filter_map(|repo| {
            let videos = shot_board::Board::open(&repo).ok()?.cards().ok()?.len();
            Some(RecentChannel {
                name: channel::name(&repo),
                repo,
                videos,
            })
        })
        .collect()
}

/// `dir` with the home folder written as `~`.
pub fn shown_path(dir: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.and_then(|home| dir.strip_prefix(home).ok().map(Path::to_owned)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => dir.display().to_string(),
    }
}

pub struct WelcomeView {
    /// The format a new video gets.
    format: FormatKind,
    recent: Vec<Recent>,
    channels: Vec<RecentChannel>,
    error: Option<SharedString>,
}

impl WelcomeView {
    /// Opens the studio on `dir` from a click in this window.
    fn open_video(&mut self, dir: &Path, window: &mut Window, cx: &mut Context<Self>) {
        match crate::studio::open(dir, cx) {
            Ok(()) => window.remove_window(),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
            }
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let format_row = |format: FormatKind, cx: &mut Context<Self>| {
            let on = self.format == format;
            div()
                .id(format_name(format))
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(34.))
                .px(px(10.))
                .rounded(RADIUS_SM)
                .text_color(if on { STRONG } else { MUTED })
                .cursor_pointer()
                .when(on, |this| this.bg(RAISED))
                .when(!on, |this| this.hover(|s| s.text_color(TEXT)))
                .child(div().size(px(8.)).rounded_full().bg(format_color(format)))
                .child(format_name(format))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.format = format;
                    cx.notify();
                }))
        };
        // Without recent videos there is little to open yet: creating is the main action.
        let (open_look, create_look) = if self.recent.is_empty() {
            (ActionLook::Plain, ActionLook::Accent)
        } else {
            (ActionLook::Accent, ActionLook::Plain)
        };
        div()
            .w(px(300.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(px(26.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(STRONG)
                            .child("Shot Studio"),
                    )
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                            .child(env!("CARGO_PKG_VERSION").replace('-', ".")),
                    ),
            )
            .child(
                div()
                    .text_color(BODY)
                    .child("Record, cut and ship developer videos."),
            )
            .child(
                div().pt(px(24.)).child(
                    proposal_action("open-video", "Open…", None, Some("⌘O"), open_look)
                        .justify_center()
                        .on_click(|_, _, cx| pick_folder(None, cx)),
                ),
            )
            .child(
                div()
                    .pt(px(6.))
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child("A video folder or a channel"),
            )
            .child(div().pt(px(28.)).pb(px(4.)).child(label("New video")))
            .children(FORMATS.map(|format| format_row(format, cx)))
            .child(
                div()
                    .pt(px(8.))
                    .flex()
                    .gap(px(8.))
                    .child(
                        proposal_action("new-video", "Create video", None, None, create_look)
                            .flex_1()
                            .justify_center()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.error = None;
                                pick_folder(Some(this.format), cx);
                            })),
                    )
                    .child(
                        proposal_action(
                            "new-channel-start",
                            "Start a channel",
                            None,
                            None,
                            ActionLook::Quiet,
                        )
                        .flex_1()
                        .justify_center()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.error = None;
                            new_channel(&home(), welcome_done, cx);
                        })),
                    ),
            )
            .child(
                div()
                    .pt(px(6.))
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child("A video starts with recording. A channel adds a board and scripts."),
            )
    }

    fn open_channel(&mut self, repo: &Path, window: &mut Window, cx: &mut Context<Self>) {
        match crate::board::open_window(repo, cx) {
            Ok(()) => window.remove_window(),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
            }
        }
    }

    fn channel_list(&self, cx: &mut Context<Self>) -> Div {
        let row = |index: usize, channel: &RecentChannel, cx: &mut Context<Self>| {
            let repo = channel.repo.clone();
            div()
                .id(("channel", index))
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(px(12.))
                .py(px(9.))
                .rounded(RADIUS_SM)
                .cursor_pointer()
                .hover(|s| s.bg(RAISED))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(icon("board", ICON_SM, MUTED))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(TEXT)
                                .child(channel.name.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(TEXT_SM)
                                .text_color(MUTED)
                                .child(match channel.videos {
                                    1 => "1 video".to_owned(),
                                    n => format!("{n} videos"),
                                }),
                        ),
                )
                .child(
                    div()
                        .pl(px(24.))
                        .truncate()
                        .font_family(MONO)
                        .text_size(TEXT_XS)
                        .text_color(FAINT)
                        .child(shown_path(&channel.repo)),
                )
                .on_click(
                    cx.listener(move |this, _, window, cx| this.open_channel(&repo, window, cx)),
                )
        };
        div()
            .flex()
            .flex_col()
            .pb(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl(px(12.))
                    .pb(px(4.))
                    .child(label("Channels"))
                    .child(
                        proposal_action(
                            "new-channel",
                            "Start a channel",
                            None,
                            None,
                            ActionLook::Quiet,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.error = None;
                            new_channel(&home(), welcome_done, cx);
                        })),
                    ),
            )
            .when(self.channels.is_empty(), |this| {
                this.child(
                    div()
                        .px(px(12.))
                        .text_color(MUTED)
                        .line_height(px(21.))
                        .child(
                            "A channel is a git repo of your videos, planned on a board from \
                             idea to shipped. Open one with ⌘O.",
                        ),
                )
            })
            .children(
                self.channels
                    .iter()
                    .enumerate()
                    .map(|(index, channel)| row(index, channel, cx)),
            )
    }

    fn recent(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let row = |index: usize, recent: &Recent, cx: &mut Context<Self>| {
            let dir = recent.dir.clone();
            div()
                .id(("recent", index))
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(px(12.))
                .py(px(9.))
                .rounded(RADIUS_SM)
                .cursor_pointer()
                .hover(|s| s.bg(RAISED))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .size(px(8.))
                                .flex_none()
                                .rounded_full()
                                .bg(format_color(recent.format)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(TEXT)
                                .child(recent.title.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(TEXT_SM)
                                .text_color(MUTED)
                                .child(format_name(recent.format)),
                        ),
                )
                .child(
                    div()
                        .pl(px(18.))
                        .truncate()
                        .font_family(MONO)
                        .text_size(TEXT_XS)
                        .text_color(FAINT)
                        .child(shown_path(&recent.dir)),
                )
                .on_click(cx.listener(move |this, _, window, cx| this.open_video(&dir, window, cx)))
        };
        // The list scrolls under a fade at the bottom edge, so a cut-off row reads as more to
        // scroll to. The end padding lets the last row clear the fade.
        let fade = px(40.);
        let list = pane()
            .id("recent")
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .p(px(12.))
            .pb(fade)
            .child(self.channel_list(cx))
            .child(
                div()
                    .px(px(12.))
                    .pt(px(4.))
                    .pb(px(8.))
                    .child(label("Recent videos")),
            )
            .when(self.recent.is_empty(), |this| {
                this.child(
                    div()
                        .px(px(12.))
                        .text_color(MUTED)
                        .child("Videos you open show up here."),
                )
            })
            .children(
                self.recent
                    .iter()
                    .enumerate()
                    .map(|(index, recent)| row(index, recent, cx)),
            );
        div().flex_1().min_w_0().relative().child(list).child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(fade)
                .rounded_b(RADIUS_LG)
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(Hsla::from(SURFACE).opacity(0.), 0.),
                    linear_color_stop(SURFACE, 1.),
                )),
        )
    }
}

impl Render for WelcomeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .bg(FRAME)
            .font_family(SANS)
            .text_size(TEXT_BASE)
            .text_color(TEXT)
            .pt(px(44.))
            .px(px(12.))
            .pb(px(12.))
            .when_some(self.error.clone(), |this, error| this.child(alert(error)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(12.))
                    .child(
                        pane()
                            .id("start")
                            .flex_none()
                            .overflow_y_scroll()
                            .px(px(24.))
                            .py(px(24.))
                            .child(self.start(cx)),
                    )
                    .child(self.recent(cx)),
            )
            .child(
                div()
                    .flex()
                    .justify_center()
                    .gap(px(6.))
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child(
                        "A video folder holds the script, the format and the takes of one video",
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_folder_opens_as_a_video_titled_after_its_name() {
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("why-we-ship_fast");
        create(&dir, FormatKind::ProductTour).unwrap();
        let folder = crate::folder::VideoFolder::open(&dir).unwrap();
        assert_eq!(folder.project.video.title, "Why we ship fast");
        assert_eq!(folder.project.video.format, FormatKind::ProductTour);
        assert_eq!(folder.project.format, FormatKind::ProductTour.preset());
        assert!(
            create(&dir, FormatKind::Tutorial).is_err(),
            "a used folder is refused"
        );
    }

    #[test]
    fn the_recent_list_puts_the_newest_first_once_and_keeps_eight() {
        let paths: Vec<PathBuf> = (0..RECENT)
            .map(|i| PathBuf::from(format!("/v{i}")))
            .collect();
        let list = remembered(paths.clone(), PathBuf::from("/v3"));
        assert_eq!(list[0], PathBuf::from("/v3"));
        assert_eq!(list.len(), RECENT);
        let list = remembered(paths, PathBuf::from("/new"));
        assert_eq!(list.len(), RECENT);
        assert_eq!(list[0], PathBuf::from("/new"));
        assert!(!list.contains(&PathBuf::from(format!("/v{}", RECENT - 1))));
    }
}
