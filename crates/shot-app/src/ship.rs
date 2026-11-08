//! The Ship screen: every cut of the video and where it goes, the `cut.kdl` diff against the
//! video branch, `shot check` per cut, a still of each export in its aspect ratio, the thumbnail
//! pick, and the button that runs `shot ship`.
//!
//! The work is `shot_ship`'s: [`ship::plan`] lists the cuts, [`ship::prepare`] renders and
//! checks, [`ship::publish`] uploads and tags. Renders, checks and uploads run on threads and
//! report back over a channel the screen polls.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use gpui::{
    App, Context, Div, Entity, FocusHandle, Focusable, FontWeight, Hsla, KeyBinding, ObjectFit,
    SharedString, Window, actions, div, img, prelude::*, px,
};
use shot_core::{Cut, DerivedKind, OutputKind};
use shot_mcp::bridge::Claude;
use shot_ship::check::{CheckKind, Problem};
use shot_ship::diff::{self, Change, Line};
use shot_ship::oauth::{self, Client, Endpoints, Keychain};
use shot_ship::record::Record;
use shot_ship::ship::{self, Channel, Destination, Options, PlannedCut, Shipped, Step};
use shot_ship::youtube::{Privacy, YouTube};

use crate::agent::{self, AgentRun};
use crate::folder::VideoFolder;
use crate::theme::*;
use crate::ui::{Segment, icon, kbd, label, mono, pane, segmented};

actions!(ship, [ShipVideo]);

const CONTEXT: &str = "ShipScreen";
/// How often background work is picked up.
const POLL: Duration = Duration::from_millis(150);
/// Thumbnail candidates shown.
const CANDIDATES: usize = 4;
/// Unchanged lines around each change of the diff.
const DIFF_CONTEXT: usize = 2;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("cmd-enter", ShipVideo, Some(CONTEXT))]);
}

enum Event {
    Rendering {
        cut: String,
        rendered: usize,
        to_render: usize,
    },
    Checked {
        cut: String,
        /// The [`check_stamp`] of what was checked.
        stamp: Option<u64>,
        result: Result<Vec<Problem>, String>,
    },
    Preview {
        cut: String,
        path: PathBuf,
    },
    Candidates(Result<Vec<PathBuf>, String>),
    Step(Step),
    /// A step of the run that is not a ship step, such as signing in.
    Status(String),
    Shipped(Result<Shipped, String>),
}

#[derive(Default)]
enum Checks {
    /// No export to check yet.
    #[default]
    NotRendered,
    Running,
    Done(Vec<Problem>),
    Failed(String),
}

#[derive(Default)]
struct CutState {
    checks: Checks,
    /// What the checks were run on, see [`check_stamp`]. Unchanged inputs are not checked again.
    stamp: Option<u64>,
    rendering: Option<(usize, usize)>,
    preview: Option<PathBuf>,
}

/// A ship run in progress or finished.
struct Run {
    /// The latest step worth a line, such as "Uploading main 42%".
    status: String,
    outcome: Option<Result<Shipped, String>>,
}

pub struct ShipScreen {
    folder: Entity<VideoFolder>,
    channel: Option<Channel>,
    cuts: Vec<PlannedCut>,
    states: HashMap<String, CutState>,
    selected: usize,
    privacy: Privacy,
    record: Record,
    /// The working `cut.kdl` against the video branch's, `None` outside a channel repo.
    branch_diff: Option<Vec<Line>>,
    candidates: Vec<PathBuf>,
    /// The candidate the thumbnail was picked from.
    picked: Option<PathBuf>,
    finding: bool,
    claude: Option<Claude>,
    claude_pick: Option<AgentRun>,
    run: Option<Run>,
    error: Option<String>,
    send: Sender<Event>,
    events: Receiver<Event>,
    polling: bool,
    focus: FocusHandle,
}

impl Focusable for ShipScreen {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ShipScreen {
    pub fn new(folder: Entity<VideoFolder>, cx: &mut Context<Self>) -> Self {
        let (send, events) = mpsc::channel();
        let channel = Channel::of(&folder.read(cx).dir).ok();
        Self {
            folder,
            channel,
            cuts: Vec::new(),
            states: HashMap::new(),
            selected: 0,
            privacy: Privacy::Private,
            record: Record::default(),
            branch_diff: None,
            candidates: Vec::new(),
            picked: None,
            finding: false,
            claude: Claude::find(),
            claude_pick: None,
            run: None,
            error: None,
            send,
            events,
            polling: false,
            focus: cx.focus_handle(),
        }
    }

