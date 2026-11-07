//! The Script screen: the outline, sources and script checks, the script as a document, and the
//! script agent.
//!
//! The script is read from the video's branch through `shot-board`. Claude changes it only by
//! proposing ([0004]): each proposal shows inline where it applies as struck and inserted
//! words with Accept (Tab), Reject (Esc) and Another take. Tab and Esc act only while a
//! proposal is selected. Proposals named `hook-*` are alternative hooks, shown as options in
//! the agent pane; picking one accepts it and rejects the others. The branch stays out of
//! sight.
//!
//! [0004]: ../../../docs/decisions/0004-proposals-as-branches.md

pub mod proposal;

use std::path::PathBuf;

use gpui::{
    App, Context, Div, Entity, FocusHandle, Focusable, FontWeight, Hsla, KeyBinding, SharedString,
    Window, actions, div, prelude::*, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use shot_board::Board;
use shot_core::{Micros, Script};
use shot_mcp::agents::ScriptCommand;
use shot_mcp::bridge::Claude;
use shot_ship::script_check::{self, ScriptFinding, ScriptReport};

use crate::agent::{self, AgentRun};
use crate::theme::*;
use crate::ui::{Segment, icon, kbd, label, mono, pane, segmented};
use proposal::{Change, Piece};

const CONTEXT: &str = "ScriptScreen";
/// Proposals with this name prefix are alternative hooks.
const HOOK: &str = "hook-";

actions!(script, [AcceptProposal, RejectProposal]);

pub fn bind_keys(cx: &mut App) {
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("tab", AcceptProposal, context),
        KeyBinding::new("escape", RejectProposal, context),
    ]);
}

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
    focus: FocusHandle,
}

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
            focus: cx.focus_handle(),
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
        self.script = match board.read_file(&self.slug, "script.md") {
            Ok(Some(text)) => Script::parse(&text).map_err(|e| format!("script.md:{e}")),
            Ok(None) => Err("no script.md yet: give Claude a brief to draft one".to_owned()),
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
        match AgentRun::start(claude, &self.repo, request, command.prompt(&self.slug)) {
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

    fn outline(&self) -> Div {
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
                    .map(|(chapter, length)| {
                        div()
                            .flex()
                            .justify_between()
                            .px(px(10.))
                            .py(px(9.))
                            .rounded(RADIUS_SM)
                            .text_color(BODY)
                            .child(chapter.title.clone())
                            .child(mono(clock(*length)).text_size(TEXT_SM).text_color(MUTED))
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
            .children(summarize(&report.findings).into_iter().map(|text| {
                div()
                    .mt(px(6.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(RADIUS_MD)
                    .bg(WARN_TINT)
                    .text_size(px(14.))
                    .text_color(WARN)
                    .child(text)
            }))
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
        let body = match &self.script {
            Err(error) => div().p(px(18.)).text_color(MUTED).child(error.clone()),
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
                            section = section.child(beat_view(beat, self.full_script));
                        }
                        for i in here {
                            shown[i] = true;
                            let (proposal, change) = inline[i];
                            section = section.child(self.proposal_card(proposal, change, cx));
                        }
                        start = end;
                    }
                    sections.push(section);
                }
                let rest: Vec<_> = (0..inline.len()).filter(|&i| !shown[i]).collect();
                let mut column = div().flex().flex_col().gap(px(8.)).children(sections);
                for i in rest {
                    let (proposal, change) = inline[i];
                    column = column.child(self.proposal_card(proposal, change, cx));
                }
                column
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
                            .text_size(px(14.))
                            .text_color(MUTED)
                            .child("Beats for reactions, full script for tours"),
                    ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_color(CUT_TEXT).child(error))
            })
            .child(
                div()
                    .id("script-document")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
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
        let action =
            |id: &'static str, text: &'static str, keys: Option<&'static str>, accent: bool| {
                div()
                    .id(SharedString::from(format!("{id}-{}", proposal.name)))
                    .h(px(30.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(9.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_size(px(14.))
                    .cursor_pointer()
                    .when(accent, |this| this.bg(ACCENT).text_color(ON_ACCENT))
                    .when(!accent && keys.is_some(), |this| {
                        this.bg(RAISED).hover(|s| s.bg(FLOAT))
                    })
                    .when(keys.is_none(), |this| {
                        this.text_color(MUTED).hover(|s| s.text_color(TEXT))
                    })
                    .child(text)
                    .when_some(keys, |this, keys| {
                        this.child(if accent {
                            kbd(keys)
                                .bg(Hsla::from(ON_ACCENT).opacity(0.14))
                                .text_color(ON_ACCENT)
                        } else {
                            kbd(keys)
                        })
                    })
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
                        action("accept", "Accept", Some("Tab"), true).on_click(cx.listener({
                            let name = name.clone();
                            move |this, _, _, cx| this.accept(&name, cx)
                        })),
                    )
                    .child(
                        action("reject", "Reject", Some("Esc"), false).on_click(cx.listener({
                            let name = name.clone();
                            move |this, _, _, cx| this.reject(&name, cx)
                        })),
                    )
                    .child(
                        action("again", "Another take", None, false).on_click(
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
                .px(px(12.))
                .py(px(6.))
                .rounded(RADIUS_SM)
                .bg(RAISED)
                .text_size(px(14.))
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
                            .child("Claude · proposes changes to script.md"),
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
            .child(div().flex().flex_wrap().gap(px(8.)).children(chips))
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
            .size_full()
            .flex()
            .gap(px(8.))
            .p(px(8.))
            .pt(px(0.))
            .child(self.outline())
            .child(self.document(cx))
            .child(self.agent_pane(cx))
    }
}

fn beat_view(beat: &shot_core::Beat, full_script: bool) -> Div {
    let visual = beat.text.to_lowercase().starts_with("b-roll");
    div()
        .flex()
        .gap(px(10.))
        .child(
            div()
                .flex_none()
                .size(px(6.))
                .mt(px(9.))
                .rounded_full()
                .bg(FAINT),
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

/// One line per problem, with beats without a source counted together.
fn summarize(findings: &[ScriptFinding]) -> Vec<String> {
    let mut out = Vec::new();
    let unsourced: Vec<usize> = findings
        .iter()
        .filter_map(|f| match f {
            ScriptFinding::NoSource { line, .. } => Some(*line),
            _ => None,
        })
        .collect();
    for finding in findings {
        match finding {
            ScriptFinding::LongHook { spoken } => out.push(format!(
                "The hook takes {} to say. Aim for under 15 s.",
                clock(*spoken)
            )),
            ScriptFinding::NoVisual {
                chapter, spoken, ..
            } => out.push(format!(
                "{chapter} runs {} without a visual. Add b-roll notes?",
                clock(*spoken)
            )),
            ScriptFinding::NoSource { .. } => {}
        }
    }
    match unsourced.as_slice() {
        [] => {}
        [line] => out.push(format!("The beat on line {line} has no source.")),
        [first, ..] => out.push(format!(
            "{} beats have no source, the first on line {first}.",
            unsourced.len()
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
