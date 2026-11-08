//! The Ship screen: every cut of the video and where it goes, its export with progress, `shot
//! check` per cut, the `cut.kdl` diff against the video branch, a still of each export, the
//! thumbnail pick, title and description, and the button that runs `shot ship`.
//!
//! The work is `shot_ship`'s: [`ship::plan`] lists the cuts, [`ship::export_cut`] renders and
//! checks one, [`ship::publish`] uploads and tags. Exports, checks, the thumbnail search and
//! uploads run on threads and report back over a channel the screen polls. One run, an export
//! or a ship, goes at a time; Esc stops it.

pub(crate) mod thumbnail;
mod words;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

use gpui::{
    App, ClipboardItem, Context, Div, Entity, FocusHandle, Focusable, FontWeight, Hsla, ObjectFit,
    PathPromptOptions, ScrollHandle, SharedString, Window, actions, div, img, prelude::*, px,
};
use shot_core::{Cut, DerivedKind, OutputKind};
use shot_mcp::bridge::{Claude, Reach};
use shot_ship::check::{CheckKind, Problem};
use shot_ship::diff::{self, Change, Line};
use shot_ship::media::{self, MediaError};
use shot_ship::oauth::{self, AuthError, Client, Endpoints, Keychain};
use shot_ship::record::Record;
use shot_ship::ship::{self, Channel, Destination, Options, PlannedCut, ShipError, Step};
use shot_ship::thumbnails::{self, Scored, ThumbnailError};
use shot_ship::youtube::{Privacy, YouTube, YouTubeError};

use crate::agent::{self, AgentRun};
use crate::folder::VideoFolder;
use crate::theme::*;
use crate::ui::{
    ActionLook, Segment, icon, kbd, label, mono, pane, proposal_action, scrolling_list, segmented,
};
use words::*;

actions!(
    ship,
    [
        ShipVideo,
        ExportCut,
        FindThumbnails,
        OpenStudio,
        Stop,
        NextCut,
        PreviousCut
    ]
);

pub(crate) const CONTEXT: &str = "ShipScreen";
/// How often background work is picked up.
const POLL: Duration = Duration::from_millis(150);
/// Thumbnail candidates shown.
const CANDIDATES: usize = 4;
/// Unchanged lines around each change of the diff.
const DIFF_CONTEXT: usize = 2;
/// Encoder sessions of an export, as `shot export` runs them.
const JOBS: usize = 3;
/// Where a Desktop app OAuth client is created.
const CREDENTIALS_URL: &str = "https://console.cloud.google.com/apis/credentials";

enum Event {
    Step(Step),
    /// The run waits for the person to sign in to YouTube in the browser.
    SigningIn,
    /// Checks of an unchanged export, from the check worker.
    Checked {
        cut: String,
        /// The [`check_stamp`] of what was checked.
        stamp: Option<u64>,
        result: Result<Vec<Problem>, String>,
    },
    /// A run exported a cut and checked it, or failed to.
    Exported {
        cut: String,
        stamp: Option<u64>,
        result: Result<Vec<Problem>, String>,
    },
    Preview {
        cut: String,
        path: PathBuf,
    },
    Scored(Scored),
    Searched(Result<(), String>),
    Finished(Outcome),
}

#[derive(Default)]
enum Checks {
    /// No export to check yet.
    #[default]
    NotExported,
    Running,
    Done(Vec<Problem>),
    Failed(String),
}

/// The checks that read the export itself, kept while the file is unchanged.
struct Measured {
    modified: SystemTime,
    problems: Vec<Problem>,
}

/// The export file of a cut.
struct Exported {
    size: u64,
    modified: SystemTime,
    /// A project file changed after the export.
    stale: bool,
}

#[derive(Default)]
struct CutState {
    checks: Checks,
    /// What the checks were run on, see [`check_stamp`]. Unchanged inputs are not checked again.
    stamp: Option<u64>,
    measured: Option<Measured>,
    export: Option<Exported>,
    /// A run is exporting this cut, so the check worker's results for it are old.
    exporting: bool,
    preview: Option<PathBuf>,
}

/// An export or ship in progress.
struct Run {
    /// Uploads and tags after the exports.
    ship: bool,
    /// The cuts it exports, in order.
    cuts: Vec<String>,
    step: Option<Step>,
    signing_in: bool,
    /// When the cut being rendered started, for the time left.
    since: Instant,
    cancel: Arc<AtomicBool>,
    /// Things the person has to do by hand afterwards.
    notes: Vec<String>,
}

enum Outcome {
    Exported(usize),
    /// `shot check` found problems the person had not seen yet.
    Problems(usize),
    Shipped(String),
    Stopped,
    Failed(String),
}

/// The last run, until the next one starts.
struct Finished {
    ship: bool,
    outcome: Outcome,
    notes: Vec<String>,
}

/// A thumbnail search in progress.
struct Search {
    scored: Option<Scored>,
    cancel: Arc<AtomicBool>,
}

pub struct ShipScreen {
    folder: Entity<VideoFolder>,
    channel: Result<Channel, String>,
    client: Result<Client, AuthError>,
    /// Why the chosen client file was not taken.
    client_note: Option<String>,
    cuts: Vec<PlannedCut>,
    /// Why the cuts could not be planned.
    plan_error: Option<String>,
    states: HashMap<String, CutState>,
    selected: usize,
    privacy: Privacy,
    record: Record,
    /// The working `cut.kdl` against the video branch's, `None` outside a channel repo.
    branch_diff: Option<Vec<Line>>,
    /// Bytes free on the video folder's disk.
    free: Option<u64>,
    candidates: Vec<PathBuf>,
    /// The candidate the thumbnail was picked from.
    picked: Option<PathBuf>,
    /// The thumbnail the studio rendered, when that is the one Ship uploads.
    designed: Option<Arc<gpui::Image>>,
    search: Option<Search>,
    search_note: Option<String>,
    claude: Option<Claude>,
    claude_pick: Option<AgentRun>,
    run: Option<Run>,
    finished: Option<Finished>,
    /// Stops the check worker, when a newer one replaces it.
    checking: Arc<AtomicBool>,
    /// What was just copied, for a moment.
    copied: Option<&'static str>,
    send: Sender<Event>,
    events: Receiver<Event>,
    polling: bool,
    focus: FocusHandle,
    /// The side column above the pinned publish bar.
    side_scroll: ScrollHandle,
    /// The thumbnail studio while it is open, in place of the screen.
    studio: Option<(Entity<thumbnail::ThumbnailStudio>, gpui::Subscription)>,
}

