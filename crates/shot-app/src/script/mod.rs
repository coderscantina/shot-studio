//! The Script screen: the outline, sources and script checks, the script in an editor, and the
//! script agent.
//!
//! The editor writes `script.md` in the video's worktree as typed: on a pause of 750 ms, when
//! focus leaves the screen, when the window loses focus and on quit, each time through an
//! atomic write. The edit stays uncommitted until something commits it, as accepting a proposal
//! does ([0037]). Every change parses the script again, so the outline, sources, checks and
//! spoken length follow as you type, and the line `Script::parse` refuses is marked with its
//! message. When the screen opens or the window comes back, a `script.md` changed on disk
//! replaces the text as one undo step, unless the editor holds an edit not yet saved. A new
//! video, with no script or only its title, starts from `# <title>` and one `## ` chapter.
//!
//! Claude changes the script only by proposing ([0004]). A run first saves the editor and
//! commits the person's script edits, so every proposal branches from the text on screen
//! ([0038]); the editor stays editable while Claude works, and Stop (⌘.) ends the run. Each
//! proposal shows as cards above the editor, one per chapter it changes, with struck and
//! inserted words, the spoken time it adds or saves, Accept (Tab), Reject (Esc) and Another
//! take; the selected one marks the lines it changes. ⌥↓ and ⌥↑ select the next or previous
//! suggestion; Tab and Esc act only while one is selected. Accept saves the editor, and on
//! lines the person changed too the proposal wins; the editor takes the new text as one step
//! that one Undo takes back. Proposals named `hook-*` are alternative hooks, shown side by side
//! with their full text and spoken length: Use this hook accepts an open draft first, then the
//! hook, and rejects the other hooks; Reject all and Another set drop them, the second asking
//! again. ⌘J puts the caret in the prompt box. The branch stays out of sight. Continue to record commits the script, marks it ready
//! and moves on, since Record reads the script from the branch.
//!
//! The agent pane keeps every run of the session in order as a conversation ([`agent`]).
//!
//! [0004]: ../../../docs/decisions/0004-proposals-as-branches.md
//! [0037]: ../../../docs/decisions/0037-a-worktree-per-video-branch.md
//! [0038]: ../../../docs/decisions/0038-script-editor-from-druck.md

pub mod proposal;

use std::collections::HashMap;
use std::fs;
use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle,
    Hsla, ScrollHandle, SharedString, StrikethroughStyle, StyledText, Subscription, Task, Window,
    actions, div, prelude::*, px, relative,
};
use gpui_component::input::{Input, InputEvent, InputState};
use shot_board::Board;
use shot_core::{Micros, Script, ScriptChapter, Status, Video};
use shot_editor::{Changed, EditorStyle, EditorView};
use shot_mcp::agents::{DRAFT, HOOK, ScriptCommand};
use shot_mcp::bridge::{Claude, Reach};
use shot_ship::script_check::{self, ScriptFinding, ScriptReport};

use crate::agent::{self, AgentRun};
use crate::theme::*;
use crate::ui::{ActionLook, icon, label, mono, pane, proposal_action};
use proposal::{Change, Piece};

pub(crate) const CONTEXT: &str = "ScriptScreen";
/// How long typing pauses before the script is saved.
const SAVE_PAUSE: Duration = Duration::from_millis(750);

actions!(
    script,
    [
        AcceptProposal,
        RejectProposal,
        NextSuggestion,
        PreviousSuggestion,
        RejectHooks,
        AnotherSet,
        FocusPrompt,
        StopRun,
        ContinueToRecord
    ]
);

/// The script is ready and the person moves on to Record.
pub struct ReadyToRecord;

/// An open proposal that changes `script.md`.
struct ScriptProposal {
    name: String,
    subject: String,
    patch: String,
    changes: Vec<Change>,
    /// Set for an alternative hook.
    hook: Option<HookOption>,
}

/// A hook proposal's first chapter as the proposal has it, and how long it takes to say.
struct HookOption {
    chapter: ScriptChapter,
    spoken: Micros,
}

/// A source the beats link, and what it is.
struct Source {
    target: String,
    kind: String,
}