    /// Reads the cuts, the record and the diff again, and checks every rendered cut.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if self.run.as_ref().is_some_and(|r| r.outcome.is_none()) {
            return;
        }
        let folder = self.folder.read(cx);
        let dir = folder.dir.clone();
        match ship::plan(&dir, &folder.project, &folder.transcripts, self.options()) {
            Ok(cuts) => self.cuts = cuts,
            Err(e) => self.error = Some(e.to_string()),
        }
        self.selected = self.selected.min(self.cuts.len().saturating_sub(1));
        self.record = Record::load(&dir).unwrap_or_default();
        let working = folder.project.cut.to_kdl();
        self.branch_diff = self.channel.as_ref().and_then(|channel| {
            let board = shot_board::Board::open(&channel.repo).ok()?;
            let committed = board.read_file(&channel.slug, "cut.kdl").ok()?;
            Some(diff::diff(
                &committed.unwrap_or_default(),
                &working,
                DIFF_CONTEXT,
            ))
        });
        self.load_candidates(&dir);
        let names: Vec<&String> = self.cuts.iter().map(|c| &c.name).collect();
        self.states.retain(|name, _| names.contains(&name));
        let mut cuts = Vec::new();
        for cut in &self.cuts {
            let state = self.states.entry(cut.name.clone()).or_default();
            let stamp = check_stamp(&dir, cut, &working);
            if !cut.export.exists() {
                *state = CutState::default();
            } else if state.stamp != stamp || matches!(state.checks, Checks::NotRendered) {
                state.checks = Checks::Running;
                state.stamp = stamp;
                cuts.push((cut.clone(), stamp));
            }
        }
        let project = folder.project.clone();
        let send = self.send.clone();
        std::thread::spawn(move || {
            for (cut, stamp) in cuts {
                preview(&dir, &cut, &send);
                let result = shot_ship::media::check_cut(&dir, &project, &cut.name, &cut.export)
                    .map_err(|e| e.to_string());
                let _ = send.send(Event::Checked {
                    cut: cut.name,
                    stamp,
                    result,
                });
            }
        });
        self.poll(cx);
    }

    fn options(&self) -> Options {
        Options {
            privacy: self.privacy,
            made_for_kids: false,
        }
    }

    fn load_candidates(&mut self, dir: &Path) {
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir.join(".shot/cache/thumbnails"))
            .into_iter()
            .flatten()
            .filter_map(|e| Some(e.ok()?.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "png"))
            .collect();
        found.sort();
        found.truncate(CANDIDATES);
        let current = ship::thumbnail(dir).and_then(|p| std::fs::read(p).ok());
        self.picked = current.and_then(|bytes| {
            found
                .iter()
                .find(|c| std::fs::read(c).is_ok_and(|b| b == bytes))
                .cloned()
        });
        self.candidates = found;
    }

    /// Polls the event channel while there may be work in flight.
    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        self.polling = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let Ok(busy) = this.update(cx, |this, cx| {
                    while let Ok(event) = this.events.try_recv() {
                        this.apply(event, cx);
                    }
                    cx.notify();
                    let busy = this.busy();
                    this.polling = busy;
                    busy
                }) else {
                    break;
                };
                if !busy {
                    break;
                }
            }
        })
        .detach();
    }

    fn busy(&self) -> bool {
        self.finding
            || self.run.as_ref().is_some_and(|r| r.outcome.is_none())
            || self
                .states
                .values()
                .any(|s| matches!(s.checks, Checks::Running) || s.rendering.is_some())
    }

    fn apply(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Rendering {
                cut,
                rendered,
                to_render,
            } => {
                let state = self.states.entry(cut.clone()).or_default();
                state.rendering = Some((rendered, to_render));
                if let Some(run) = &mut self.run {
                    run.status = format!("Rendering {} {rendered} of {to_render}", cut);
                }
            }
            Event::Checked { cut, stamp, result } => {
                let state = self.states.entry(cut).or_default();
                state.rendering = None;
                state.stamp = stamp;
                state.checks = match result {
                    Ok(problems) => Checks::Done(problems),
                    Err(e) => Checks::Failed(e),
                };
            }
            Event::Preview { cut, path } => {
                self.states.entry(cut).or_default().preview = Some(path);
            }
            Event::Candidates(result) => {
                self.finding = false;
                match result {
                    Ok(_) => self.load_candidates(&self.folder.read(cx).dir.clone()),
                    Err(e) => self.error = Some(e),
                }
            }
            Event::Step(step) => {
                if let Some(run) = &mut self.run {
                    run.status = step_line(&step);
                }
            }
            Event::Status(status) => {
                if let Some(run) = &mut self.run {
                    run.status = status;
                }
            }
            Event::Shipped(result) => {
                if let Some(run) = &mut self.run {
                    run.outcome = Some(result);
                }
                self.record = Record::load(&self.folder.read(cx).dir).unwrap_or_default();
            }
        }
    }

    fn render_and_check(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(cut) = self.cuts.get(index).cloned() else {
            return;
        };
        let folder = self.folder.read(cx);
        let (dir, project) = (folder.dir.clone(), folder.project.clone());
        self.states.entry(cut.name.clone()).or_default().rendering = Some((0, 0));
        let send = self.send.clone();
        std::thread::spawn(move || {
            let progress = |p: shot_render::export::Progress| {
                let _ = send.send(Event::Rendering {
                    cut: cut.name.clone(),
                    rendered: p.rendered,
                    to_render: p.to_render,
                });
            };
            let rendered = shot_render::export::export(
                &dir,
                &project,
                &cut.name,
                &cut.export,
                shot_render::export::ExportOptions::default(),
                &progress,
            )
            .map_err(|e| e.to_string());
            let result = rendered.and_then(|_| {
                preview(&dir, &cut, &send);
                shot_ship::media::check_cut(&dir, &project, &cut.name, &cut.export)
                    .map_err(|e| e.to_string())
            });
            let stamp = check_stamp(&dir, &cut, &project.cut.to_kdl());
            let _ = send.send(Event::Checked {
                cut: cut.name,
                stamp,
                result,
            });
        });
        self.poll(cx);
        cx.notify();
    }

    fn find_candidates(&mut self, cx: &mut Context<Self>) {
        if self.finding {
            return;
        }
        self.finding = true;
        let folder = self.folder.read(cx);
        let (dir, project) = (folder.dir.clone(), folder.project.clone());
        let send = self.send.clone();
        std::thread::spawn(move || {
            let found = shot_ship::thumbnails::find(&dir, &project, CANDIDATES)
                .map_err(|e| e.to_string())
                .and_then(|found| {
                    if found.is_empty() {
                        Err("no face found in the camera frames the main cut shows".to_owned())
                    } else {
                        Ok(found.into_iter().filter_map(|c| c.path).collect())
                    }
                });
            let _ = send.send(Event::Candidates(found));
        });
        self.poll(cx);
        cx.notify();
    }

    fn pick(&mut self, still: PathBuf, cx: &mut Context<Self>) {
        let dir = self.folder.read(cx).dir.clone();
        match ship::pick_thumbnail(&dir, &still) {
            Ok(_) => {
                self.picked = Some(still);
                self.reload(cx);
            }
            Err(e) => self.error = Some(format!("{}: {e}", still.display())),
        }
        cx.notify();
    }

    /// The thumbnail frame task: Claude sees the candidate stills, because the person asked.
    fn ask_claude(&mut self, cx: &mut Context<Self>) {
        let (Some(claude), Some(channel)) = (self.claude.clone(), self.channel.clone()) else {
            return;
        };
        let title = self.folder.read(cx).project.video.title.clone();
        let prompt = format!(
            "Call thumbnail_frames for the video {slug}. Pick the candidate that makes the best \
             YouTube thumbnail for the title \"{title}\": a clear, expressive face that reads at \
             a small size. Answer with the candidate's file name on the first line, then one \
             sentence why.",
            slug = channel.slug
        );
        match AgentRun::start_frame_task(
            claude,
            &channel.repo,
            "Pick a thumbnail".to_owned(),
            prompt,
        ) {
            Ok(run) => {
                self.claude_pick = Some(run);
                agent::watch(cx, |this: &mut Self| this.claude_pick.as_mut(), |_, _| {});
            }
            Err(e) => self.error = Some(e),
        }
        cx.notify();
    }

    /// The candidate Claude named in its answer.
    fn claude_choice(&self) -> Option<&PathBuf> {
        let answer = self.claude_pick.as_ref()?.outcome.as_ref()?.as_ref().ok()?;
        self.candidates.iter().find(|c| {
            c.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| answer.contains(n))
        })
    }

    fn problems(&self) -> usize {
        self.states
            .values()
            .map(|s| match &s.checks {
                Checks::Done(p) => p.len(),
                _ => 0,
            })
            .sum()
    }

    fn ship(&mut self, _: &ShipVideo, _: &mut Window, cx: &mut Context<Self>) {
        if self.busy() || self.shipped() {
            return;
        }
        let Some(channel) = self.channel.clone() else {
            self.error =
                Some(ship::ShipError::NoChannel(self.folder.read(cx).dir.clone()).to_string());
            cx.notify();
            return;
        };
        let client = match Client::load(&oauth::client_path()) {
            Ok(client) => client,
            Err(e) => {
                self.error = Some(e.to_string());
                cx.notify();
                return;
            }
        };
        // Problems already shown count as seen: shipping with them is the person's call.
        let force = self.problems() > 0;
        let folder = self.folder.read(cx);
        let (dir, project, transcripts) = (
            folder.dir.clone(),
            folder.project.clone(),
            folder.transcripts.clone(),
        );
        let options = self.options();
        self.error = None;
        self.run = Some(Run {
            status: "Rendering".to_owned(),
            outcome: None,
        });
        let send = self.send.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let step = |step: Step| {
                    let event = match step {
                        Step::Rendering {
                            cut,
                            rendered,
                            to_render,
                        } => Event::Rendering {
                            cut,
                            rendered,
                            to_render,
                        },
                        step => Event::Step(step),
                    };
                    let _ = send.send(event);
                };
                let prepared = ship::prepare(&dir, &project, &transcripts, options, 3, &step)
                    .map_err(|e| e.to_string())?;
                for cut in &prepared.cuts {
                    let _ = send.send(Event::Checked {
                        cut: cut.name.clone(),
                        stamp: check_stamp(&dir, cut, &project.cut.to_kdl()),
                        result: Ok(prepared
                            .problems
                            .iter()
                            .filter(|(name, _)| *name == cut.name)
                            .map(|(_, p)| p.clone())
                            .collect()),
                    });
                }
                if !prepared.problems.is_empty() && !force {
                    return Err(format!(
                        "shot check found {} problems. Ship again to publish anyway.",
                        prepared.problems.len()
                    ));
                }
                let store = Box::new(Keychain::new(&client));
                let mut youtube = YouTube::new(client, store, Endpoints::default());
                if !youtube.signed_in().map_err(|e| e.to_string())? {
                    let _ = send.send(Event::Status(
                        "Sign in to YouTube in the browser".to_owned(),
                    ));
                    youtube
                        .sign_in(&mut |url| {
                            std::process::Command::new("open")
                                .arg(url)
                                .status()
                                .map(drop)
                        })
                        .map_err(|e| e.to_string())?;
                }
                let mut step = |s: Step| {
                    let _ = send.send(Event::Step(s));
                };
                ship::publish(
                    &dir,
                    &channel,
                    &prepared.cuts,
                    &mut youtube,
                    ship::today(),
                    &mut step,
                )
                .map_err(|e| e.to_string())
            })();
            let _ = send.send(Event::Shipped(result));
        });
        self.poll(cx);
        cx.notify();
    }

    fn shipped(&self) -> bool {
        self.record.release.is_some()
    }

    fn cut_list(&self, cx: &mut Context<Self>) -> Div {
        let rows = self.cuts.iter().enumerate().map(|(index, cut)| {
            let on = index == self.selected;
            let uploaded = self.record.video_id(&cut.name).is_some();
            let meta = format!(
                "{} · {}{}",
                aspect(cut.kind),
                destination(cut),
                if uploaded { " · uploaded" } else { "" }
            );
            div()
                .id(("ship-cut", index))
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(11.))
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .when(on, |this| this.bg(RAISED))
                .when(!on, |this| this.hover(|s| s.bg(CARD)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .child(cut_title(cut)),
                        )
                        .child(div().text_size(TEXT_SM).text_color(MUTED).child(meta)),
                )
                .child(
                    mono(shot_ship::clock(cut.length))
                        .text_size(px(14.))
                        .text_color(TEXT),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = index;
                    cx.notify();
                }))
        });
        pane()
            .w(px(290.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(12.))
            .child(div().px(px(12.)).pt(px(4.)).pb(px(8.)).child(label("Cuts")))
            .children(rows)
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(12.))
                    .pt(px(14.))
                    .pb(px(4.))
                    .child(label("Also from the terminal"))
                    .child(
                        mono("$ shot check --video .")
                            .text_size(TEXT_SM)
                            .text_color(BODY),
                    )
                    .child(
                        mono("$ shot ship --video .")
                            .text_size(TEXT_SM)
                            .text_color(BODY),
                    ),
            )
    }

    fn diff_pane(&self, cut: &PlannedCut, cx: &App) -> Div {
        let (subtitle, lines) = match cut.kind {
            OutputKind::Main => match (&self.branch_diff, &self.channel) {
                (Some(lines), Some(channel)) if lines.is_empty() => {
                    (format!("same as video/{}", channel.slug), Vec::new())
                }
                (Some(lines), Some(channel)) => (
                    format!(
                        "not yet on video/{}, so not in the release tag",
                        channel.slug
                    ),
                    lines.clone(),
                ),
                _ => ("not in a channel repo".to_owned(), Vec::new()),
            },
            OutputKind::Derived(_) => {
                let derived = self
                    .folder
                    .read(cx)
                    .project
                    .cut
                    .derived
                    .iter()
                    .find(|d| d.name.as_str() == cut.name)
                    .cloned();
                let block = Cut {
                    derived: derived.into_iter().collect(),
                    ..Cut::default()
                }
                .to_kdl();
                (
                    "cut derived from main".to_owned(),
                    diff::diff("", &block, 0),
                )
            }
        };
        let (added, removed) = diff::stat(&lines);
        let body = div()
            .id("ship-diff")
            .max_h(px(300.))
            .overflow_y_scroll()
            .mx(px(10.))
            .mb(px(10.))
            .py(px(8.))
            .rounded(RADIUS_MD)
            .bg(WELL)
            .children(lines.into_iter().map(diff_line));
        pane()
            .flex_none()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(16.))
                    .pt(px(14.))
                    .pb(px(8.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("cut.kdl"))
                    .child(div().text_size(TEXT_SM).text_color(MUTED).child(subtitle))
                    .child(div().flex_1())
                    .when(added + removed > 0, |this| {
                        this.child(
                            mono(format!("+{added} −{removed}"))
                                .text_size(TEXT_SM)
                                .text_color(MUTED),
                        )
                    }),
            )
            .when(added + removed > 0, |this| this.child(body))
    }

    fn checks_pane(&self, index: usize, cx: &mut Context<Self>) -> Div {
        let cut = &self.cuts[index];
        let state = self.states.get(&cut.name);
        let checks = state.map_or(&Checks::NotRendered, |s| &s.checks);
        let rendering = state.and_then(|s| s.rendering);
        let summary = match (rendering, checks) {
            (Some((done, total)), _) if total > 0 => format!("rendering {done} of {total} chunks"),
            (Some(_), _) => "rendering".to_owned(),
            (_, Checks::NotRendered) => "not rendered yet".to_owned(),
            (_, Checks::Running) => "checking".to_owned(),
            (_, Checks::Failed(_)) => "could not check".to_owned(),
            (_, Checks::Done(p)) if p.is_empty() => "all passed".to_owned(),
            (_, Checks::Done(p)) => format!(
                "{} passed, {} to look at",
                CheckKind::ALL.len() - kinds(p),
                p.len()
            ),
        };
        let rows = CheckKind::ALL.iter().map(|&kind| {
            let found: Vec<&Problem> = match checks {
                Checks::Done(p) => p.iter().filter(|p| p.check == kind).collect(),
                _ => Vec::new(),
            };
            let (value, color, glyph) = match checks {
                Checks::Done(_) if found.is_empty() => ("ok".to_owned(), TEXT, "check"),
                Checks::Done(_) => (format!("{}", found.len()), WARN, "alert"),
                _ => ("–".to_owned(), MUTED, "check"),
            };
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .py(px(8.))
                .px(px(2.))
                .text_color(BODY)
                .child(icon(
                    glyph,
                    ICON,
                    if found.is_empty() { MUTED } else { WARN },
                ))
                .child(div().flex_1().child(check_name(kind)))
                .child(mono(value).text_size(px(14.)).text_color(color))
        });
        let messages: Vec<String> = match checks {
            Checks::Done(p) => p.iter().map(ToString::to_string).collect(),
            Checks::Failed(e) => vec![e.clone()],
            Checks::NotRendered => {
                vec!["Render this cut to check its loudness, frames and captions.".to_owned()]
            }
            Checks::Running => Vec::new(),
        };
        let busy = rendering.is_some() || matches!(checks, Checks::Running);
        let action = match checks {
            Checks::NotRendered => "Render and check",
            _ => "Render again",
        };
        let column = |range: std::ops::Range<usize>| {
            div()
                .flex_1()
                .flex_basis(px(0.))
                .flex()
                .flex_col()
                .children(rows.clone().skip(range.start).take(range.len()))
        };
        pane()
            .flex_1()
            .flex()
            .flex_col()
            .px(px(16.))
            .pt(px(12.))
            .pb(px(14.))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .px(px(2.))
                    .pt(px(2.))
                    .pb(px(6.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Checks"))
                    .child(div().text_size(TEXT_SM).text_color(MUTED).child(summary)),
            )
            .child(
                div()
                    .flex()
                    .gap(px(24.))
                    .child(column(0..3))
                    .child(column(3..6)),
            )
            .when(!messages.is_empty(), |this| {
                this.child(
                    div()
                        .id("ship-problems")
                        .mt(px(10.))
                        .px(px(14.))
                        .py(px(12.))
                        .rounded(RADIUS_MD)
                        .bg(WARN_TINT)
                        .flex()
                        .items_start()
                        .gap(px(12.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .text_color(WARN)
                                .children(messages.into_iter().map(|m| div().child(m))),
                        )
                        .when(!busy, |this| {
                            this.child(button(("ship-render", index), action).h(px(30.)).on_click(
                                cx.listener(move |this, _, _, cx| this.render_and_check(index, cx)),
                            ))
                        }),
                )
            })
    }

    fn preview_pane(&self, cut: &PlannedCut, cx: &mut Context<Self>) -> impl IntoElement {
        let (w, h) = match cut.kind {
            OutputKind::Main => (340., 191.),
            OutputKind::Derived(DerivedKind::Short) => (176., 313.),
            OutputKind::Derived(DerivedKind::XClip) => (250., 250.),
        };
        let still = match cut.kind {
            OutputKind::Main => cut
                .thumbnail
                .clone()
                .or_else(|| self.states.get(&cut.name)?.preview.clone()),
            _ => self.states.get(&cut.name).and_then(|s| s.preview.clone()),
        };
        let format = format!("{} · {}", aspect(cut.kind), shot_ship::clock(cut.length));
        let mut side = div()
            .w(px(400.))
            .min_h_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(14.))
            .p(px(14.))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Preview"))
                    .child(div().text_size(TEXT_SM).text_color(MUTED).child(format)),
            )
            .child(
                div()
                    .flex()
                    .justify_center()
                    .py(px(16.))
                    .rounded(px(12.))
                    .bg(SUNK)
                    .child(
                        div()
                            .w(px(w))
                            .h(px(h))
                            .rounded(RADIUS_MD)
                            .bg(BLOCK)
                            .overflow_hidden()
                            .shadow_lg()
                            .when_some(still, |this, still| {
                                this.child(img(still).size_full().object_fit(ObjectFit::Cover))
                            }),
                    ),
            );
        if cut.kind == OutputKind::Main {
            side = side.child(self.thumbnails(cx));
        }
        if let Some(metadata) = &cut.metadata {
            side = side
                .child(field("Title", div().child(metadata.title.clone())))
                .child(field(
                    "Description",
                    mono(metadata.description.clone())
                        .text_size(TEXT_SM)
                        .line_height(px(20.))
                        .text_color(BODY),
                ));
        } else {
            side = side.child(field(
                "File",
                mono(cut.export.display().to_string())
                    .text_size(TEXT_SM)
                    .text_color(BODY),
            ));
        }
        pane()
            .flex_none()
            .id("ship-side")
            .overflow_y_scroll()
            .child(side.child(self.publish(cx)))
    }

    fn thumbnails(&self, cx: &mut Context<Self>) -> Div {
        let claude = self.claude_choice().cloned();
        let tiles = self.candidates.iter().enumerate().map(|(index, still)| {
            let on = self.picked.as_ref() == Some(still);
            let by_claude = claude.as_ref() == Some(still);
            let still = still.clone();
            div()
                .id(("ship-thumb", index))
                .flex_1()
                .flex_basis(px(0.))
                .h(px(48.))
                .rounded(px(7.))
                .overflow_hidden()
                .bg(BLOCK)
                .cursor_pointer()
                .border_2()
                .border_color(if on {
                    Hsla::from(ACCENT)
                } else if by_claude {
                    Hsla::from(ACCENT).opacity(0.45)
                } else {
                    gpui::transparent_black()
                })
                .child(img(still.clone()).size_full().object_fit(ObjectFit::Cover))
                .on_click(cx.listener(move |this, _, _, cx| this.pick(still.clone(), cx)))
        });
        let running = self.claude_pick.as_ref().is_some_and(AgentRun::running);
        let note = match &self.claude_pick {
            Some(run) if run.running() => Some("Claude is looking at the four stills".to_owned()),
            Some(run) => match &run.outcome {
                Some(Ok(answer)) => Some(answer.lines().skip(1).collect::<Vec<_>>().join(" ")),
                Some(Err(e)) => Some(e.clone()),
                None => None,
            },
            None => None,
        };
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .child(label("Thumbnail candidates"))
            .when(self.candidates.is_empty(), |this| {
                this.child(
                    button(
                        "ship-find",
                        if self.finding {
                            "Finding"
                        } else {
                            "Find candidates"
                        },
                    )
                    .h(px(28.))
                    .on_click(cx.listener(|this, _, _, cx| this.find_candidates(cx))),
                )
            })
            .when(
                !self.candidates.is_empty() && self.claude.is_some() && self.channel.is_some(),
                |this| {
                    this.child(
                        button(
                            "ship-ask",
                            if running {
                                "Asking Claude"
                            } else {
                                "Ask Claude"
                            },
                        )
                        .h(px(28.))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.claude_pick.as_ref().is_some_and(AgentRun::running) {
                                this.ask_claude(cx)
                            }
                        })),
                    )
                },
            );
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(header)
            .when(!self.candidates.is_empty(), |this| {
                this.child(div().flex().gap(px(8.)).children(tiles))
            })
            .when_some(note.filter(|n| !n.is_empty()), |this, note| {
                this.child(div().text_size(TEXT_SM).text_color(MUTED).child(note))
            })
    }

    fn publish(&self, cx: &mut Context<Self>) -> Div {
        let entity = cx.entity();
        let privacy = Privacy::ALL
            .into_iter()
            .map(|p| {
                let entity = entity.clone();
                let name = match p {
                    Privacy::Private => "Private",
                    Privacy::Unlisted => "Unlisted",
                    Privacy::Public => "Public",
                };
                Segment::new(name, self.privacy == p, move |_, _, cx| {
                    entity.update(cx, |this, cx| {
                        this.privacy = p;
                        this.reload(cx);
                    })
                })
            })
            .collect();
        let shorts = self
            .cuts
            .iter()
            .filter(|c| c.kind == OutputKind::Derived(DerivedKind::Short))
            .count();
        let files = self
            .cuts
            .iter()
            .filter(|c| c.destination == Destination::File)
            .count();
        let mut what = vec!["main".to_owned()];
        if shorts > 0 {
            what.push(plural(shorts, "Short"));
        }
        let mut summary = format!("Uploads {} to YouTube", what.join(" and "));
        if files > 0 {
            summary.push_str(&format!(", keeps {} as a file", plural(files, "X clip")));
        }
        let running = self.run.as_ref().is_some_and(|r| r.outcome.is_none());
        let problems = self.problems();
        let cta = if self.shipped() {
            format!(
                "Shipped as {}",
                self.record.release.clone().unwrap_or_default()
            )
        } else if running {
            "Shipping".to_owned()
        } else if problems > 0 {
            format!("Ship with {} and tag", plural(problems, "problem"))
        } else {
            "Ship to YouTube and tag".to_owned()
        };
        let status = match &self.run {
            Some(Run {
                outcome: Some(Err(e)),
                ..
            }) => Some((e.clone(), WARN)),
            Some(Run {
                outcome: Some(Ok(shipped)),
                ..
            }) => Some((
                shipped
                    .videos
                    .iter()
                    .map(|(cut, id)| format!("{cut} {}", shot_ship::youtube::watch_url(id)))
                    .collect::<Vec<_>>()
                    .join("\n"),
                TEXT,
            )),
            Some(run) => Some((run.status.clone(), MUTED)),
            None => self.error.clone().map(|e| (e, WARN)),
        };
        let enabled = !running && !self.shipped();
        div()
            .mt_auto()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(segmented("ship-privacy", WELL, privacy))
            .child(div().text_size(TEXT_SM).text_color(MUTED).child(summary))
            .when_some(status, |this, (text, color)| {
                this.child(div().text_size(TEXT_SM).text_color(color).child(text))
            })
            .child(
                div()
                    .id("ship-go")
                    .h(px(44.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .rounded(px(9.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(enabled, |this| {
                        this.bg(ACCENT)
                            .text_color(ON_ACCENT)
                            .cursor_pointer()
                            .hover(|s| s.bg(INSERTED))
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.ship(&ShipVideo, window, cx)
                                }),
                            )
                    })
                    .when(!enabled, |this| this.bg(RAISED).text_color(MUTED))
                    .child(cta)
                    .when(enabled, |this| {
                        this.child(
                            kbd("⌘⏎")
                                .bg(Hsla::from(ON_ACCENT).opacity(0.14))
                                .text_color(ON_ACCENT),
                        )
                    }),
            )
    }
}