impl Focusable for ShipScreen {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ShipScreen {
    pub fn new(folder: Entity<VideoFolder>, cx: &mut Context<Self>) -> Self {
        let (send, events) = mpsc::channel();
        let channel = Channel::of(&folder.read(cx).dir).map_err(|e| match e {
            ShipError::NoChannel(_) => "This video is not in a channel.".to_owned(),
            e => capitalized(&e.to_string()),
        });
        Self {
            folder,
            channel,
            client: Client::load(&oauth::client_path()),
            client_note: None,
            cuts: Vec::new(),
            plan_error: None,
            states: HashMap::new(),
            selected: 0,
            privacy: Privacy::Private,
            record: Record::default(),
            branch_diff: None,
            free: None,
            candidates: Vec::new(),
            picked: None,
            designed: None,
            search: None,
            search_note: None,
            claude: Claude::find(),
            claude_pick: None,
            run: None,
            finished: None,
            checking: Arc::default(),
            copied: None,
            send,
            events,
            polling: false,
            focus: cx.focus_handle(),
            side_scroll: ScrollHandle::new(),
            studio: None,
        }
    }

    /// Opens the thumbnail studio over the screen. Closing it picks up the new thumbnail.
    fn open_studio(&mut self, _: &OpenStudio, window: &mut Window, cx: &mut Context<Self>) {
        if self.studio.is_some() {
            return;
        }
        let folder = self.folder.clone();
        let studio = cx.new(|cx| thumbnail::ThumbnailStudio::new(folder, window, cx));
        let closed = cx.subscribe_in(
            &studio,
            window,
            |this, _, _: &thumbnail::Closed, window, cx| {
                this.studio = None;
                this.focus.focus(window, cx);
                this.reload(cx);
                cx.notify();
            },
        );
        self.studio = Some((studio, closed));
        cx.notify();
    }