pub struct ScriptScreen {
    repo: PathBuf,
    slug: String,
    /// `script.md` in the video's worktree, which the editor saves to.
    file: PathBuf,
    editor: Entity<EditorView>,
    /// `script.md` as last read from or written to disk, `None` while there is no file.
    saved: Option<String>,
    /// The editor holds an edit not written yet.
    unsaved: bool,
    save_later: Task<()>,
    /// The script as last parsed without an error. While a line is wrong the outline, sources and
    /// checks stay as they were, and the editor marks the line.
    script: Option<Script>,
    report: Option<ScriptReport>,
    sources: Vec<Source>,
    /// What each linked source is, read once per reload.
    source_kinds: HashMap<String, String>,
    proposals: Vec<ScriptProposal>,
    /// The proposal Tab and Esc act on.
    selected: Option<String>,
    error: Option<String>,
    claude: Option<Claude>,
    /// Every run of the session, oldest first.
    runs: Vec<AgentRun>,
    chat: ScrollHandle,
    input: Entity<InputState>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ReadyToRecord> for ScriptScreen {}

/// The screen gives the keys to the editor, inside the screen's own focus.
impl Focusable for ScriptScreen {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl ScriptScreen {
    pub fn new(repo: PathBuf, slug: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Ask for changes, or type / for commands")
        });
        cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let text = input.read(cx).value().to_string();
                    if !text.trim().is_empty() && this.ask(&text, cx) {
                        input.update(cx, |input, cx| input.set_value("", window, cx));
                    }
                }
                InputEvent::Focus => this.select(None, cx),
                _ => {}
            },
        )
        .detach();
        let file = script_file(&repo, &slug);
        let saved = fs::read_to_string(&file).ok();
        let text = match saved.as_deref() {
            Some(text) if !is_bare_title(text) => text.to_owned(),
            Some(title) => format!("{}\n\n## ", title.trim_end()),
            None => {
                let title = fs::read_to_string(file.with_file_name("video.kdl"))
                    .ok()
                    .and_then(|kdl| Video::parse(&kdl).ok())
                    .map_or_else(|| slug.clone(), |video| video.title);
                format!("# {title}\n\n## ")
            }
        };
        let editor = cx.new(|cx| EditorView::new(&text, editor_style(), cx));
        let focus = cx.focus_handle();
        let subscriptions = vec![
            cx.subscribe(&editor, |this, _, _: &Changed, cx| this.edited(cx)),
            cx.on_focus(&editor.focus_handle(cx), window, |this, _, cx| {
                this.select(None, cx)
            }),
            cx.on_focus_out(&focus, window, |this, _, _, cx| {
                this.save(cx);
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.reload(cx);
                } else {
                    this.save(cx);
                }
                cx.notify();
            }),
            cx.on_app_quit(|this, cx| {
                this.save(cx);
                async {}
            }),
        ];
        let mut screen = Self {
            repo,
            slug,
            file,
            editor,
            saved,
            unsaved: false,
            save_later: Task::ready(()),
            script: None,
            report: None,
            sources: Vec::new(),
            source_kinds: HashMap::new(),
            proposals: Vec::new(),
            selected: None,
            error: None,
            claude: Claude::find(),
            runs: Vec::new(),
            chat: ScrollHandle::new(),
            input,
            focus,
            _subscriptions: subscriptions,
        };
        screen.reload(cx);
        screen
    }

    /// Reads `script.md` from disk and the proposals from git. A script changed on disk replaces
    /// the editor's text unless the editor holds an unsaved edit.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let on_disk = fs::read_to_string(&self.file).ok();
        if on_disk != self.saved && !self.unsaved {
            if let Some(text) = &on_disk {
                self.editor
                    .update(cx, |editor, cx| editor.set_text_external(text, cx));
            }
            self.saved = on_disk;
        }
        self.source_kinds.clear();
        self.proposals = match Board::open(&self.repo) {
            Ok(board) => board
                .proposals(Some(&self.slug))
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| {
                    let diff = board.proposal_diff(&p.slug, &p.name).ok()?;
                    let file = diff.into_iter().find(|f| f.path == "script.md")?;
                    let hook = p
                        .name
                        .starts_with(HOOK)
                        .then(|| hook_option(&board, &p.slug, &p.name))
                        .flatten();
                    Some(ScriptProposal {
                        changes: proposal::changes(&file.patch),
                        patch: file.patch,
                        name: p.name,
                        subject: p.subject,
                        hook,
                    })
                })
                .collect(),
            Err(e) => {
                self.error = Some(e.to_string());
                Vec::new()
            }
        };
        let selected = self
            .selected
            .take()
            .filter(|s| self.proposals.iter().any(|p| &p.name == s));
        self.select(selected, cx);
        self.parse(cx);
    }

    /// Parses the editor's text: the outline, checks and sources follow it, and the line the
    /// parser refuses is marked in the editor.
    fn parse(&mut self, cx: &mut Context<Self>) {
        let parsed = Script::parse(self.editor.read(cx).text());
        let mark = parsed
            .as_ref()
            .err()
            .map(|e| (e.line, SharedString::from(e.message.clone())));
        self.editor
            .update(cx, |editor, cx| editor.set_error(mark, cx));
        let Ok(script) = parsed else {
            return;
        };
        self.report = Some(script_check::check_script(&script));
        let mut targets: Vec<String> = Vec::new();
        for target in script
            .chapters
            .iter()
            .flat_map(|c| &c.beats)
            .flat_map(|b| &b.sources)
        {
            if !targets.contains(target) {
                targets.push(target.clone());
            }
        }
        self.script = Some(script);
        // Git is read only for a source this reload has not seen yet, not on every key.
        let board = targets
            .iter()
            .any(|t| !self.source_kinds.contains_key(t))
            .then(|| Board::open(&self.repo).ok())
            .flatten();
        let sources = targets
            .into_iter()
            .map(|target| {
                let kind = self
                    .source_kinds
                    .entry(target.clone())
                    .or_insert_with(|| {
                        if target.starts_with("http") {
                            return "Link".to_owned();
                        }
                        match board.as_ref().map(|b| b.read_file(&self.slug, &target)) {
                            Some(Ok(Some(text))) => {
                                format!("Snapshot · {} words", text.split_whitespace().count())
                            }
                            _ => "Missing from sources/".to_owned(),
                        }
                    })
                    .clone();
                Source { target, kind }
            })
            .collect();
        self.sources = sources;
    }

    /// The editor changed: parse again and save after a pause.
    fn edited(&mut self, cx: &mut Context<Self>) {
        self.unsaved = true;
        self.parse(cx);
        self.save_later = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_PAUSE).await;
            this.update(cx, |this, cx| this.save(cx)).ok();
        });
        cx.notify();
    }

    /// Writes the editor's text to `script.md` if it holds an unsaved edit. False when the write
    /// failed, with why in the screen's error.
    fn save(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.unsaved {
            return true;
        }
        let text = self.editor.read(cx).text().to_owned();
        match shot_fs::write(&self.file, &text) {
            Ok(()) => {
                self.saved = Some(text);
                self.unsaved = false;
                self.error = None;
                true
            }
            Err(e) => {
                self.error = Some(format!("The script was not saved: {e}"));
                cx.notify();
                false
            }
        }
    }

    /// Selects a proposal for Tab and Esc and marks the lines it changes in the editor, for a
    /// hook the first chapter.
    fn select(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        let lines = match name
            .as_ref()
            .and_then(|name| self.proposals.iter().find(|p| &p.name == name))
        {
            Some(p) if p.hook.is_some() => first_chapter(self.editor.read(cx).text())
                .into_iter()
                .collect(),
            Some(p) => p
                .changes
                .iter()
                .map(|c| c.line..c.end.max(c.line + 1))
                .collect(),
            None => Vec::new(),
        };
        self.selected = name;
        self.editor
            .update(cx, |editor, cx| editor.set_highlights(lines, cx));
        cx.notify();
    }

    /// Sends what the person typed. False when no run started.
    fn ask(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let command = match &self.script {
            Some(script) if !script.chapters.is_empty() => ScriptCommand::parse(text),
            _ => ScriptCommand::Draft {
                brief: text.trim().to_owned(),
            },
        };
        self.start(text.trim().to_owned(), command, cx)
    }

    fn running(&self) -> bool {
        self.runs.last().is_some_and(AgentRun::running)
    }

    /// Starts a run after saving the editor and committing the person's script edits, so its
    /// proposals branch from the text on screen. False when it did not start.
    fn start(&mut self, request: String, command: ScriptCommand, cx: &mut Context<Self>) -> bool {
        let Some(claude) = self.claude.clone() else {
            return false;
        };
        if self.running() || !self.save(cx) {
            return false;
        }
        let committed = Board::open(&self.repo).and_then(|b| {
            b.commit_video_files(&self.slug, &["script.md"], "📝 edits before asking Claude")
        });
        if let Err(e) = committed.or_else(|e| match e {
            shot_board::Error::NoWorktree(_) => Ok(None),
            e => Err(e),
        }) {
            self.error = Some(format!("The script was not committed: {e}"));
            cx.notify();
            return false;
        }
        let started = AgentRun::start(
            claude,
            &self.repo,
            request,
            command.prompt(&self.slug),
            Reach::Web,
        );
        let ok = started.is_ok();
        match started {
            Ok(run) => {
                self.runs.push(run);
                self.chat.scroll_to_bottom();
                agent::watch(
                    cx,
                    |this: &mut Self| this.runs.last_mut(),
                    |this, cx| {
                        this.chat.scroll_to_bottom();
                        this.reload(cx);
                    },
                );
            }
            Err(e) => self.error = Some(e),
        }
        cx.notify();
        ok
    }

    /// Accepts a proposal after saving the editor. On lines the person changed too, the proposal
    /// wins. Using a hook first accepts an open draft, which the hooks build on, and rejects the
    /// other hooks. The editor takes the new text as one step that one Undo takes back.
    fn accept(&mut self, name: &str, cx: &mut Context<Self>) {
        if !self.save(cx) {
            return;
        }
        let hook = self.hook_names().any(|hook| hook == name);
        let draft = hook && self.proposals.iter().any(|p| p.name == DRAFT);
        let others: Vec<String> = self
            .hook_names()
            .filter(|other| hook && *other != name)
            .map(str::to_owned)
            .collect();
        let result = Board::open(&self.repo).and_then(|board| {
            if draft {
                board.accept_proposal_over_conflicts(&self.slug, DRAFT)?;
            }
            board.accept_proposal_over_conflicts(&self.slug, name)?;
            others
                .iter()
                .try_for_each(|other| board.reject_proposal(&self.slug, other))
        });
        self.error = result.err().map(|e| e.to_string());
        if let Ok(text) = fs::read_to_string(&self.file) {
            self.editor
                .update(cx, |editor, cx| editor.set_text_accepted(&text, cx));
            self.saved = Some(text);
        }
        self.reload(cx);
    }

    fn hook_names(&self) -> impl Iterator<Item = &str> {
        self.proposals
            .iter()
            .filter(|p| p.hook.is_some())
            .map(|p| p.name.as_str())
    }

    fn reject_hooks(&mut self, cx: &mut Context<Self>) {
        let hooks: Vec<String> = self.hook_names().map(str::to_owned).collect();
        let result = Board::open(&self.repo).and_then(|board| {
            hooks
                .iter()
                .try_for_each(|hook| board.reject_proposal(&self.slug, hook))
        });
        if let Err(e) = result {
            self.error = Some(e.to_string());
        }
        self.reload(cx);
    }

    /// Drops the hooks and asks for three new ones.
    fn another_set(&mut self, cx: &mut Context<Self>) {
        self.reject_hooks(cx);
        self.start("Another set of hooks".to_owned(), ScriptCommand::Hooks, cx);
    }

    fn reject(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Err(e) = Board::open(&self.repo).and_then(|b| b.reject_proposal(&self.slug, name)) {
            self.error = Some(e.to_string());
        }
        self.reload(cx);
        cx.notify();
    }

    fn another_take(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(patch) = self
            .proposals
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.patch.clone())
        else {
            return;
        };
        self.reject(name, cx);
        self.start(
            format!("Another take on {name}"),
            ScriptCommand::AnotherTake {
                proposal: name.to_owned(),
                diff: patch,
            },
            cx,
        );
    }

    /// Tab accepts the selected suggestion and gives the keys back to the editor, so Undo
    /// takes it back.
    fn on_accept(&mut self, _: &AcceptProposal, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected.clone() {
            Some(name) => {
                self.accept(&name, cx);
                window.focus(&self.editor.focus_handle(cx), cx);
            }
            None => window.focus_next(cx),
        }
    }

    /// Esc rejects the selected suggestion, on a hook every hook, since they are one choice.
    fn on_reject(&mut self, _: &RejectProposal, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.selected.clone() else {
            return;
        };
        if self.hook_names().any(|hook| hook == name) {
            self.reject_hooks(cx);
        } else {
            self.reject(&name, cx);
        }
        window.focus(&self.editor.focus_handle(cx), cx);
    }

    fn on_focus_prompt(&mut self, _: &FocusPrompt, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn on_next(&mut self, _: &NextSuggestion, window: &mut Window, cx: &mut Context<Self>) {
        self.step_selection(true, window, cx);
    }

    fn on_previous(&mut self, _: &PreviousSuggestion, window: &mut Window, cx: &mut Context<Self>) {
        self.step_selection(false, window, cx);
    }

    /// Selects the next or previous suggestion in the order they show, hooks first, and takes
    /// the keys from the editor so Tab and Esc reach it.
    fn step_selection(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let names: Vec<String> = self
            .hook_names()
            .chain(self.card_proposals().map(|p| p.name.as_str()))
            .map(str::to_owned)
            .collect();
        let Some(last) = names.len().checked_sub(1) else {
            return;
        };
        let at = (self.selected.as_ref()).and_then(|s| names.iter().position(|n| n == s));
        let next = match (at, forward) {
            (None, true) => 0,
            (None, false) => last,
            (Some(i), true) => (i + 1) % names.len(),
            (Some(i), false) => i.checked_sub(1).unwrap_or(last),
        };
        window.focus(&self.focus, cx);
        self.select(Some(names[next].clone()), cx);
    }

    fn on_reject_hooks(&mut self, _: &RejectHooks, _: &mut Window, cx: &mut Context<Self>) {
        self.reject_hooks(cx);
    }

    fn on_another_set(&mut self, _: &AnotherSet, _: &mut Window, cx: &mut Context<Self>) {
        self.another_set(cx);
    }

    fn on_stop(&mut self, _: &StopRun, _: &mut Window, _: &mut Context<Self>) {
        if let Some(run) = self.runs.last() {
            run.stop();
        }
    }

    /// The proposals that show as cards: every one but the hooks.
    fn card_proposals(&self) -> impl Iterator<Item = &ScriptProposal> {
        self.proposals.iter().filter(|p| p.hook.is_none())
    }

    /// Saves and commits the script, marks it ready, as moving its card to Ready to record does,
    /// and moves on. Record reads the script from the branch.
    fn continue_to_record(&mut self, _: &ContinueToRecord, _: &mut Window, cx: &mut Context<Self>) {
        if !self.save(cx) {
            return;
        }
        let committed = Board::open(&self.repo)
            .and_then(|b| b.commit_video_files(&self.slug, &["script.md"], "📝 write the script"));
        if let Err(e) = committed {
            self.error = Some(format!("The script was not committed: {e}"));
            cx.notify();
            return;
        }
        let status = crate::board::channel::status(&self.repo, &self.slug);
        if matches!(status, Some(Status::Ideas | Status::Scripting)) {
            let moved =
                Board::open(&self.repo).and_then(|b| b.move_card(&self.slug, Status::Ready));
            if let Err(e) = moved {
                self.error = Some(e.to_string());
                cx.notify();
                return;
            }
        }
        cx.emit(ReadyToRecord);
    }

    /// Shows a 1-based line of the script at the top of the editor, with the caret on it.
    fn show_line(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.go_to_line(line, window, cx));
    }

    fn outline(&self, cx: &mut Context<Self>) -> Div {
        let mut column = pane()
            .w(px(268.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(14.));
        let Some(script) = &self.script else {
            return column;
        };
        let report = self.report.as_ref().expect("a parsed script has a report");
        column = column
            .child(
                div()
                    .flex()
                    .justify_between()
                    .px(px(4.))
                    .pb(px(6.))
                    .child(label("Outline"))
                    .child(
                        mono(clock(report.total))
                            .text_size(TEXT_SM)
                            .text_color(MUTED),
                    ),
            )
            .children(
                script
                    .chapters
                    .iter()
                    .zip(&report.chapters)
                    .enumerate()
                    .map(|(index, (chapter, length))| {
                        div()
                            .id(("outline", index))
                            .flex()
                            .justify_between()
                            .gap(px(8.))
                            .px(px(10.))
                            .py(px(9.))
                            .rounded(RADIUS_SM)
                            .text_color(BODY)
                            .cursor_pointer()
                            .hover(|s| s.bg(RAISED).text_color(STRONG))
                            .child(div().min_w_0().truncate().child(chapter.title.clone()))
                            .child(
                                mono(clock(*length))
                                    .flex_none()
                                    .text_size(TEXT_SM)
                                    .text_color(MUTED),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let text = this.editor.read(cx).text();
                                if let Some(line) = chapter_lines(text).get(index).copied() {
                                    this.show_line(line, window, cx);
                                }
                            }))
                    }),
            )
            .child(label("Sources").px(px(4.)).pt(px(16.)).pb(px(6.)))
            .children(self.sources.iter().map(|source| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(RADIUS_MD)
                    .bg(RAISED)
                    .child(
                        div()
                            .truncate()
                            .text_color(STRONG)
                            .child(source_name(&source.target)),
                    )
                    .child(
                        div()
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                            .child(source.kind.clone()),
                    )
            }))
            .when(self.sources.is_empty(), |this| {
                this.child(
                    div()
                        .px(px(4.))
                        .text_size(px(14.))
                        .text_color(FAINT)
                        .child("No sources linked yet"),
                )
            })
            .child(label("Checks").px(px(4.)).pt(px(16.)).pb(px(6.)));
        let passing = [
            report
                .findings
                .iter()
                .all(|f| !matches!(f, ScriptFinding::LongHook { .. }))
                .then(|| {
                    format!(
                        "Hook lands in {} s",
                        report.hook.unwrap_or_default().0 / 1_000_000
                    )
                }),
            report
                .findings
                .iter()
                .all(|f| !matches!(f, ScriptFinding::NoSource { .. }))
                .then(|| "Every beat has a source".to_owned()),
            report
                .findings
                .iter()
                .all(|f| !matches!(f, ScriptFinding::NoVisual { .. }))
                .then(|| "No long stretch without a visual".to_owned()),
        ];
        column
            .children(passing.into_iter().flatten().map(|text| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(6.))
                    .py(px(3.))
                    .text_size(px(14.))
                    .text_color(KBD_TEXT)
                    .child(icon("check", ICON, MUTED))
                    .child(text)
            }))
            .children(summarize(&report.findings).into_iter().enumerate().map(
                |(index, (text, line))| {
                    div()
                        .id(("finding", index))
                        .mt(px(6.))
                        .px(px(12.))
                        .py(px(10.))
                        .rounded(RADIUS_MD)
                        .bg(WARN_TINT)
                        .text_size(px(14.))
                        .text_color(WARN)
                        .child(text)
                        .when_some(line, |this, line| {
                            this.cursor_pointer()
                                .hover(|s| s.text_color(STRONG))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.show_line(line, window, cx)
                                }))
                        })
                },
            ))
            .child(div().flex_1())
            .child(
                div().pt(px(16.)).child(
                    proposal_action(
                        "continue-to-record",
                        "Continue to record",
                        Some("record"),
                        Some("⌘↩"),
                        ActionLook::Accent,
                    )
                    .justify_center()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.continue_to_record(&ContinueToRecord, window, cx)
                    })),
                ),
            )
    }

    /// The hooks to pick from and the proposals as cards above the editor, which takes the rest
    /// of the pane.
    fn document(&self, cx: &mut Context<Self>) -> Div {
        let text = self.editor.read(cx).text().to_owned();
        let cards: Vec<_> = self
            .card_proposals()
            .flat_map(|p| p.changes.iter().map(move |c| (p, c)))
            .map(|(proposal, change)| {
                let chapter = change
                    .chapter
                    .clone()
                    .or_else(|| chapter_at(&text, change.line));
                self.proposal_card(proposal, change, chapter, cx)
                    .into_any_element()
            })
            .collect();
        let hooks: Vec<_> = self
            .proposals
            .iter()
            .filter_map(|p| Some((p, p.hook.as_ref()?)))
            .collect();
        pane()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(16.))
            .when_some(self.error.clone(), |this, error| {
                this.child(div().px(px(16.)).text_color(CUT_TEXT).child(error))
            })
            .when(!cards.is_empty() || !hooks.is_empty(), |this| {
                this.child(
                    div()
                        .id("script-proposals")
                        .flex_none()
                        .max_h(relative(0.6))
                        .overflow_y_scroll()
                        .px(px(16.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .when(!hooks.is_empty(), |this| {
                            this.child(self.hook_choice(hooks, cx))
                        })
                        .children(cards),
                )
            })
            .child(div().flex_1().min_h_0().child(self.editor.clone()))
    }

    /// The hook options side by side, each with its full text and spoken length, and Reject
    /// all and Another set under them.
    fn hook_choice(
        &self,
        hooks: Vec<(&ScriptProposal, &HookOption)>,
        cx: &mut Context<Self>,
    ) -> Div {
        let options = hooks.into_iter().map(|(proposal, hook)| {
            let name = proposal.name.clone();
            let selected = self.selected.as_deref() == Some(name.as_str());
            let letter = name.strip_prefix(HOOK).unwrap_or(&name).to_uppercase();
            let paragraphs = (!hook.chapter.note.is_empty())
                .then(|| div().text_color(BODY).child(hook.chapter.note.clone()))
                .into_iter()
                .chain(hook.chapter.beats.iter().flat_map(|beat| {
                    [
                        Some(div().text_color(TEXT).child(beat.text.clone())),
                        (!beat.script.is_empty())
                            .then(|| div().text_color(BODY).child(beat.script.clone())),
                    ]
                    .into_iter()
                    .flatten()
                }));
            div()
                .id(SharedString::from(format!("hook-option-{name}")))
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(8.))
                .px(px(14.))
                .py(px(12.))
                .rounded(RADIUS_MD)
                .bg(SUNK)
                .border_1()
                .border_color(if selected {
                    Hsla::from(ACCENT).opacity(0.5)
                } else {
                    Hsla::from(SUNK)
                })
                .cursor_pointer()
                .on_click(cx.listener({
                    let name = name.clone();
                    move |this, _, window, cx| {
                        window.focus(&this.focus, cx);
                        this.select(Some(name.clone()), cx);
                    }
                }))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(label(format!("Hook {letter}")))
                        .child(
                            mono(clock(hook.spoken))
                                .text_size(TEXT_SM)
                                .text_color(MUTED),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .line_height(px(22.))
                        .children(paragraphs),
                )
                .child(
                    div().flex().child(
                        proposal_action(
                            format!("use-{name}"),
                            "Use this hook",
                            None,
                            selected.then_some("Tab"),
                            if selected {
                                ActionLook::Accent
                            } else {
                                ActionLook::Plain
                            },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.accept(&name, cx))),
                    ),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(label("Claude suggests · pick a hook"))
            .child(div().flex().gap(px(8.)).children(options))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        proposal_action(
                            "reject-hooks",
                            "Reject all",
                            None,
                            Some("Esc"),
                            ActionLook::Plain,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.reject_hooks(cx))),
                    )
                    .child(
                        proposal_action(
                            "another-set",
                            "Another set",
                            Some("again"),
                            None,
                            ActionLook::Quiet,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.another_set(cx))),
                    ),
            )
    }

    fn proposal_card(
        &self,
        proposal: &ScriptProposal,
        change: &Change,
        chapter: Option<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let name = proposal.name.clone();
        let selected = self.selected.as_deref() == Some(name.as_str());
        let count = proposal.changes.len();
        let position = proposal
            .changes
            .iter()
            .position(|c| c == change)
            .unwrap_or_default();
        let saved = change.words_saved();
        let saved_text = match saved.signum() {
            1 => format!("saves {:.1} s", spoken_seconds(saved)),
            -1 => format!("adds {:.1} s", spoken_seconds(-saved)),
            _ => String::new(),
        };
        let action = |id: &'static str, text, keys, look| {
            proposal_action(format!("{id}-{}", proposal.name), text, None, keys, look)
        };
        let place = [
            chapter,
            (count > 1).then(|| format!("{} of {count}, accepted together", position + 1)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        div()
            .id(SharedString::from(format!(
                "proposal-{}-{}",
                proposal.name, change.line
            )))
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(14.))
            .py(px(12.))
            .rounded(RADIUS_MD)
            .bg(SUNK)
            .border_1()
            .border_color(if selected {
                Hsla::from(ACCENT).opacity(0.5)
            } else {
                Hsla::from(SUNK)
            })
            .cursor_pointer()
            .on_click(cx.listener({
                let name = name.clone();
                move |this, _, window, cx| {
                    window.focus(&this.focus, cx);
                    this.select(Some(name.clone()), cx);
                }
            }))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(label(format!("Claude suggests · {}", proposal.subject)))
                    .child(div().text_size(TEXT_SM).text_color(FAINT).child(place))
                    .child(div().flex_1())
                    .child(div().text_size(TEXT_SM).text_color(MUTED).child(saved_text)),
            )
            .child(
                div()
                    .text_color(BODY)
                    .line_height(px(24.))
                    .child(inline_change(&change.pieces)),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .mt(px(4.))
                    .child(
                        action("accept", "Accept", Some("Tab"), ActionLook::Accent).on_click(
                            cx.listener({
                                let name = name.clone();
                                move |this, _, _, cx| this.accept(&name, cx)
                            }),
                        ),
                    )
                    .child(
                        action("reject", "Reject", Some("Esc"), ActionLook::Plain).on_click(
                            cx.listener({
                                let name = name.clone();
                                move |this, _, _, cx| this.reject(&name, cx)
                            }),
                        ),
                    )
                    .child(
                        action("again", "Another take", None, ActionLook::Quiet).on_click(
                            cx.listener(move |this, _, _, cx| this.another_take(&name, cx)),
                        ),
                    ),
            )
    }

    fn agent_pane(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let body = match &self.claude {
            None => agent::setup_view(cx.listener(|this, _, _, cx| {
                this.claude = Claude::find();
                cx.notify();
            })),
            Some(_) => div()
                .flex()
                .flex_col()
                .gap(px(20.))
                .when(self.runs.is_empty(), |this| {
                    let drafted = self.script.as_ref().is_some_and(|s| !s.chapters.is_empty());
                    this.child(div().text_color(MUTED).line_height(px(22.)).child(if drafted {
                        "Ask for a change below, or pick a command. Claude's changes show up \
                         in the script as suggestions you accept or reject."
                    } else {
                        "Write a brief below: what the video is about, your take and how long \
                         it should run. Claude drafts the script as a suggestion you accept or \
                         reject."
                    }))
                })
                .children(self.runs.iter().map(|run| agent::run_view(run, window, cx))),
        };
        let running = self.running();
        let chips = ScriptCommand::SLASH.map(|command| {
            div()
                .id(SharedString::from(format!("slash-{command}")))
                .h(px(28.))
                .flex()
                .items_center()
                .px(px(8.))
                .rounded(RADIUS_SM)
                .bg(RAISED)
                .text_size(TEXT_SM)
                .cursor_pointer()
                .hover(|s| s.bg(FLOAT))
                .when(running || self.claude.is_none(), |this| this.opacity(0.5))
                .child(command)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.start(command.to_owned(), ScriptCommand::parse(command), cx);
                }))
        });
        pane()
            .w(px(392.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(STRONG)
                            .child("Script agent"),
                    )
                    .child(
                        div()
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                            .child("Claude · suggests changes to the script"),
                    ),
            )
            .child(
                div()
                    .id("agent-steps")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.chat)
                    .child(body),
            )
            .child(div().flex().flex_wrap().gap(px(6.)).children(chips))
            .child(
                div()
                    .p(px(4.))
                    .rounded(RADIUS_MD)
                    .bg(SUNK)
                    .child(Input::new(&self.input).appearance(false)),
            )
    }
}

