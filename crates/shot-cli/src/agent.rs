//! `shot agent` and `shot mcp`: Claude through the local `claude` CLI, and the server it uses.

use std::path::{Path, PathBuf};

use clap::Args;
use shot_mcp::agents::{ScriptCommand, plan_week};
use shot_mcp::bridge::{Claude, Session, Step, new_log_dir};

#[derive(Args)]
pub struct McpArgs {
    /// Append every byte sent to the client to this file.
    #[arg(long)]
    log: Option<PathBuf>,
    /// Allow the frame task tools. The app sets this only for a thumbnail pick a person started.
    #[arg(long)]
    frame_task: bool,
}

pub fn mcp(repo: PathBuf, args: McpArgs) -> Result<(), String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(shot_mcp::serve_stdio(
            repo,
            shot_mcp::ServeOptions {
                frame_task: args.frame_task,
                log: args.log,
            },
        ))
        .map_err(|e| e.to_string())
}

#[derive(Args)]
pub struct AgentArgs {
    /// The video the request is about. Without it the request is about the board.
    #[arg(long)]
    video: Option<String>,
    /// Plan the week: which ideas to script next.
    #[arg(long, conflicts_with = "request")]
    plan_week: bool,
    /// Videos published per week, for --plan-week.
    #[arg(long, default_value_t = 2)]
    cadence: u8,
    /// What to ask, or a slash command such as `/tighten`.
    request: Option<String>,
}

/// Streams the steps to the terminal, then prints Claude's answer.
pub fn run(repo: &Path, args: AgentArgs) -> Result<(), String> {
    let claude = Claude::find().ok_or(
        "Claude Code is not installed: install it from https://claude.com/claude-code and sign in, then run this again",
    )?;
    let prompt = match (&args.video, args.request) {
        _ if args.plan_week => plan_week(args.cadence),
        (Some(slug), Some(request)) => ScriptCommand::parse(&request).prompt(slug),
        (None, Some(request)) => request,
        (_, None) => return Err("say what to ask, or use --plan-week".to_owned()),
    };
    let repo = std::path::absolute(repo).map_err(|e| e.to_string())?;
    let session = Session {
        log_dir: new_log_dir(&repo),
        repo,
        shot: std::env::current_exe().map_err(|e| e.to_string())?,
        prompt,
        frame_task: false,
    };
    eprintln!("log: {}", session.log_dir.display());
    let answer = session
        .run(&claude, |step| match step {
            Step::Started { model, tools } => eprintln!("{model} with {}", tools.join(", ")),
            Step::ToolUse { name, input } => eprintln!("-> {name} {input}"),
            Step::ToolResult { error, text } => eprintln!(
                "   {}{}",
                if error { "error: " } else { "" },
                text.lines().next().unwrap_or_default()
            ),
            Step::Text(_) | Step::Finished { .. } => {}
        })
        .map_err(|e| e.to_string())?;
    println!("{answer}");
    Ok(())
}