    /// Reads the cuts, the record, the diff and the OAuth client again, and checks every export
    /// whose inputs changed.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if self.run.is_some() {
            return;
        }
        let folder = self.folder.read(cx);
        let dir = folder.dir.clone();
        match ship::plan(&dir, &folder.project, &folder.transcripts, self.options()) {
            Ok(cuts) => {
                self.cuts = cuts;
                self.plan_error = None;
            }
            Err(e) => self.plan_error = Some(e.to_string()),
        }
        self.selected = self.selected.min(self.cuts.len().saturating_sub(1));
        self.record = Record::load(&dir).unwrap_or_default();
        self.client = Client::load(&oauth::client_path());
        self.free = ship::free_space(&dir).ok();
        let working = folder.project.cut.to_kdl();
        self.branch_diff = self.channel.as_ref().ok().and_then(|channel| {
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
        let mut queue = Vec::new();
        for cut in &self.cuts {
            let state = self.states.entry(cut.name.clone()).or_default();
            state.export = exported(&dir, &cut.export);
            let Some(export) = &state.export else {
                *state = CutState::default();
                continue;
            };
            let stamp = check_stamp(&dir, cut, &working);
            if state.stamp != stamp || matches!(state.checks, Checks::NotExported | Checks::Running)
            {
                state.checks = Checks::Running;
                state.stamp = stamp;
                let known = state
                    .measured
                    .as_ref()
                    .filter(|m| m.modified == export.modified)
                    .map(|m| m.problems.clone());
                queue.push((cut.clone(), stamp, known));
            }
        }
        if !queue.is_empty() {
            self.checking.store(true, Ordering::Relaxed);
            self.checking = Arc::default();
            let cancel = self.checking.clone();
            let project = folder.project.clone();
            let send = self.send.clone();
            std::thread::spawn(move || {
                let cancelled = || cancel.load(Ordering::Relaxed);
                for (cut, stamp, known) in queue {
                    preview(&dir, &cut, &send);
                    let result = match known {
                        Some(mut measured) => {
                            media::check_files(&dir, &project, &cut.name, &cut.export).map(|p| {
                                measured.extend(p);
                                measured.sort_by_key(|p| p.check);
                                measured
                            })
                        }
                        None => media::check_cut_until(
                            &dir,
                            &project,
                            &cut.name,
                            &cut.export,
                            &cancelled,
                        ),
                    };
                    let result = match result {
                        Err(MediaError::Cancelled) => return,
                        result => result.map_err(|e| e.to_string()),
                    };
                    let _ = send.send(Event::Checked {
                        cut: cut.name,
                        stamp,
                        result,
                    });
                }
            });
        }
        self.poll(cx);
        cx.notify();
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
        // The search names the stills by rank.
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
        // A thumbnail that is no candidate came from the studio. Read as bytes, so a new render
        // under the same path shows.
        self.designed = ship::thumbnail(dir)
            .filter(|_| self.picked.is_none() && dir.join("thumbnail.kdl").exists())
            .and_then(|path| std::fs::read(path).ok())
            .map(|bytes| Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes)));
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
        self.run.is_some()
            || self.search.is_some()
            || self
                .states
                .values()
                .any(|s| matches!(s.checks, Checks::Running))
    }

    fn apply(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Step(step) => {
                let Some(run) = &mut self.run else {
                    return;
                };
                if let Step::Rendering { cut, .. } = &step
                    && !matches!(&run.step, Some(Step::Rendering { cut: c, .. }) if c == cut)
                {
                    run.since = Instant::now();
                }
                if let Step::ThumbnailRefused { cut, reason } = &step {
                    run.notes.push(format!(
                        "YouTube refused the thumbnail of {cut} ({reason}). Set it in YouTube \
                         Studio, or ship again once the channel may use custom thumbnails."
                    ));
                }
                run.signing_in = false;
                run.step = Some(step);
            }
            Event::SigningIn => {
                if let Some(run) = &mut self.run {
                    run.signing_in = true;
                }
            }
            Event::Checked { cut, stamp, result } => {
                if let Some(state) = self.states.get_mut(&cut)
                    && !state.exporting
                    && state.stamp == stamp
                {
                    state.set_checks(result);
                }
            }
            Event::Exported { cut, stamp, result } => {
                let dir = self.folder.read(cx).dir.clone();
                let export = self.cuts.iter().find(|c| c.name == cut);
                let state = self.states.entry(cut.clone()).or_default();
                state.exporting = false;
                state.stamp = stamp;
                state.export = export.and_then(|c| exported(&dir, &c.export));
                state.set_checks(result);
            }
            Event::Preview { cut, path } => {
                self.states.entry(cut).or_default().preview = Some(path);
            }
            Event::Scored(scored) => {
                if let Some(search) = &mut self.search {
                    search.scored = Some(scored);
                }
            }
            Event::Searched(result) => {
                self.search = None;
                self.search_note = result.err();
                self.load_candidates(&self.folder.read(cx).dir.clone());
            }
            Event::Finished(outcome) => {
                let Some(run) = self.run.take() else {
                    return;
                };
                for state in self.states.values_mut() {
                    state.exporting = false;
                }
                self.finished = Some(Finished {
                    ship: run.ship,
                    outcome,
                    notes: run.notes,
                });
                self.reload(cx);
            }
        }
    }

    /// Exports `cuts` and checks each, then, given a channel and a client, uploads and tags.
    fn start(
        &mut self,
        cuts: Vec<PlannedCut>,
        ship_to: Option<(Channel, Client)>,
        cx: &mut Context<Self>,
    ) {
        if self.run.is_some() || cuts.is_empty() {
            return;
        }
        // Problems already shown count as seen: shipping with them is the person's call.
        let force = self.problems() > 0;
        for cut in &cuts {
            let state = self.states.entry(cut.name.clone()).or_default();
            state.exporting = true;
            state.checks = Checks::Running;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.finished = None;
        self.run = Some(Run {
            ship: ship_to.is_some(),
            cuts: cuts.iter().map(|c| c.name.clone()).collect(),
            step: None,
            signing_in: false,
            since: Instant::now(),
            cancel: cancel.clone(),
            notes: Vec::new(),
        });
        let folder = self.folder.read(cx);
        let (dir, project) = (folder.dir.clone(), folder.project.clone());
        let send = self.send.clone();
        std::thread::spawn(move || {
            let outcome = run(&dir, &project, &cuts, ship_to, force, &cancel, &send);
            let _ = send.send(Event::Finished(outcome));
        });
        self.poll(cx);
        cx.notify();
    }

    /// ⌘⏎: ship when YouTube is set up, otherwise export every cut.
    fn primary(&mut self, _: &ShipVideo, _: &mut Window, cx: &mut Context<Self>) {
        if self.shipped() {
            return;
        }
        let ship_to = match (&self.channel, &self.client) {
            (Ok(channel), Ok(client)) => Some((channel.clone(), client.clone())),
            _ => None,
        };
        self.start(self.cuts.clone(), ship_to, cx);
    }

    fn export_selected(&mut self, _: &ExportCut, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(cut) = self.cuts.get(self.selected).cloned() {
            self.start(vec![cut], None, cx);
        }
    }

    fn stop(&mut self, _: &Stop, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(run) = &self.run {
            run.cancel.store(true, Ordering::Relaxed);
        } else if let Some(search) = &self.search {
            search.cancel.store(true, Ordering::Relaxed);
        }
        cx.notify();
    }

    fn select(&mut self, by: isize, cx: &mut Context<Self>) {
        let last = self.cuts.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(by).min(last);
        cx.notify();
    }

    fn find_candidates(&mut self, _: &FindThumbnails, _: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_some() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.search = Some(Search {
            scored: None,
            cancel: cancel.clone(),
        });
        self.search_note = None;
        let folder = self.folder.read(cx);
        let (dir, project) = (folder.dir.clone(), folder.project.clone());
        let send = self.send.clone();
        std::thread::spawn(move || {
            let found = thumbnails::find_with(
                &dir,
                &project,
                CANDIDATES,
                &|scored| {
                    let _ = send.send(Event::Scored(scored));
                },
                &|| cancel.load(Ordering::Relaxed),
            );
            let result = match found {
                Ok(found) if found.is_empty() => Err(
                    "No face in the camera frames the main cut shows, so there is nothing to \
                     suggest. Pick a frame in YouTube Studio after the upload."
                        .to_owned(),
                ),
                Ok(_) | Err(ThumbnailError::Cancelled) => Ok(()),
                Err(e) => Err(e.to_string()),
            };
            let _ = send.send(Event::Searched(result));
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
            Err(e) => self.search_note = Some(format!("Could not use {}: {e}", still.display())),
        }
        cx.notify();
    }

    /// The thumbnail frame task: Claude sees the candidate stills, because the person asked.
    fn ask_claude(&mut self, cx: &mut Context<Self>) {
        if self.claude_pick.as_ref().is_some_and(AgentRun::running) {
            return;
        }
        let (Some(claude), Ok(channel)) = (self.claude.clone(), self.channel.clone()) else {
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
        match AgentRun::start(
            claude,
            &channel.repo,
            "Pick a thumbnail".to_owned(),
            prompt,
            Reach::FrameTask,
        ) {
            Ok(run) => {
                self.claude_pick = Some(run);
                agent::watch(cx, |this: &mut Self| this.claude_pick.as_mut(), |_, _| {});
            }
            Err(e) => self.search_note = Some(e),
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

    /// Asks for the downloaded OAuth client file and keeps a copy where `shot ship` reads it.
    fn choose_client(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Use this client".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                match oauth::install_client(&path, &oauth::client_path()) {
                    Ok(client) => {
                        this.client = Ok(client);
                        this.client_note = None;
                    }
                    Err(e) => this.client_note = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn copy(&mut self, what: &'static str, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(what);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            this.update(cx, |this, cx| {
                if this.copied == Some(what) {
                    this.copied = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
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

    fn shipped(&self) -> bool {
        self.record.release.is_some()
    }

    /// What a cut's row says about it, and in which color.
    fn status(&self, cut: &PlannedCut) -> (String, Hsla) {
        let state = self.states.get(&cut.name);
        if state.is_some_and(|s| s.exporting) {
            return ("exporting".to_owned(), ACCENT.into());
        }
        if self.record.video_id(&cut.name).is_some() {
            return ("uploaded".to_owned(), MUTED.into());
        }
        if state
            .and_then(|s| s.export.as_ref())
            .is_some_and(|e| e.stale)
        {
            return ("export is out of date".to_owned(), WARN.into());
        }
        match state.map(|s| &s.checks) {
            None | Some(Checks::NotExported) => ("not exported".to_owned(), MUTED.into()),
            Some(Checks::Running) => ("checking".to_owned(), MUTED.into()),
            Some(Checks::Failed(_)) => ("check failed".to_owned(), WARN.into()),
            Some(Checks::Done(p)) if p.is_empty() => ("ready".to_owned(), TEXT.into()),
            Some(Checks::Done(p)) => (format!("{} to look at", p.len()), WARN.into()),
        }
    }

    fn cut_list(&self, cx: &mut Context<Self>) -> Div {
        let rows = self.cuts.iter().enumerate().map(|(index, cut)| {
            let on = index == self.selected;
            let (status, color) = self.status(cut);
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
                        .child(
                            div()
                                .flex()
                                .gap(px(5.))
                                .text_size(TEXT_SM)
                                .text_color(MUTED)
                                .child(format!("{} · {} ·", aspect(cut.kind), destination(cut)))
                                .child(div().truncate().text_color(color).child(status)),
                        ),
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(12.))
                    .pt(px(4.))
                    .pb(px(8.))
                    .child(label("Cuts"))
                    .child(div().flex().gap(px(4.)).child(kbd("↑")).child(kbd("↓"))),
            )
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

    fn export_pane(&self, cut: &PlannedCut, cx: &mut Context<Self>) -> Div {
        let state = self.states.get(&cut.name);
        let export = state.and_then(|s| s.export.as_ref());
        let room = ship::export_room(cut.length);
        let header = div()
            .flex()
            .items_baseline()
            .gap(px(10.))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Export"))
            .child(div().flex_1())
            .when_some(self.free, |this, free| {
                this.child(
                    div()
                        .text_size(TEXT_SM)
                        .text_color(MUTED)
                        .child(format!("{} free", bytes(free))),
                )
            });
        let running = self
            .run
            .as_ref()
            .filter(|run| run.cuts.contains(&cut.name) && state.is_some_and(|s| s.exporting));
        let body = if let Some(run) = running {
            let now = run.step.as_ref().and_then(step_cut);
            let line = match (&run.step, now) {
                (Some(step), Some(now)) if now == cut.name => {
                    let mut line = step_line(step);
                    if let Step::Rendering {
                        rendered,
                        to_render,
                        ..
                    } = step
                        && let Some(left) = left(run.since.elapsed(), *rendered, *to_render)
                    {
                        line = format!("{line}, {left}");
                    }
                    line
                }
                (_, Some(now)) => format!("Waits for {now} to export"),
                _ => "Starting".to_owned(),
            };
            let fraction = run
                .step
                .as_ref()
                .filter(|step| step_cut(step) == Some(cut.name.as_str()))
                .and_then(step_fraction);
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(progress(fraction))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(div().flex_1().text_color(BODY).child(line))
                        .child(self.stop_button("ship-export-stop", run, cx)),
                )
        } else {
            let (line, again) = match export {
                Some(e) => (
                    format!(
                        "{}, exported {}",
                        bytes(e.size),
                        ago(SystemTime::now()
                            .duration_since(e.modified)
                            .unwrap_or_default())
                    ),
                    true,
                ),
                None => (
                    format!(
                        "Not exported yet. The {} cut takes about {} of disk while it renders.",
                        shot_ship::clock(cut.length),
                        bytes(room)
                    ),
                    false,
                ),
            };
            let busy = self.run.is_some();
            let path = cut.export.clone();
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().flex_1().min_w_0().text_color(BODY).child(line))
                .when(export.is_some(), |this| {
                    this.child(
                        proposal_action(
                            "ship-reveal",
                            "Show in Finder",
                            Some("folder"),
                            None,
                            ActionLook::Quiet,
                        )
                        .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&path))),
                    )
                })
                .when(!busy, |this| {
                    this.child(
                        proposal_action(
                            "ship-export",
                            if again { "Export again" } else { "Export" },
                            Some(if again { "again" } else { "export" }),
                            Some("E"),
                            // Lime belongs to the pinned main action below.
                            ActionLook::Plain,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.export_selected(&ExportCut, window, cx)
                        })),
                    )
                })
        };
        let stale = running.is_none() && export.is_some_and(|e| e.stale);
        let short = running.is_none()
            && (export.is_none() || stale)
            && self.free.is_some_and(|free| free < room);
        pane()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(16.))
            .pt(px(14.))
            .pb(px(14.))
            .child(header)
            .child(body)
            .when(stale, |this| {
                this.child(note(
                    "A project file changed after this export. Export again to ship what you \
                     see in the editor.",
                ))
            })
            .when(short, |this| {
                this.child(note(format!(
                    "Only {} free on this disk, and the export needs about {}. Free some space \
                     first, or the export stops when the disk is full.",
                    bytes(self.free.unwrap_or_default()),
                    bytes(room)
                )))
            })
    }

    fn stop_button(&self, id: &'static str, run: &Run, cx: &mut Context<Self>) -> impl IntoElement {
        let stopping = run.cancel.load(Ordering::Relaxed);
        if stopping {
            return div()
                .text_size(TEXT_SM)
                .text_color(MUTED)
                .child("Stopping")
                .into_any_element();
        }
        proposal_action(id, "Stop", None, Some("Esc"), ActionLook::Plain)
            .on_click(cx.listener(|this, _, window, cx| this.stop(&Stop, window, cx)))
            .into_any_element()
    }

    fn checks_pane(&self, cut: &PlannedCut) -> Div {
        let state = self.states.get(&cut.name);
        let checks = state.map_or(&Checks::NotExported, |s| &s.checks);
        let summary = match checks {
            Checks::NotExported => "they run on the export".to_owned(),
            Checks::Running if state.is_some_and(|s| s.exporting) => {
                "they run after the export".to_owned()
            }
            Checks::Running => "checking".to_owned(),
            Checks::Failed(_) => "could not check".to_owned(),
            Checks::Done(p) if p.is_empty() => "all passed".to_owned(),
            Checks::Done(p) => format!(
                "{} passed, {} to look at",
                CheckKind::ALL.len() - kinds(p),
                p.len()
            ),
        };
        let row = |kind: CheckKind| {
            let found = match checks {
                Checks::Done(p) => Some(p.iter().filter(|p| p.check == kind).count()),
                _ => None,
            };
            let (value, color, glyph) = match found {
                Some(0) => ("ok".to_owned(), Hsla::from(TEXT), Some(("check", MUTED))),
                Some(n) => (n.to_string(), WARN.into(), Some(("alert", WARN))),
                None => ("–".to_owned(), FAINT.into(), None),
            };
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .py(px(8.))
                .px(px(2.))
                .text_color(if found.is_some() { BODY } else { MUTED })
                .child(match glyph {
                    Some((name, color)) => icon(name, ICON, color).into_any_element(),
                    None => div()
                        .size(ICON)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().size(px(5.)).rounded_full().bg(FAINT))
                        .into_any_element(),
                })
                .child(div().flex_1().child(check_name(kind)))
                .child(mono(value).text_size(px(14.)).text_color(color))
        };
        let (left, right) = CheckKind::ALL.split_at(CheckKind::ALL.len().div_ceil(2));
        let column = |kinds: &[CheckKind]| {
            div()
                .flex_1()
                .flex_basis(px(0.))
                .flex()
                .flex_col()
                .children(kinds.iter().map(|&k| row(k)))
        };
        let messages: Vec<String> = match checks {
            Checks::Done(p) => p.iter().map(ToString::to_string).collect(),
            Checks::Failed(e) => vec![e.clone()],
            Checks::NotExported | Checks::Running => Vec::new(),
        };
        pane()
            .flex_none()
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
                    .child(column(left))
                    .child(column(right)),
            )
            .when(!messages.is_empty(), |this| {
                this.child(
                    div()
                        .mt(px(10.))
                        .px(px(14.))
                        .py(px(12.))
                        .rounded(RADIUS_MD)
                        .bg(WARN_TINT)
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .text_color(WARN)
                        .children(messages.into_iter().map(|m| div().child(m))),
                )
            })
    }

    fn diff_pane(&self, cut: &PlannedCut, cx: &App) -> Div {
        let (subtitle, lines) = match cut.kind {
            OutputKind::Main => match (&self.branch_diff, &self.channel) {
                (Some(lines), Ok(channel)) if lines.is_empty() => {
                    (format!("same as video/{}", channel.slug), Vec::new())
                }
                (Some(lines), Ok(channel)) => (
                    format!(
                        "not yet on video/{}, so not in the release tag",
                        channel.slug
                    ),
                    lines.clone(),
                ),
                _ => (
                    "not in a channel, so there is nothing to compare with".to_owned(),
                    Vec::new(),
                ),
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
        let changed = added + removed > 0;
        pane()
            .when(changed, |this| this.flex_1())
            .when(!changed, |this| this.flex_none())
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(16.))
                    .py(px(12.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Edits"))
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                            .child(subtitle),
                    )
                    .when(changed, |this| {
                        this.child(
                            mono(format!("+{added} −{removed}"))
                                .text_size(TEXT_SM)
                                .text_color(MUTED),
                        )
                    }),
            )
            .when(changed, |this| {
                this.child(
                    div()
                        .id("ship-diff")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .mx(px(10.))
                        .mb(px(10.))
                        .py(px(8.))
                        .rounded(RADIUS_MD)
                        .bg(WELL)
                        .children(lines.into_iter().map(diff_line)),
                )
            })
    }

    fn side(&self, cut: &PlannedCut, cx: &mut Context<Self>) -> impl IntoElement {
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
                            .flex()
                            .items_center()
                            .justify_center()
                            .map(|this| match still {
                                Some(still) => {
                                    this.child(img(still).size_full().object_fit(ObjectFit::Cover))
                                }
                                None => this.child(
                                    div()
                                        .w_full()
                                        .px(px(16.))
                                        .text_size(TEXT_SM)
                                        .text_color(MUTED)
                                        .text_center()
                                        .child("A frame shows here once the cut is exported"),
                                ),
                            }),
                    ),
            );
        if cut.kind == OutputKind::Main {
            side = side.child(self.thumbnails(cx));
        }
        match &cut.metadata {
            Some(metadata) => {
                let problems = metadata.problems();
                let title = metadata.title.clone();
                let description = metadata.description.clone();
                side = side
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(field_header(
                                "Title",
                                Some(format!(
                                    "{}/{}",
                                    title.chars().count(),
                                    shot_ship::youtube::TITLE_MAX
                                )),
                                self.copy_button("title", title.clone(), cx),
                            ))
                            .child(field(div().child(title)))
                            .children(problems.into_iter().map(|p| {
                                div()
                                    .text_size(TEXT_SM)
                                    .text_color(WARN)
                                    .child(capitalized(&p))
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(field_header(
                                "Description",
                                None,
                                self.copy_button("description", description.clone(), cx),
                            ))
                            .child(field(
                                div()
                                    .text_size(TEXT_SM)
                                    .line_height(px(20.))
                                    .text_color(BODY)
                                    .child(description),
                            )),
                    );
            }
            None => {
                let path = cut.export.clone();
                side = side.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(field_header(
                            "File",
                            None,
                            proposal_action(
                                "ship-file-reveal",
                                "Show in Finder",
                                Some("folder"),
                                None,
                                ActionLook::Quiet,
                            )
                            .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&path)))
                            .into_any_element(),
                        ))
                        .child(field(
                            mono(cut.export.display().to_string())
                                .text_size(TEXT_SM)
                                .text_color(BODY),
                        ))
                        .child(
                            div()
                                .text_size(TEXT_SM)
                                .text_color(MUTED)
                                .child("Not uploaded: post this file by hand."),
                        ),
                );
            }
        }
        pane()
            .w(px(400.))
            .flex_none()
            .flex()
            .flex_col()
            // The fade shows the column goes on under the pinned bar.
            .child(scrolling_list(
                "ship-side",
                &self.side_scroll,
                div()
                    .child(side)
                    .child(self.destination(cx).px(px(14.)).pb(px(14.))),
            ))
            .child(self.publish(cx).px(px(14.)).pt(px(4.)).pb(px(14.)))
    }

    fn copy_button(
        &self,
        what: &'static str,
        text: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let copied = self.copied == Some(what);
        proposal_action(
            SharedString::from(format!("ship-copy-{what}")),
            if copied { "Copied" } else { "Copy" },
            Some(if copied { "check" } else { "copy" }),
            None,
            ActionLook::Quiet,
        )
        .on_click(cx.listener(move |this, _, _, cx| this.copy(what, text.clone(), cx)))
        .into_any_element()
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
                .hover(|s| s.border_color(Hsla::from(ACCENT).opacity(0.3)))
                .child(img(still.clone()).size_full().object_fit(ObjectFit::Cover))
                .on_click(cx.listener(move |this, _, _, cx| this.pick(still.clone(), cx)))
        });
        let asking = self.claude_pick.as_ref().is_some_and(AgentRun::running);
        let claude_note = match &self.claude_pick {
            Some(run) if run.running() => Some("Claude is looking at the stills".to_owned()),
            Some(run) => match &run.outcome {
                Some(Ok(answer)) => Some(answer.lines().skip(1).collect::<Vec<_>>().join(" ")),
                Some(Err(e)) => Some(e.clone()),
                None => None,
            },
            None => None,
        };
        let hint = match (&self.search, self.candidates.is_empty(), &self.picked) {
            (Some(_), _, _) => None,
            (None, true, _) => Some(
                "Scores the camera frames of the main cut by expression and suggests four."
                    .to_owned(),
            ),
            (None, false, None) if self.designed.is_none() => {
                Some("Click one to make it the thumbnail.".to_owned())
            }
            (None, false, None) => None,
            (None, false, Some(_)) => None,
        };
        let header = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(div().flex_1().child(label("Thumbnail")))
            .map(|this| match &self.search {
                Some(search) => this
                    .child(
                        proposal_action(
                            "ship-find-stop",
                            "Stop",
                            None,
                            Some("Esc"),
                            ActionLook::Quiet,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.stop(&Stop, window, cx))),
                    )
                    .when(search.cancel.load(Ordering::Relaxed), |this| {
                        this.child(div().text_size(TEXT_SM).text_color(MUTED).child("Stopping"))
                    }),
                None => this
                    .when(
                        !self.candidates.is_empty()
                            && self.claude.is_some()
                            && self.channel.is_ok(),
                        |this| {
                            this.child(
                                proposal_action(
                                    "ship-ask",
                                    if asking {
                                        "Asking Claude"
                                    } else {
                                        "Ask Claude"
                                    },
                                    Some("spark"),
                                    None,
                                    ActionLook::Quiet,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.ask_claude(cx))),
                            )
                        },
                    )
                    .child(
                        proposal_action(
                            "ship-studio",
                            "Design",
                            Some("picture"),
                            Some("D"),
                            ActionLook::Plain,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_studio(&OpenStudio, window, cx)
                        })),
                    )
                    .child(
                        proposal_action(
                            "ship-find",
                            if self.candidates.is_empty() {
                                "Find candidates"
                            } else {
                                "Find again"
                            },
                            None,
                            Some("T"),
                            ActionLook::Plain,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.find_candidates(&FindThumbnails, window, cx)
                        })),
                    ),
            });
        let scoring = self.search.as_ref().map(|search| {
            let (line, fraction) = match search.scored {
                Some(Scored { done, total }) if total > 0 => (
                    format!("Scoring camera frames, {done} of {total}"),
                    Some(done as f32 / total as f32),
                ),
                _ => ("Reading the main cut".to_owned(), None),
            };
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(progress(fraction))
                .child(div().text_size(TEXT_SM).text_color(MUTED).child(line))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(header)
            .children(scoring)
            .when_some(self.designed.clone(), |this, still| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .w(px(128.))
                                .h(px(72.))
                                .flex_none()
                                .rounded(px(7.))
                                .overflow_hidden()
                                .border_2()
                                .border_color(ACCENT)
                                .child(img(still).size_full().object_fit(ObjectFit::Cover)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(TEXT_SM)
                                .text_color(MUTED)
                                .child(if self.candidates.is_empty() {
                                    "Designed in the studio."
                                } else {
                                    "Designed in the studio. Click a candidate to use a plain \
                                     frame instead."
                                }),
                        ),
                )
            })
            .when(!self.candidates.is_empty(), |this| {
                this.child(div().flex().gap(px(8.)).children(tiles))
            })
            .when_some(self.search_note.clone(), |this, note| {
                this.child(div().text_size(TEXT_SM).text_color(WARN).child(note))
            })
            .when_some(claude_note.filter(|n| !n.is_empty()), |this, note| {
                this.child(div().text_size(TEXT_SM).text_color(MUTED).child(note))
            })
            .when_some(hint, |this, hint| {
                this.child(div().text_size(TEXT_SM).text_color(MUTED).child(hint))
            })
    }

    /// Where the video goes: the shipped links, why uploads wait, or the privacy to upload
    /// with. It scrolls with the column, so it never covers the fields above it.
    fn destination(&self, cx: &mut Context<Self>) -> Div {
        let out = div().flex().flex_col().gap(px(10.));
        if self.shipped() {
            out.child(self.shipped_card(cx))
        } else {
            match (&self.channel, &self.client) {
                (Err(e), _) => out.child(card(
                    "Uploads need a channel",
                    vec![
                        div().text_size(TEXT_SM).text_color(BODY).child(format!(
                            "{e} Shipping moves the video's card to shipped and tags the \
                             release, so open the video from its channel to upload from \
                             here."
                        )),
                        div()
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                            .child("⌘⏎ exports every cut, so you can upload the files by hand."),
                    ],
                )),
                (Ok(_), Err(e)) => out.child(self.setup_card(e, cx)),
                (Ok(_), Ok(_)) => {
                    let entity = cx.entity();
                    let segments = Privacy::ALL
                        .into_iter()
                        .map(|p| {
                            let entity = entity.clone();
                            Segment::new(privacy(p), self.privacy == p, move |_, _, cx| {
                                entity.update(cx, |this, cx| {
                                    this.privacy = p;
                                    this.reload(cx);
                                })
                            })
                        })
                        .collect();
                    out.child(segmented("ship-privacy", WELL, segments)).child(
                        div()
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                            .child(self.summary()),
                    )
                }
            }
        }
    }

    /// The pinned foot of the column: what a run is doing or did, and the main action.
    fn publish(&self, cx: &mut Context<Self>) -> Div {
        let mut out = div().flex().flex_col().gap(px(10.));
        let ready = self.channel.is_ok() && self.client.is_ok();
        // A single cut's export shows its progress in the export pane only.
        if let Some(run) = self.run.as_ref().filter(|r| r.ship || r.cuts.len() > 1) {
            let line = if run.signing_in {
                "Sign in to YouTube in your browser. The ship waits up to 5 minutes.".to_owned()
            } else {
                run.step.as_ref().map_or("Starting".to_owned(), step_line)
            };
            out = out.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(progress(run.step.as_ref().and_then(step_fraction)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(TEXT_SM)
                                    .text_color(BODY)
                                    .child(line),
                            )
                            .child(self.stop_button("ship-run-stop", run, cx)),
                    ),
            );
        }
        if let Some(finished) = &self.finished {
            let (text, color) = match &finished.outcome {
                Outcome::Exported(n) => (
                    format!("Exported {} to .shot/export.", plural(*n, "cut")),
                    Hsla::from(BODY),
                ),
                Outcome::Problems(n) => (
                    format!(
                        "shot check found {}. Look at them in each cut, then ship anyway with \
                         ⌘⏎ or fix them first.",
                        plural(*n, "problem")
                    ),
                    WARN.into(),
                ),
                Outcome::Shipped(tag) => (format!("Shipped and tagged {tag}."), TEXT.into()),
                Outcome::Stopped if finished.ship => (
                    "Stopped. The next ship continues where this one stopped.".to_owned(),
                    MUTED.into(),
                ),
                Outcome::Stopped => ("Stopped.".to_owned(), MUTED.into()),
                Outcome::Failed(e) => (capitalized(e), WARN.into()),
            };
            out = out.child(div().text_size(TEXT_SM).text_color(color).child(text));
            out = out.children(
                finished
                    .notes
                    .iter()
                    .map(|n| div().text_size(TEXT_SM).text_color(WARN).child(n.clone())),
            );
        }
        // The reason is why the button is off.
        let (cta, reason) = if self.shipped() {
            (
                format!(
                    "Shipped as {}",
                    self.record.release.clone().unwrap_or_default()
                ),
                Some("This video is shipped"),
            )
        } else if let Some(run) = &self.run {
            let (doing, reason) = if run.ship {
                ("Shipping", "Shipping is running")
            } else {
                ("Exporting", "An export is running")
            };
            (doing.to_owned(), Some(reason))
        } else if !ready {
            (
                if self.cuts.len() == 1 {
                    "Export the cut".to_owned()
                } else {
                    "Export every cut".to_owned()
                },
                self.cuts.is_empty().then_some("No cuts to export"),
            )
        } else if self.problems() > 0 {
            (
                format!("Ship with {} and tag", plural(self.problems(), "problem")),
                None,
            )
        } else if self.record.uploads.values().any(|u| u.video_id.is_some()) {
            ("Finish shipping and tag".to_owned(), None)
        } else {
            ("Ship to YouTube and tag".to_owned(), None)
        };
        let enabled = reason.is_none();
        out.child(
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
                            cx.listener(|this, _, window, cx| this.primary(&ShipVideo, window, cx)),
                        )
                })
                .when_some(reason, |this, reason| {
                    this.bg(RAISED)
                        .text_color(MUTED)
                        .tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(reason).build(window, cx)
                        })
                })
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

    /// What a ship sends where, such as "Uploads main and 1 Short as private videos".
    fn summary(&self) -> String {
        let count = |kind| self.cuts.iter().filter(|c| c.kind == kind).count();
        let shorts = count(OutputKind::Derived(DerivedKind::Short));
        let files = self
            .cuts
            .iter()
            .filter(|c| c.destination == Destination::File)
            .count();
        let mut what = vec!["main".to_owned()];
        if shorts > 0 {
            what.push(plural(shorts, "Short"));
        }
        let mut summary = format!(
            "Uploads {} to YouTube as {}",
            what.join(" and "),
            privacy(self.privacy).to_lowercase()
        );
        if files > 0 {
            summary.push_str(&format!(", keeps {} as a file", plural(files, "X clip")));
        }
        summary.push_str(", then tags the release.");
        summary
    }

    fn setup_card(&self, error: &AuthError, cx: &mut Context<Self>) -> Div {
        let path = oauth::client_path();
        let problem = match error {
            AuthError::Setup(_) => None,
            other => Some(format!(
                "The client file can't be used: {other}. Choose the downloaded Desktop app \
                 JSON again."
            )),
        };
        let step = |n: &'static str, text: String| {
            div()
                .flex()
                .gap(px(8.))
                .text_size(TEXT_SM)
                .text_color(BODY)
                .child(mono(n).text_color(MUTED))
                .child(div().flex_1().min_w_0().child(text))
        };
        card(
            "Connect YouTube",
            vec![
                div()
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child("Uploads use your own Google Cloud OAuth client. Set it up once:"),
                step(
                    "1",
                    "In Google Cloud, create a project and enable the YouTube Data API v3."
                        .to_owned(),
                ),
                step(
                    "2",
                    "Under Credentials, create an OAuth client ID of type Desktop app and \
                     download its JSON. While the consent screen is in testing, add your \
                     Google account as a test user."
                        .to_owned(),
                ),
                step(
                    "3",
                    format!(
                        "Choose the downloaded file. A copy goes to {}.",
                        home_relative(&path)
                    ),
                ),
                div()
                    .flex()
                    .gap(px(8.))
                    .pt(px(2.))
                    .child(
                        proposal_action(
                            "ship-console",
                            "Open Google Cloud",
                            Some("link"),
                            None,
                            ActionLook::Plain,
                        )
                        .on_click(|_, _, cx| cx.open_url(CREDENTIALS_URL)),
                    )
                    .child(
                        proposal_action(
                            "ship-client",
                            "Choose client file",
                            Some("folder"),
                            None,
                            ActionLook::Plain,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.choose_client(cx))),
                    ),
            ]
            .into_iter()
            .chain(
                problem
                    .into_iter()
                    .chain(self.client_note.clone())
                    .map(|p| div().text_size(TEXT_SM).text_color(WARN).child(p)),
            )
            .chain([div().text_size(TEXT_SM).text_color(MUTED).child(
                "Until then, ⌘⏎ exports every cut, so you can upload the files by hand. The \
                 first ship signs you in to YouTube in the browser.",
            )])
            .collect(),
        )
    }

    fn shipped_card(&self, cx: &mut Context<Self>) -> Div {
        let tag = self.record.release.clone().unwrap_or_default();
        let rows = self.record.uploads.iter().filter_map(|(cut, upload)| {
            let url = shot_ship::youtube::watch_url(upload.video_id.as_deref()?);
            let open = url.clone();
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(70.))
                            .truncate()
                            .text_color(BODY)
                            .child(cut.clone()),
                    )
                    .child(
                        mono(url.clone())
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(TEXT_SM)
                            .text_color(MUTED),
                    )
                    .child(
                        proposal_action(
                            SharedString::from(format!("ship-open-{cut}")),
                            "Open",
                            Some("link"),
                            None,
                            ActionLook::Quiet,
                        )
                        .on_click(move |_, _, cx| cx.open_url(&open)),
                    ),
            )
        });
        let files: Vec<PathBuf> = self
            .cuts
            .iter()
            .filter(|c| c.destination == Destination::File)
            .map(|c| c.export.clone())
            .collect();
        card(
            "Shipped",
            vec![div().text_size(TEXT_SM).text_color(MUTED).child(format!(
                "Tagged {tag}. The videos keep the privacy they went up with."
            ))]
            .into_iter()
            .chain(rows)
            .chain(files.into_iter().enumerate().map(|(index, file)| {
                let name = file
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(mono(name).flex_1().text_size(TEXT_SM).text_color(BODY))
                    .child(
                        proposal_action(
                            SharedString::from(format!("ship-shipped-file-{index}")),
                            "Show in Finder",
                            Some("folder"),
                            None,
                            ActionLook::Quiet,
                        )
                        .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&file))),
                    )
            }))
            .collect(),
        )
    }
}

