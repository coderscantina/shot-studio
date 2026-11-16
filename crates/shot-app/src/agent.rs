//! Claude runs for the Board and Script screens: the `claude` bridge on a background thread,
//! its steps collected for the screen to show, a run as one exchange of a conversation, and the
//! setup notice when Claude Code is missing.
//!
//! An exchange shows the request as a bubble on the right, the steps live while the run works
//! (with Stop), folded into one line once it ends, and the answer as selectable Markdown with a
//! Copy button.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use gpui::{
    App, ClipboardItem, Context, Div, FontWeight, SharedString, Window, div, prelude::*, px,
    relative,
};
use gpui_component::text::TextView;
use shot_mcp::bridge::{Claude, Reach, Session, Step, Stop, new_log_dir};

use crate::theme::*;
use crate::ui::{ActionLook, icon, mono, proposal_action};

/// How often a running agent's steps are picked up.
pub const POLL: Duration = Duration::from_millis(100);

enum Event {
    Step(Step),
    Done(Result<String, String>),
}

/// One agent run and what it did so far.
pub struct AgentRun {
    /// Tells runs apart for the state their view keeps.
    id: usize,
    /// What the person asked, shown as the first bubble.
    pub request: String,
    pub steps: Vec<Step>,
    /// Claude's answer, or why the run failed. `None` while it runs.
    pub outcome: Option<Result<String, String>>,
    events: Receiver<Event>,
    stop: Stop,
}

impl AgentRun {
    /// Starts `prompt` against the channel repo `repo`. `request` is how the screen shows it.
    /// A [`Reach::FrameTask`] only starts from an explicit action of the person.
    pub fn start(
        claude: Claude,
        repo: &Path,
        request: String,
        prompt: String,
        reach: Reach,
    ) -> Result<Self, String> {
        let session = Session {
            log_dir: new_log_dir(repo),
            repo: repo.to_owned(),
            shot: shot_executable()?,
            prompt,
            reach,
        };
        static RUNS: AtomicUsize = AtomicUsize::new(0);
        let (send, events) = mpsc::channel();
        let stop = Stop::default();
        let stopping = stop.clone();
        std::thread::spawn(move || {
            let steps = send.clone();
            let result = session
                .run_until(&claude, &stopping, |step| {
                    let _ = steps.send(Event::Step(step));
                })
                .map_err(|e| e.to_string());
            let _ = send.send(Event::Done(result));
        });
        Ok(Self {
            id: RUNS.fetch_add(1, Ordering::Relaxed),
            request,
            steps: Vec::new(),
            outcome: None,
            events,
            stop,
        })
    }

    pub fn running(&self) -> bool {
        self.outcome.is_none()
    }