impl Render for ScriptScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("script")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_accept))
            .on_action(cx.listener(Self::on_reject))
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_previous))
            .on_action(cx.listener(Self::on_reject_hooks))
            .on_action(cx.listener(Self::on_another_set))
            .on_action(cx.listener(Self::on_stop))
            .on_action(cx.listener(Self::on_focus_prompt))
            .on_action(cx.listener(Self::continue_to_record))
            .size_full()
            .flex()
            .gap(px(8.))
            .p(px(8.))
            .pt(px(0.))
            .child(self.outline(cx))
            .child(self.document(cx))
            .child(self.agent_pane(window, cx))
    }
}

/// One line per problem with the line of the beat it points at, beats without a source counted
/// together.
fn summarize(findings: &[ScriptFinding]) -> Vec<(String, Option<usize>)> {
    let mut out = Vec::new();
    let unsourced: Vec<(&str, usize)> = findings
        .iter()
        .filter_map(|f| match f {
            ScriptFinding::NoSource { chapter, line } => Some((chapter.as_str(), *line)),
            _ => None,
        })
        .collect();
    for finding in findings {
        match finding {
            ScriptFinding::LongHook { spoken } => out.push((
                format!(
                    "The hook takes {} to say. Aim for under 15 s.",
                    clock(*spoken)
                ),
                None,
            )),
            ScriptFinding::NoVisual {
                chapter,
                spoken,
                line,
            } => out.push((
                format!(
                    "{chapter} runs {} without a visual. Add b-roll notes?",
                    clock(*spoken)
                ),
                Some(*line),
            )),
            ScriptFinding::NoSource { .. } => {}
        }
    }
    match unsourced.as_slice() {
        [] => {}
        [(chapter, line)] => out.push((format!("A beat in {chapter} has no source."), Some(*line))),
        [(chapter, line), ..] => out.push((
            format!(
                "{} beats have no source, the first in {chapter}.",
                unsourced.len()
            ),
            Some(*line),
        )),
    }
    out
}