impl CutState {
    fn set_checks(&mut self, result: Result<Vec<Problem>, String>) {
        self.checks = match result {
            Ok(problems) => {
                if let Some(export) = &self.export {
                    self.measured = Some(Measured {
                        modified: export.modified,
                        problems: problems
                            .iter()
                            .filter(|p| measured_from_export(p.check))
                            .cloned()
                            .collect(),
                    });
                }
                Checks::Done(problems)
            }
            Err(e) => Checks::Failed(e),
        };
    }
}

impl Render for ShipScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The studio takes the whole screen, outside its keys, so typing a headline stays text.
        if let Some((studio, _)) = &self.studio {
            return div().size_full().child(studio.clone()).into_any_element();
        }
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
                        .child(self.export_pane(&cut, cx))
                        .child(self.checks_pane(&cut))
                        .child(self.diff_pane(&cut, cx)),
                )
                .child(self.side(&cut, cx)),
            None => pane().flex_1().p(px(16.)).text_color(MUTED).child(
                self.plan_error
                    .clone()
                    .map(|e| capitalized(&e))
                    .unwrap_or_else(|| {
                        "The cut has no chapters yet, so there is nothing to ship.".to_owned()
                    }),
            ),
        };
        div()
            .id("ship")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::primary))
            .on_action(cx.listener(Self::export_selected))
            .on_action(cx.listener(Self::find_candidates))
            .on_action(cx.listener(Self::open_studio))
            .on_action(cx.listener(Self::stop))
            .on_action(cx.listener(|this, _: &NextCut, _, cx| this.select(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousCut, _, cx| this.select(-1, cx)))
            .size_full()
            .flex()
            .gap(px(8.))
            .p(px(8.))
            .pt(px(0.))
            .child(self.cut_list(cx))
            .child(main)
            .into_any_element()
    }
}