impl Render for ShipScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main = match self.cuts.get(self.selected).cloned() {
            Some(cut) => div()
                .flex_1()
                .min_w_0()
                .flex()
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(self.diff_pane(&cut, cx))
                        .child(self.checks_pane(self.selected, cx)),
                )
                .child(self.preview_pane(&cut, cx)),
            None => pane()
                .flex_1()
                .p(px(16.))
                .text_color(MUTED)
                .child("The cut has no chapters yet, so there is nothing to ship."),
        };
        div()
            .id("ship")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::ship))
            .size_full()
            .flex()
            .gap(px(8.))
            .p(px(8.))
            .pt(px(0.))
            .child(self.cut_list(cx))
            .child(main)
    }
}

/// A hash of what a cut's checks read: its export, the thumbnail and `cut.kdl`.
fn check_stamp(dir: &Path, cut: &PlannedCut, cut_kdl: &str) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    modified(&cut.export)?.hash(&mut hasher);
    ship::thumbnail(dir)
        .and_then(|p| modified(&p))
        .hash(&mut hasher);
    cut_kdl.hash(&mut hasher);
    // A migrated video's frames follow its lock too, which a rule change rebuilds.
    std::fs::read(dir.join(shot_ship::lock::LOCK_FILE))
        .ok()
        .hash(&mut hasher);
    Some(hasher.finish())
}