/// `script.md` in the video's folder, in its worktree once the video has a branch.
fn script_file(repo: &std::path::Path, slug: &str) -> PathBuf {
    shot_mcp::style::video_folder(repo, slug)
        .unwrap_or_else(|_| repo.join("videos").join(slug))
        .join("script.md")
}

/// Whether a script is only its `# title` line, as a new video's is.
fn is_bare_title(text: &str) -> bool {
    let text = text.trim();
    text.starts_with("# ") && !text.contains('\n')
}

/// The pieces of a change as one wrapping text: struck words in red, inserted ones lime.
fn inline_change(pieces: &[Piece]) -> StyledText {
    let struck = HighlightStyle {
        color: Some(STRUCK.into()),
        strikethrough: Some(StrikethroughStyle {
            thickness: px(1.),
            color: Some(CUT.into()),
        }),
        ..Default::default()
    };
    let inserted = HighlightStyle {
        color: Some(INSERTED.into()),
        ..Default::default()
    };
    let mut text = String::new();
    let mut highlights = Vec::new();
    for piece in pieces {
        let (words, style) = match piece {
            Piece::Same(words) => (words, None),
            Piece::Removed(words) => (words, Some(struck)),
            Piece::Added(words) => (words, Some(inserted)),
        };
        if !text.is_empty() {
            text.push(' ');
        }
        let start = text.len();
        text.push_str(words);
        if let Some(style) = style {
            highlights.push((start..text.len(), style));
        }
    }
    StyledText::new(text).with_highlights(highlights)
}

