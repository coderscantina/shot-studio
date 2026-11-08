//! The Script screen: the outline, sources and script checks, the script as a document, and the
//! script agent.
//!
//! The script is read from the video's branch through `shot-board`. Claude changes it only by
//! proposing ([0004]): each proposal shows inline where it applies as struck and inserted
//! words with Accept (Tab), Reject (Esc) and Another take. Tab and Esc act only while a
//! proposal is selected. Proposals named `hook-*` are alternative hooks, shown as options in
//! the agent pane; picking one accepts it and rejects the others. The branch stays out of
//! sight. Edit script opens the script in the default editor, and what is saved there lands on
//! the branch when the window comes back. Continue to record marks the script ready and moves
//! on.
//!
//! [0004]: ../../../docs/decisions/0004-proposals-as-branches.md

pub mod proposal;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use gpui::{
    AnyElement, App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla,
    ScrollHandle, SharedString, Subscription, Window, actions, div, prelude::*, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use shot_board::Board;
use shot_core::{Micros, Script, Status};
use shot_mcp::agents::ScriptCommand;
use shot_mcp::bridge::{Claude, Reach};
use shot_ship::script_check::{self, ScriptFinding, ScriptReport};

use crate::agent::{self, AgentRun};
use crate::theme::*;
use crate::ui::{ActionLook, Segment, icon, label, mono, pane, proposal_action, segmented};
use proposal::{Change, Piece};

pub(crate) const CONTEXT: &str = "ScriptScreen";
/// Proposals with this name prefix are alternative hooks.
const HOOK: &str = "hook-";

/// The proposal an edit made in the default editor goes through on its way to the branch.
const OWN_EDIT: &str = "own-edit";

actions!(script, [AcceptProposal, RejectProposal, ContinueToRecord]);

/// The script is ready and the person moves on to Record.
pub struct ReadyToRecord;

/// An open proposal that changes `script.md`.
struct ScriptProposal {
    name: String,
    subject: String,
    patch: String,
    changes: Vec<Change>,
}

/// A source the beats link, and what it is.
struct Source {
    target: String,
    kind: String,
}

pub struct ScriptScreen {
    repo: PathBuf,
    slug: String,
    script: Result<Script, String>,
    report: Option<ScriptReport>,
    sources: Vec<Source>,
    proposals: Vec<ScriptProposal>,
    /// The proposal Tab and Esc act on.
    selected: Option<String>,
    full_script: bool,
    error: Option<String>,
    claude: Option<Claude>,
    run: Option<AgentRun>,
    input: Entity<InputState>,
    /// The document, one child per chapter, so the outline can scroll to one.
    scroll: ScrollHandle,
    /// The line of the beat a check finding pointed at, shown marked.
    target: Option<usize>,
    /// The script on disk when Edit script opened it, to tell a saved edit from no edit.
    opened: Option<String>,
    focus: FocusHandle,
    _activation: Subscription,
}

impl EventEmitter<ReadyToRecord> for ScriptScreen {}

impl Focusable for ScriptScreen {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ScriptScreen {
    pub fn new(
        repo: PathBuf,
        slug: String,
        full_script: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Ask for changes, or type / for commands")
        });
        cx.subscribe(&input, |this, input, event: &InputEvent, cx| match event {
            InputEvent::PressEnter { .. } => {
                let text = input.read(cx).value().to_string();
                if !text.trim().is_empty() {
                    this.ask(&text, cx);
                }
            }
            InputEvent::Focus => {
                this.selected = None;
                cx.notify();
            }
            _ => {}
        })
        .detach();
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() && this.opened.is_some() {
                this.take_own_edit();
                cx.notify();
            }
        });
        let mut screen = Self {
            repo,
            slug,
            script: Err(String::new()),
            report: None,
            sources: Vec::new(),
            proposals: Vec::new(),
            selected: None,
            full_script,
            error: None,
            claude: Claude::find(),
            run: None,
            input,
            scroll: ScrollHandle::new(),
            target: None,
            opened: None,
            focus: cx.focus_handle(),
            _activation: activation,
        };
        screen.reload();
        screen
    }

    /// Reads the script and its proposals from git.
    pub fn reload(&mut self) {
        let board = match Board::open(&self.repo) {
            Ok(board) => board,
            Err(e) => {
                self.script = Err(e.to_string());
                return;
            }
        };
        let text = board.read_file(&self.slug, "script.md");
        if let (Some(opened), Ok(Some(text))) = (&self.opened, &text) {
            self.follow_branch(opened.clone(), text);
        }
        self.target = None;
        self.script = match text {
            Ok(Some(text)) => Script::parse(&text)
                .map_err(|e| format!("Line {} of the script: {}", e.line, e.message)),
            Ok(None) => Err(
                "No script yet. Give Claude a brief to draft one, or write your own with Edit \
                 script."
                    .to_owned(),
            ),
            Err(e) => Err(e.to_string()),
        };
        self.report = self.script.as_ref().ok().map(script_check::check_script);
        self.sources = self
            .script
            .as_ref()
            .map(|script| {
                let mut seen: Vec<Source> = Vec::new();
                for target in script
                    .chapters
                    .iter()
                    .flat_map(|c| &c.beats)
                    .flat_map(|b| &b.sources)
                {
                    if seen.iter().any(|s| &s.target == target) {
                        continue;
                    }
                    let kind = if target.starts_with("http") {
                        "Link".to_owned()
                    } else {
                        match board.read_file(&self.slug, target) {
                            Ok(Some(text)) => {
                                format!("Snapshot · {} words", text.split_whitespace().count())
                            }
                            _ => "Missing from sources/".to_owned(),
                        }
                    };
                    seen.push(Source {
                        target: target.clone(),
                        kind,
                    });
                }
                seen
            })
            .unwrap_or_default();
        self.proposals = board
            .proposals(Some(&self.slug))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| {
                let diff = board.proposal_diff(&p.slug, &p.name).ok()?;
                let file = diff.into_iter().find(|f| f.path == "script.md")?;
                Some(ScriptProposal {
                    changes: proposal::changes(&file.patch),
                    patch: file.patch,
                    name: p.name,
                    subject: p.subject,
                })
            })
            .collect();
        if self
            .selected
            .as_ref()
            .is_some_and(|s| !self.proposals.iter().any(|p| &p.name == s))
        {
            self.selected = None;
        }
    }

    fn ask(&mut self, text: &str, cx: &mut Context<Self>) {
        let command = match &self.script {
            Ok(script) if !script.chapters.is_empty() => ScriptCommand::parse(text),
            _ => ScriptCommand::Draft {
                brief: text.trim().to_owned(),
            },
        };
        self.start(text.trim().to_owned(), command, cx);
    }

    fn start(&mut self, request: String, command: ScriptCommand, cx: &mut Context<Self>) {
        let Some(claude) = self.claude.clone() else {
            return;
        };
        if self.run.as_ref().is_some_and(AgentRun::running) {
            return;
        }
        match AgentRun::start(
            claude,
            &self.repo,
            request,
            command.prompt(&self.slug),
            Reach::Web,
        ) {
            Ok(run) => {
                self.run = Some(run);
                agent::watch(
                    cx,
                    |this: &mut Self| this.run.as_mut(),
                    |this, _| this.reload(),
                );
            }
            Err(e) => self.error = Some(e),
        }
        cx.notify();
    }

    fn accept(&mut self, name: &str, cx: &mut Context<Self>) {
        let result = Board::open(&self.repo).and_then(|b| b.accept_proposal(&self.slug, name));
        if let Err(e) = result {
            self.error = Some(e.to_string());
        } else {
            self.error = None;
            if name.starts_with(HOOK) {
                for other in self
                    .proposals
                    .iter()
                    .filter(|p| p.name.starts_with(HOOK) && p.name != name)
                {
                    let _ = Board::open(&self.repo)
                        .and_then(|b| b.reject_proposal(&self.slug, &other.name));
                }
            }
        }
        self.reload();
        cx.notify();
    }

    fn reject(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Err(e) = Board::open(&self.repo).and_then(|b| b.reject_proposal(&self.slug, name)) {
            self.error = Some(e.to_string());
        }
        self.reload();
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
            "Another take".to_owned(),
            ScriptCommand::AnotherTake {
                proposal: name.to_owned(),
                diff: patch,
            },
            cx,
        );
    }

    fn on_accept(&mut self, _: &AcceptProposal, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected.clone() {
            Some(name) => self.accept(&name, cx),
            None => window.focus_next(cx),
        }
    }

    fn on_reject(&mut self, _: &RejectProposal, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(name) = self.selected.clone() {
            self.reject(&name, cx);
        }
    }

    /// The script in the video's folder: the file Edit script opens.
    fn file(&self) -> PathBuf {
        self.repo.join("videos").join(&self.slug).join("script.md")
    }

    /// Opens the script in the default editor, after putting the script this screen shows in
    /// the folder, so the editor never opens an older one.
    fn edit_script(&mut self, cx: &mut Context<Self>) {
        let file = self.file();
        let shown = Board::open(&self.repo)
            .ok()
            .and_then(|board| board.read_file(&self.slug, "script.md").ok().flatten());
        let on_disk = fs::read_to_string(&file).ok();
        let text = shown.or_else(|| on_disk.clone()).unwrap_or_default();
        let opened = if on_disk.as_ref() == Some(&text) {
            Ok(())
        } else {
            shot_fs::write(&file, &text)
        }
        .and_then(|()| {
            std::process::Command::new("open")
                .arg(&file)
                .spawn()
                .map(drop)
        });
        match opened {
            Ok(()) => {
                self.opened = Some(text);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("The script did not open in your editor: {e}")),
        }
        cx.notify();
    }

    /// Puts what was saved in the editor since it opened on the video's branch, as one commit.
    fn take_own_edit(&mut self) {
        let (Some(opened), Ok(text)) = (&self.opened, fs::read_to_string(self.file())) else {
            return;
        };
        if *opened == text {
            return;
        }
        let saved = Board::open(&self.repo).and_then(|board| {
            // A proposal left by an edit that failed to land would block this one.
            let _ = board.reject_proposal(&self.slug, OWN_EDIT);
            let files = BTreeMap::from([("script.md".to_owned(), text.clone())]);
            match board.create_proposal(&self.slug, OWN_EDIT, "📝 edit the script", &files) {
                Ok(_) => board.accept_proposal(&self.slug, OWN_EDIT).map(drop),
                Err(shot_board::Error::EmptyProposal) => Ok(()),
                Err(e) => Err(e),
            }
        });
        match saved {
            Ok(()) => {
                self.opened = Some(text);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Your edit did not reach the board: {e}")),
        }
        self.reload();
    }

    /// While the script is open in an editor, changes accepted here reach the file too, so the
    /// next save there does not undo them. A file with unsaved-here edits stays as it is.
    fn follow_branch(&mut self, opened: String, branch: &str) {
        let file = self.file();
        if opened != branch
            && fs::read_to_string(&file).is_ok_and(|text| text == opened)
            && shot_fs::write(&file, branch).is_ok()
        {
            self.opened = Some(branch.to_owned());
        }
    }

    /// Marks the script ready, as moving its card to Ready to record does, and moves on.
    fn continue_to_record(&mut self, _: &ContinueToRecord, _: &mut Window, cx: &mut Context<Self>) {
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

    /// Scrolls to the chapter of the beat on `line` and marks the beat.
    fn show_beat(&mut self, line: usize, cx: &mut Context<Self>) {
        let Ok(script) = &self.script else {
            return;
        };
        if let Some(index) = script
            .chapters
            .iter()
            .position(|c| c.beats.iter().any(|b| b.line == line))
        {
            self.scroll.scroll_to_top_of_item(index);
            self.target = Some(line);
            cx.notify();
        }
    }

    fn outline(&self, cx: &mut Context<Self>) -> Div {
        let mut column = pane()
            .w(px(268.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(14.));
        let Ok(script) = &self.script else {
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
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.scroll.scroll_to_top_of_item(index);
                                cx.notify();
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
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.show_beat(line, cx)),
                                )
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

    fn document(&self, cx: &mut Context<Self>) -> Div {
        let entity = cx.entity();
        let modes = vec![
            Segment::new("Full script", self.full_script, {
                let entity = entity.clone();
                move |_, _, cx| {
                    entity.update(cx, |this, cx| {
                        this.full_script = true;
                        cx.notify();
                    })
                }
            }),
            Segment::new("Beats", !self.full_script, move |_, _, cx| {
                entity.update(cx, |this, cx| {
                    this.full_script = false;
                    cx.notify();
                })
            }),
        ];
        let body: Vec<AnyElement> = match &self.script {
            Err(error) => vec![
                div()
                    .p(px(18.))
                    .text_color(MUTED)
                    .child(error.clone())
                    .into_any_element(),
            ],
            Ok(script) => {
                let report = self.report.as_ref().expect("a parsed script has a report");
                let inline: Vec<(&ScriptProposal, &Change)> = self
                    .proposals
                    .iter()
                    .filter(|p| !p.name.starts_with(HOOK))
                    .flat_map(|p| p.changes.iter().map(move |c| (p, c)))
                    .collect();
                let mut shown = vec![false; inline.len()];
                let mut sections = Vec::new();
                // Each beat owns the lines from the end of the beat before it up to the next
                // beat, so every changed line lands at exactly one beat.
                let lines: Vec<usize> = script
                    .chapters
                    .iter()
                    .flat_map(|c| &c.beats)
                    .map(|b| b.line)
                    .collect();
                let mut beat_number = 0;
                let mut start = 0;
                for (chapter, length) in script.chapters.iter().zip(&report.chapters) {
                    let mut section = div()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .px(px(18.))
                        .py(px(16.))
                        .rounded(RADIUS_MD + px(2.))
                        .bg(CARD)
                        .child(
                            div()
                                .flex()
                                .items_baseline()
                                .gap(px(10.))
                                .child(
                                    div()
                                        .text_size(px(17.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(STRONG)
                                        .child(chapter.title.clone()),
                                )
                                .child(div().flex_1())
                                .child(mono(clock(*length)).text_size(TEXT_SM).text_color(MUTED)),
                        );
                    for beat in &chapter.beats {
                        beat_number += 1;
                        let end = lines.get(beat_number).copied().unwrap_or(usize::MAX);
                        let here: Vec<usize> = (0..inline.len())
                            .filter(|&i| !shown[i] && (start..end).contains(&inline[i].1.line))
                            .collect();
                        // A beat a proposal replaces shows struck inside the proposal instead.
                        let replaced = inline
                            .iter()
                            .any(|(_, c)| (c.line..c.end).contains(&beat.line));
                        if !replaced {
                            let marked = self.target == Some(beat.line);
                            section = section.child(beat_view(beat, self.full_script, marked));
                        }
                        for i in here {
                            shown[i] = true;
                            let (proposal, change) = inline[i];
                            section = section.child(self.proposal_card(proposal, change, cx));
                        }
                        start = end;
                    }
                    sections.push(section.into_any_element());
                }
                for i in (0..inline.len()).filter(|&i| !shown[i]) {
                    let (proposal, change) = inline[i];
                    sections.push(self.proposal_card(proposal, change, cx).into_any_element());
                }
                sections
            }
        };
        pane()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .child(segmented("script-mode", SUNK, modes).w(px(170.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(14.))
                            .text_color(MUTED)
                            .child("Beats for reactions, full script for tours"),
                    )
                    .child(
                        proposal_action(
                            "edit-script",
                            "Edit script",
                            Some("edit"),
                            None,
                            ActionLook::Plain,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.edit_script(cx))),
                    ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_color(CUT_TEXT).child(error))
            })
            .child(
                div()
                    .id("script-document")
                    .track_scroll(&self.scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .children(body),
            )
    }

    fn proposal_card(
        &self,
        proposal: &ScriptProposal,
        change: &Change,
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
        let pieces = change.pieces.iter().map(|piece| match piece {
            Piece::Same(text) => div().text_color(BODY).child(text.clone()),
            Piece::Removed(text) => div()
                .text_color(STRUCK)
                .line_through()
                .text_decoration_color(CUT)
                .child(text.clone()),
            Piece::Added(text) => div().text_color(INSERTED).child(text.clone()),
        });
        let action = |id: &'static str, text, keys, look| {
            proposal_action(format!("{id}-{}", proposal.name), text, None, keys, look)
        };
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
                    this.selected = Some(name.clone());
                    window.focus(&this.focus, cx);
                    cx.notify();
                }
            }))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(label(format!("Claude suggests · {}", proposal.subject)))
                    .when(count > 1, |this| {
                        this.child(
                            div()
                                .text_size(TEXT_SM)
                                .text_color(FAINT)
                                .child(format!("{} of {count}, accepted together", position + 1)),
                        )
                    })
                    .child(div().flex_1())
                    .child(div().text_size(TEXT_SM).text_color(MUTED).child(saved_text)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(5.))
                    .line_height(px(24.))
                    .children(pieces),
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

    fn agent_pane(&self, cx: &mut Context<Self>) -> Div {
        let hooks: Vec<&ScriptProposal> = self
            .proposals
            .iter()
            .filter(|p| p.name.starts_with(HOOK))
            .collect();
        let body = match &self.claude {
            None => agent::setup_view(cx.listener(|this, _, _, cx| {
                this.claude = Claude::find();
                cx.notify();
            })),
            Some(_) => div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .when(self.run.is_none() && hooks.is_empty(), |this| {
                    let drafted = self.script.as_ref().is_ok_and(|s| !s.chapters.is_empty());
                    this.child(div().text_color(MUTED).line_height(px(22.)).child(if drafted {
                        "Ask for a change below, or pick a command. Claude's changes show up \
                         in the script as suggestions you accept or reject."
                    } else {
                        "Write a brief below: what the video is about, your take and how long \
                         it should run. Claude drafts the script as a suggestion you accept or \
                         reject."
                    }))
                })
                .when_some(self.run.as_ref(), |this, run| {
                    this.child(agent::steps_view(run))
                })
                .when(!hooks.is_empty(), |this| {
                    this.child(div().text_color(BODY).child("Pick a hook:"))
                        .children(hooks.into_iter().map(|hook| {
                            let name = hook.name.clone();
                            let text = hook
                                .changes
                                .first()
                                .map(Change::added_text)
                                .unwrap_or_else(|| hook.subject.clone());
                            div()
                                .id(SharedString::from(format!("hook-option-{name}")))
                                .p(px(14.))
                                .rounded(RADIUS_MD)
                                .bg(RAISED)
                                .text_color(TEXT)
                                .cursor_pointer()
                                .hover(|s| {
                                    s.border_1().border_color(Hsla::from(ACCENT).opacity(0.5))
                                })
                                .child(text)
                                .on_click(cx.listener(move |this, _, _, cx| this.accept(&name, cx)))
                        }))
                }),
        };
        let running = self.run.as_ref().is_some_and(AgentRun::running);
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
                    this.start(command.to_owned(), ScriptCommand::parse(command), cx)
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("script")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_accept))
            .on_action(cx.listener(Self::on_reject))
            .on_action(cx.listener(Self::continue_to_record))
            .size_full()
            .flex()
            .gap(px(8.))
            .p(px(8.))
            .pt(px(0.))
            .child(self.outline(cx))
            .child(self.document(cx))
            .child(self.agent_pane(cx))
    }
}

/// A beat, `marked` with the warning tint when a check finding pointed at it.
fn beat_view(beat: &shot_core::Beat, full_script: bool, marked: bool) -> Div {
    let visual = beat.text.to_lowercase().starts_with("b-roll");
    div()
        .flex()
        .gap(px(10.))
        .when(marked, |this| {
            this.mx(px(-8.))
                .px(px(8.))
                .my(px(-4.))
                .py(px(4.))
                .rounded(RADIUS_SM)
                .bg(WARN_TINT)
        })
        .child(
            div()
                .flex_none()
                .size(px(6.))
                .mt(px(9.))
                .rounded_full()
                .bg(if marked { WARN } else { FAINT }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .line_height(px(24.))
                .text_color(if visual { MUTED } else { BODY })
                .child(beat.text.clone())
                .when(full_script && !beat.script.is_empty(), |this| {
                    this.child(div().text_color(MUTED).child(beat.script.clone()))
                }),
        )
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