/// Writes a still of the export a third in, so a Short's captions are likely on screen.
fn preview(dir: &Path, cut: &PlannedCut, send: &Sender<Event>) {
    let stamp = std::fs::metadata(&cut.export)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    // The modification time is in the name, so gpui's image cache never shows an old render.
    let out = dir.join(format!(".shot/cache/ship/{}-{stamp}.png", cut.name));
    if out.exists()
        || shot_ship::media::still(&cut.export, shot_core::Micros(cut.length.0 / 3), &out).is_ok()
    {
        let _ = send.send(Event::Preview {
            cut: cut.name.clone(),
            path: out,
        });
    }
}

fn diff_line(line: Line) -> Div {
    let (sign, color, bg) = match line.change {
        Change::Added => ("+", INSERTED, Some(Hsla::from(ACCENT).opacity(0.07))),
        Change::Removed => ("-", CUT_TEXT, Some(Hsla::from(CUT).opacity(0.08))),
        Change::Same => (" ", MUTED, None),
        Change::Gap => ("", FAINT, None),
    };
    let text = match line.change {
        Change::Gap => "⋯".to_owned(),
        _ => format!("{sign}{}", line.text),
    };
    div()
        .px(px(16.))
        .font_family(MONO)
        .text_size(TEXT_SM)
        .line_height(px(23.))
        .whitespace_nowrap()
        .text_color(color)
        .when_some(bg, |this, bg| this.bg(bg))
        .child(text)
}