/// A hook proposal's first chapter and its spoken length, read from the proposal's script.
fn hook_option(board: &Board, slug: &str, name: &str) -> Option<HookOption> {
    let (_, text) = board.proposal_file(slug, name, "script.md").ok()?;
    let script = Script::parse(&text?).ok()?;
    let spoken = script_check::check_script(&script).hook?;
    Some(HookOption {
        chapter: script.chapters.into_iter().next()?,
        spoken,
    })
}

/// The 1-based lines of the first chapter, from its heading to the next one.
fn first_chapter(text: &str) -> Option<Range<usize>> {
    let headings = chapter_lines(text);
    let start = *headings.first()?;
    Some(start..headings.get(1).copied().unwrap_or(text.lines().count() + 1))
}

/// The title of the chapter a 1-based line is in.
fn chapter_at(text: &str, line: usize) -> Option<String> {
    text.lines()
        .take(line)
        .filter_map(proposal::heading)
        .last()
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
}

/// The 1-based lines of the chapter headings, in order.
fn chapter_lines(text: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| line.trim().starts_with("## "))
        .map(|(index, _)| index + 1)
        .collect()
}

/// The editor in the app's type and colors.
fn editor_style() -> EditorStyle {
    EditorStyle {
        font: SANS.into(),
        mono: MONO.into(),
        size: TEXT_BASE,
        leading: 1.6,
        title_size: px(26.),
        chapter_size: px(19.),
        heading: STRONG.into(),
        text: TEXT.into(),
        body: BODY.into(),
        quiet: MUTED.into(),
        link: ACCENT.into(),
        code_background: RAISED.into(),
        caret: ACCENT.into(),
        selection: SELECTION.into(),
        highlight: Hsla::from(ACCENT).opacity(0.08),
        error: CUT_TEXT.into(),
        error_background: CUT_TINT.into(),
        misspelled: CUT.into(),
        thumb: FAINT.into(),
        max_width: px(720.),
        padding: px(24.),
    }
}

