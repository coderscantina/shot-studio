//! Claude runs for the Board and Script screens: the `claude` bridge on a background thread,
//! its steps collected for the screen to show, and the setup notice when Claude Code is missing.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use gpui::{Context, Div, FontWeight, SharedString, div, prelude::*, px};
use shot_mcp::bridge::{Claude, Session, Step, new_log_dir};

use crate::theme::*;
use crate::ui::{icon, mono};

/// How often a running agent's steps are picked up.
pub const POLL: Duration = Duration::from_millis(100);

enum Event {
    Step(Step),
    Done(Result<String, String>),
}

/// One agent run and what it did so far.
pub struct AgentRun {
    /// What the person asked, shown as the first bubble.
    pub request: String,
    pub steps: Vec<Step>,
    /// Claude's answer, or why the run failed. `None` while it runs.
    pub outcome: Option<Result<String, String>>,
    events: Receiver<Event>,
}

impl AgentRun {
    /// Starts `prompt` against the channel repo `repo`. `request` is how the screen shows it.
    pub fn start(
        claude: Claude,
        repo: &Path,
        request: String,
        prompt: String,
    ) -> Result<Self, String> {
        let session = Session {
            log_dir: new_log_dir(repo),
            repo: repo.to_owned(),
            shot: shot_executable()?,
            prompt,
            frame_task: false,
        };
        let (send, events) = mpsc::channel();
        std::thread::spawn(move || {
            let steps = send.clone();
            let result = session
                .run(&claude, |step| {
                    let _ = steps.send(Event::Step(step));
                })
                .map_err(|e| e.to_string());
            let _ = send.send(Event::Done(result));
        });
        Ok(Self {
            request,
            steps: Vec::new(),
            outcome: None,
            events,
        })
    }

    pub fn running(&self) -> bool {
        self.outcome.is_none()
    }

    /// Takes the steps that arrived. Returns true when the run just ended.
    pub fn poll(&mut self) -> bool {
        loop {
            match self.events.try_recv() {
                Ok(Event::Step(step)) => self.steps.push(step),
                Ok(Event::Done(result)) => {
                    self.outcome = Some(result);
                    return true;
                }
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => {
                    self.outcome = Some(Err("the agent stopped unexpectedly".to_owned()));
                    return true;
                }
            }
        }
    }

    /// The steps as the screen lists them: one line per tool call, done or in progress.
    pub fn lines(&self) -> Vec<(String, bool)> {
        let calls: Vec<&String> = self
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::ToolUse { name, .. } => Some(name),
                _ => None,
            })
            .collect();
        let answered = self
            .steps
            .iter()
            .filter(|s| matches!(s, Step::ToolResult { .. }))
            .count();
        calls
            .iter()
            .enumerate()
            .map(|(i, name)| (describe_tool(name), i < answered || !self.running()))
            .collect()
    }
}

/// Polls `run` on the screen until it ends, then calls `ended`.
pub fn watch<V: 'static>(
    cx: &mut Context<V>,
    run: impl Fn(&mut V) -> Option<&mut AgentRun> + 'static,
    ended: impl Fn(&mut V, &mut Context<V>) + 'static,
) {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(POLL).await;
            let Ok(done) = this.update(cx, |view, cx| {
                let done = run(view).is_none_or(|r| r.poll() || !r.running());
                if done {
                    ended(view, cx);
                }
                cx.notify();
                done
            }) else {
                break;
            };
            if done {
                break;
            }
        }
    })
    .detach();
}

/// A tool call in plain words.
fn describe_tool(name: &str) -> String {
    let tool = name.strip_prefix("mcp__shot__").unwrap_or(name);
    match tool {
        "list_videos" => "Listed the videos",
        "read_video" => "Read the video",
        "read_transcript" => "Read the transcript",
        "script_checks" => "Ran the script checks",
        "propose_script" => "Proposed a script change",
        "propose_cut" => "Proposed a cut",
        "proposal_diff" => "Read a proposal",
        "list_proposals" => "Listed the proposals",
        "move_card" => "Moved a card",
        "activity" => "Read the activity",
        "review_queue" => "Read the review queue",
        "caption_fixes" => "Read the caption fixes",
        "describe" => "Drafted the description",
        other => other,
    }
    .to_owned()
}

/// The `shot` executable that serves MCP, next to the app.
fn shot_executable() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let shot = exe.with_file_name("shot");
    if shot.exists() {
        Ok(shot)
    } else {
        Err(format!(
            "the shot command is missing next to the app at {}",
            shot.display()
        ))
    }
}

/// The step list of a run: the request bubble, done steps with a check, the running step lime.
pub fn steps_view(run: &AgentRun) -> Div {
    let lines = run.lines();
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(
            div()
                .p(px(14.))
                .rounded(RADIUS_MD)
                .bg(RAISED)
                .text_color(TEXT)
                .child(run.request.clone()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .children(lines.into_iter().map(|(line, done)| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .text_size(px(14.))
                        .text_color(if done { KBD_TEXT } else { ACCENT })
                        .child(if done {
                            icon("check", ICON, MUTED).into_any_element()
                        } else {
                            div()
                                .size(px(7.))
                                .mx(px(5.))
                                .rounded_full()
                                .bg(ACCENT)
                                .into_any_element()
                        })
                        .child(line)
                })),
        )
        .when(run.running(), |this| {
            this.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(px(14.))
                    .text_color(ACCENT)
                    .child(div().size(px(7.)).mx(px(5.)).rounded_full().bg(ACCENT))
                    .child("Working"),
            )
        })
        .when_some(run.outcome.as_ref(), |this, outcome| match outcome {
            // Bold marks are the Markdown Claude uses most; the pane shows plain text.
            Ok(answer) => this.child(div().text_color(BODY).child(answer.replace("**", ""))),
            Err(error) => this.child(div().text_color(CUT_TEXT).child(error.clone())),
        })
}

/// What the person sees instead of AI features when Claude Code is not installed.
pub fn setup_view(
    on_retry: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> Div {
    let step = |n: &'static str, text: SharedString| {
        div()
            .flex()
            .gap(px(10.))
            .child(div().text_color(MUTED).child(n))
            .child(div().flex_1().child(text))
    };
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .p(px(16.))
        .rounded(RADIUS_MD)
        .bg(RAISED)
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(STRONG)
                .child("Set up Claude Code"),
        )
        .child(
            div().text_color(BODY).child(
                "Shot Studio asks Claude through Claude Code on this Mac. It stores no API key, and only text leaves the machine. Everything else works without it.",
            ),
        )
        .child(step("1", "Install Claude Code from claude.com/claude-code.".into()))
        .child(step(
            "2",
            "Run claude once in Terminal and sign in.".into(),
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(mono("claude").text_size(TEXT_SM).text_color(MUTED))
                .child(div().text_size(TEXT_SM).text_color(MUTED).child("is found on PATH, in ~/.claude/local or Homebrew")),
        )
        .child(
            div()
                .id("claude-retry")
                .flex_none()
                .h(px(34.))
                .px(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(9.))
                .bg(FLOAT)
                .cursor_pointer()
                .hover(|s| s.bg(FLOAT_ON))
                .font_weight(FontWeight::MEDIUM)
                .child("Check again")
                .on_click(on_retry),
        )
}