fn field(name: &'static str, value: Div) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(label(name))
        .child(value.px(px(11.)).py(px(9.)).rounded(px(9.)).bg(SUNK))
}

fn button(id: impl Into<gpui::ElementId>, text: &'static str) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(34.))
        .px(px(14.))
        .flex()
        .items_center()
        .rounded(px(9.))
        .bg(RAISED)
        .font_weight(FontWeight::MEDIUM)
        .text_size(px(14.))
        .cursor_pointer()
        .hover(|s| s.bg(FLOAT))
        .child(text)
}

fn cut_title(cut: &PlannedCut) -> SharedString {
    match cut.kind {
        OutputKind::Main => "Main video".into(),
        OutputKind::Derived(DerivedKind::Short) => format!("Short: {}", cut.name).into(),
        OutputKind::Derived(DerivedKind::XClip) => format!("Clip for X: {}", cut.name).into(),
    }
}

fn aspect(kind: OutputKind) -> &'static str {
    match kind {
        OutputKind::Main => "16:9",
        OutputKind::Derived(DerivedKind::Short) => "9:16",
        OutputKind::Derived(DerivedKind::XClip) => "1:1",
    }
}

fn destination(cut: &PlannedCut) -> &'static str {
    match (cut.destination, cut.kind) {
        (Destination::File, _) => "File",
        (_, OutputKind::Main) => "YouTube",
        _ => "Shorts",
    }
}