fn source_name(target: &str) -> String {
    target
        .strip_prefix("https://")
        .or_else(|| target.strip_prefix("http://"))
        .or_else(|| target.strip_prefix("sources/"))
        .unwrap_or(target)
        .to_owned()
}

fn clock(t: Micros) -> String {
    shot_ship::clock(t)
}

fn spoken_seconds(words: isize) -> f64 {
    words as f64 * 60.0 / script_check::WORDS_PER_MINUTE as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_video_is_a_bare_title_and_a_drafted_one_is_not() {
        assert!(is_bare_title("# Agents that write your changelog\n"));
        assert!(!is_bare_title("# Title\n\n## Hook\n"));
        assert_eq!(chapter_lines("# Title\n\n## Hook\n\n- a\n## Wrap"), [3, 6]);
        assert_eq!(
            first_chapter("# Title\n\n## Hook\n\n- a\n## Wrap"),
            Some(3..6)
        );
        assert_eq!(
            chapter_at("# Title\n\n## Hook\n\n- a\n## Wrap", 5).as_deref(),
            Some("Hook")
        );
    }

    #[test]
    fn a_finding_names_the_chapter_and_points_at_the_first_beat() {
        let findings = [
            ScriptFinding::NoSource {
                chapter: "Setup".to_owned(),
                line: 5,
            },
            ScriptFinding::NoSource {
                chapter: "Wrap".to_owned(),
                line: 9,
            },
        ];
        assert_eq!(
            summarize(&findings),
            [(
                "2 beats have no source, the first in Setup.".to_owned(),
                Some(5)
            )]
        );
    }
}