    /// Kills the run's `claude` process. It ends as stopped on the next poll.
    pub fn stop(&self) {
        self.stop.stop();
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

/// The tool calls of an ended run in one line, such as "Read 3 pages, proposed 4 changes".
fn summary(steps: &[Step]) -> Option<String> {
    let (mut pages, mut searches, mut proposals, mut other) = (0, 0, 0, 0);
    for step in steps {
        let Step::ToolUse { name, .. } = step else {
            continue;
        };
        match tool_name(name) {
            "WebFetch" => pages += 1,
            "WebSearch" => searches += 1,
            tool if tool.starts_with("propose_") => proposals += 1,
            _ => other += 1,
        }
    }
    let count =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut parts: Vec<String> = [
        (pages > 0).then(|| format!("read {}", count(pages, "page", "pages"))),
        (searches > 0).then(|| format!("ran {}", count(searches, "search", "searches"))),
        (proposals > 0).then(|| format!("proposed {}", count(proposals, "change", "changes"))),
    ]
    .into_iter()
    .flatten()
    .collect();
    if other > 0 {
        parts.push(if parts.is_empty() {
            format!("took {}", count(other, "step", "steps"))
        } else {
            count(other, "more step", "more steps")
        });
    }
    let line = parts.join(", ");
    let mut chars = line.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

fn tool_name(name: &str) -> &str {
    name.strip_prefix("mcp__shot__").unwrap_or(name)
}

/// A tool call in plain words.
fn describe_tool(name: &str) -> String {
    let tool = tool_name(name);
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
        "WebSearch" => "Searched the web",
        "WebFetch" => "Read a web page",
        other => other,
    }
    .to_owned()
}

/// The `shot` executable that serves MCP, next to the app.
pub fn shot_executable() -> Result<PathBuf, String> {
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

/// A run as one exchange of a conversation: the request bubble on the right, the steps, and the
/// answer or why the run failed. While the run works the steps show live with Stop; once it ends
/// they fold into one line that opens on a click.
pub fn run_view(run: &AgentRun, window: &mut Window, cx: &mut App) -> Div {
    let open = window.use_keyed_state(("agent-steps-open", run.id), cx, |_, _| false);
    let lines = run.lines();
    let folded = !run.running() && !*open.read(cx);
    let step = |line: String, done: bool| {
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .text_size(px(14.))
            .text_color(if done { KBD_TEXT } else { ACCENT })
            .child(if done {
                icon("check", ICON, MUTED).into_any_element()
            } else {
                dot().into_any_element()
            })
            .child(line)
    };
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(
            div().flex().justify_end().child(
                div()
                    .max_w(relative(0.85))
                    .px(px(14.))
                    .py(px(10.))
                    .rounded(RADIUS_MD)
                    .bg(RAISED)
                    .text_color(TEXT)
                    .child(run.request.clone()),
            ),
        )
        .when_some(
            summary(&run.steps).filter(|_| !run.running()),
            |this, line| {
                let chevron = if folded {
                    "chevron-right"
                } else {
                    "chevron-down"
                };
                this.child(
                    div()
                        .id(("agent-steps-toggle", run.id))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .text_size(px(14.))
                        .text_color(KBD_TEXT)
                        .cursor_pointer()
                        .hover(|s| s.text_color(TEXT))
                        .child(icon(chevron, ICON, MUTED))
                        .child(line)
                        .on_click(move |_, _, cx| {
                            open.update(cx, |open, cx| {
                                *open = !*open;
                                cx.notify();
                            })
                        }),
                )
            },
        )
        .when(!folded, |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .children(lines.into_iter().map(|(line, done)| step(line, done))),
            )
        })
        .when(run.running(), |this| {
            let stop = run.stop.clone();
            this.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(step("Working".to_owned(), false))
                    .child(
                        proposal_action(
                            format!("agent-stop-{}", run.id),
                            "Stop",
                            Some("close"),
                            None,
                            ActionLook::Plain,
                        )
                        .on_click(move |_, _, _| stop.stop()),
                    ),
            )
        })
        .when_some(run.outcome.as_ref(), |this, outcome| match outcome {
            Ok(answer) => {
                let copied = answer.clone();
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .text_color(BODY)
                        .line_height(px(22.))
                        .child(
                            TextView::markdown(("agent-answer", run.id), answer.clone())
                                .selectable(true),
                        )
                        .child(
                            div().flex().child(
                                proposal_action(
                                    format!("agent-copy-{}", run.id),
                                    "Copy",
                                    Some("copy"),
                                    None,
                                    ActionLook::Quiet,
                                )
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))
                                }),
                            ),
                        ),
                )
            }
            Err(_) if run.stop.asked() => this.child(div().text_color(MUTED).child("Stopped")),
            Err(error) => this.child(div().text_color(CUT_TEXT).child(error.clone())),
        })
}

/// The lime dot of a step in progress.
fn dot() -> Div {
    div().size(px(7.)).mx(px(5.)).rounded_full().bg(ACCENT)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ended_runs_steps_fold_into_one_line() {
        let call = |name: &str| Step::ToolUse {
            name: name.to_owned(),
            input: "{}".to_owned(),
        };
        let steps = [
            call("mcp__shot__read_video"),
            call("WebFetch"),
            call("WebFetch"),
            call("mcp__shot__propose_script"),
            Step::Text("Done.".to_owned()),
        ];
        assert_eq!(
            summary(&steps).as_deref(),
            Some("Read 2 pages, proposed 1 change, 1 more step")
        );
        assert_eq!(
            summary(&[call("mcp__shot__script_checks")]).as_deref(),
            Some("Took 1 step")
        );
        assert_eq!(summary(&[]), None);
    }
}