fn check_name(kind: CheckKind) -> &'static str {
    match kind {
        CheckKind::Loudness => "Loudness",
        CheckKind::Captions => "Captions",
        CheckKind::Frames => "No black or frozen frames",
        CheckKind::Links => "Links have cards",
        CheckKind::Chapters => "Chapters",
        CheckKind::Thumbnail => "Thumbnail",
        CheckKind::Overrides => "Overrides find their element",
    }
}

/// How many check kinds have a problem.
fn kinds(problems: &[Problem]) -> usize {
    CheckKind::ALL
        .iter()
        .filter(|k| problems.iter().any(|p| p.check == **k))
        .count()
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn step_line(step: &Step) -> String {
    match step {
        Step::Rendering {
            cut,
            rendered,
            to_render,
        } => format!("Rendering {cut} {rendered} of {to_render}"),
        Step::Checking { cut } => format!("Checking {cut}"),
        Step::Uploading { cut, sent } => format!(
            "Uploading {cut} {:.0}%",
            sent.bytes as f64 * 100.0 / sent.total.max(1) as f64
        ),
        Step::Uploaded { cut, .. } | Step::AlreadyUploaded { cut, .. } => {
            format!("Uploaded {cut}")
        }
        Step::Thumbnail { cut } => format!("Set the thumbnail of {cut}"),
        Step::ThumbnailRefused { cut, .. } => {
            format!("YouTube refused the thumbnail of {cut}, set it in YouTube Studio")
        }
        Step::File { cut, .. } => format!("{cut} is ready as a file"),
        Step::Moved { slug } => format!("Moved {slug} to shipped"),
        Step::Tagged { tag } => format!("Tagged {tag}"),
    }
}