/// Exports `cuts` and checks each, then uploads and tags when `ship_to` is given. Runs on its
/// own thread and reports to `send`.
fn run(
    dir: &Path,
    project: &shot_core::Project,
    cuts: &[PlannedCut],
    ship_to: Option<(Channel, Client)>,
    force: bool,
    cancel: &Arc<AtomicBool>,
    send: &Sender<Event>,
) -> Outcome {
    let cancelled = || cancel.load(Ordering::Relaxed);
    let step = |step: Step| {
        let _ = send.send(Event::Step(step));
    };
    let cut_kdl = project.cut.to_kdl();
    let mut problems = 0;
    for cut in cuts {
        step(Step::Rendering {
            cut: cut.name.clone(),
            rendered: 0,
            to_render: 0,
        });
        let result = match ship::export_cut(dir, project, cut, JOBS, &step, &cancelled) {
            Err(ShipError::Cancelled) => return Outcome::Stopped,
            result => result.map_err(|e| e.to_string()),
        };
        if result.is_ok() {
            preview(dir, cut, send);
        }
        let failed = result.as_ref().err().cloned();
        problems += result.as_ref().map_or(0, Vec::len);
        let _ = send.send(Event::Exported {
            cut: cut.name.clone(),
            stamp: check_stamp(dir, cut, &cut_kdl),
            result,
        });
        if let Some(e) = failed {
            return Outcome::Failed(e);
        }
    }
    let Some((channel, client)) = ship_to else {
        return Outcome::Exported(cuts.len());
    };
    if problems > 0 && !force {
        return Outcome::Problems(problems);
    }
    let store = Box::new(Keychain::new(&client));
    let mut youtube = YouTube::new(client, store, Endpoints::default());
    youtube.cancel = cancel.clone();
    let sign_in = |youtube: &mut YouTube| {
        let _ = send.send(Event::SigningIn);
        youtube
            .sign_in(&mut |url| {
                std::process::Command::new("open")
                    .arg(url)
                    .status()
                    .map(drop)
            })
            .map_err(|e| e.to_string())
    };
    match youtube.signed_in() {
        Ok(true) => {}
        Ok(false) => {
            if let Err(e) = sign_in(&mut youtube) {
                return Outcome::Failed(e);
            }
        }
        Err(e) => return Outcome::Failed(e.to_string()),
    }
    let mut on_step = |s: Step| {
        let _ = send.send(Event::Step(s));
    };
    let mut result = ship::publish(
        dir,
        &channel,
        cuts,
        &mut youtube,
        ship::today(),
        &mut on_step,
    );
    // A revoked sign-in: the record keeps what already went up, so the second try continues.
    if matches!(
        result,
        Err(ShipError::YouTube(YouTubeError::Auth(AuthError::Expired)))
    ) {
        if let Err(e) = sign_in(&mut youtube) {
            return Outcome::Failed(e);
        }
        result = ship::publish(
            dir,
            &channel,
            cuts,
            &mut youtube,
            ship::today(),
            &mut on_step,
        );
    }
    match result {
        Ok(shipped) => Outcome::Shipped(shipped.tag),
        Err(ShipError::YouTube(YouTubeError::Cancelled)) => Outcome::Stopped,
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

/// Whether a check reads the export itself rather than the project files.
fn measured_from_export(kind: CheckKind) -> bool {
    matches!(kind, CheckKind::Loudness | CheckKind::Frames)
}

/// The export of a cut, when there is one.
fn exported(dir: &Path, export: &Path) -> Option<Exported> {
    let meta = std::fs::metadata(export).ok()?;
    let modified = meta.modified().ok()?;
    let edited = ["cut.kdl", "format.kdl", shot_ship::lock::LOCK_FILE]
        .iter()
        .filter_map(|f| {
            std::fs::metadata(dir.join(f))
                .and_then(|m| m.modified())
                .ok()
        })
        .max();
    Some(Exported {
        size: meta.len(),
        modified,
        stale: edited.is_some_and(|e| e > modified),
    })
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
    if out.exists() || media::still(&cut.export, shot_core::Micros(cut.length.0 / 3), &out).is_ok()
    {
        let _ = send.send(Event::Preview {
            cut: cut.name.clone(),
            path: out,
        });
    }
}

/// The cut a step is about, when it is about one.
fn step_cut(step: &Step) -> Option<&str> {
    match step {
        Step::Rendering { cut, .. }
        | Step::Checking { cut }
        | Step::Uploading { cut, .. }
        | Step::Uploaded { cut, .. }
        | Step::AlreadyUploaded { cut, .. }
        | Step::Thumbnail { cut }
        | Step::ThumbnailRefused { cut, .. }
        | Step::File { cut, .. } => Some(cut),
        Step::Moved { .. } | Step::Tagged { .. } => None,
    }
}

/// `path` with the home folder written as `~`.
fn home_relative(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// How many check kinds have a problem.
fn kinds(problems: &[Problem]) -> usize {
    CheckKind::ALL
        .iter()
        .filter(|k| problems.iter().any(|p| p.check == **k))
        .count()
}

/// `text` with its first letter capitalized, for the lowercase messages of `shot_ship` errors.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
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

/// A progress bar, filled to `fraction`, or a quiet full track while there is no measure.
fn progress(fraction: Option<f32>) -> Div {
    div()
        .h(px(4.))
        .w_full()
        .rounded_full()
        .bg(RAISED)
        .overflow_hidden()
        .child(div().h_full().rounded_full().map(|this| match fraction {
            Some(f) => this.w(gpui::relative(f.clamp(0.0, 1.0))).bg(ACCENT),
            None => this.w_full().bg(FLOAT),
        }))
}

/// A field's label with an optional count and an action on the right.
fn field_header(name: &'static str, count: Option<String>, action: gpui::AnyElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(div().flex_1().child(label(name)))
        .when_some(count, |this, count| {
            this.child(mono(count).text_size(TEXT_XS).text_color(MUTED))
        })
        .child(action)
}

fn field(value: Div) -> Div {
    value.px(px(11.)).py(px(9.)).rounded(px(9.)).bg(SUNK)
}

/// A raised block with a title, for the upload setup and the shipped links.
fn card(title: &'static str, rows: Vec<Div>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(14.))
        .rounded(RADIUS_MD)
        .bg(CARD)
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
        .children(rows)
}

/// A warning under a pane's content.
fn note(text: impl Into<SharedString>) -> Div {
    div()
        .px(px(12.))
        .py(px(10.))
        .rounded(RADIUS_MD)
        .bg(WARN_TINT)
        .text_size(TEXT_SM)
        .text_color(WARN)
        .child(text.into())
}
