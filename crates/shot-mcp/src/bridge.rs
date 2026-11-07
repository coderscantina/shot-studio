//! The `claude` bridge ([0003]): runs the local Claude Code CLI headless with the `shot` MCP
//! server attached and turns its stream into [`Step`]s for the app or the terminal.
//!
//! Shot Studio stores no API key: Claude Code signs in on its own. The session is fenced to text
//! ([0005]). Claude Code's built-in tools are off (`--tools ""`), so it cannot read files or run
//! commands, only the `shot` tools; other MCP servers, settings files and slash commands are
//! ignored, it runs in an empty folder, and its telemetry is off. Every byte that goes to Claude
//! is logged in the session folder: the prompt in `sent.txt`, every MCP reply in `mcp.jsonl`
//! (written by the server itself), and Claude's stream in `steps.jsonl`.
//!
//! [0003]: ../../docs/decisions/0003-claude-through-local-cli.md
//! [0005]: ../../docs/decisions/0005-privacy-text-only-to-claude.md

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// Overrides where the `claude` executable is, for tests and unusual installs.
pub const CLAUDE_ENV: &str = "SHOT_CLAUDE";

/// The installed Claude Code CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claude {
    path: PathBuf,
}

impl Claude {
    /// Finds `claude`: `$SHOT_CLAUDE`, then `PATH`, then the places its installers use, since an
    /// app started from Finder gets a minimal `PATH`. `None` means the setup screen.
    pub fn find() -> Option<Self> {
        if let Some(path) = std::env::var_os(CLAUDE_ENV) {
            let path = PathBuf::from(path);
            return is_executable(&path).then_some(Self { path });
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        if let Some(home) = &home {
            dirs.push(home.join(".claude/local"));
            dirs.push(home.join(".local/bin"));
        }
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
        dirs.into_iter()
            .map(|dir| dir.join("claude"))
            .find(|path| is_executable(path))
            .map(|path| Self { path })
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// One agent run.
#[derive(Debug, Clone)]
pub struct Session {
    /// The channel repo the `shot` server serves.
    pub repo: PathBuf,
    /// The `shot` executable that runs the MCP server.
    pub shot: PathBuf,
    pub prompt: String,
    /// Starts the server with the frame task tools. Only for a run a person started on purpose.
    pub frame_task: bool,
    /// A new, empty folder for the session's logs. Claude runs in it.
    pub log_dir: PathBuf,
}

/// What Claude does, as it happens.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// The session started with these tools.
    Started { model: String, tools: Vec<String> },
    /// Claude says something.
    Text(String),
    /// Claude calls a tool. `input` is the arguments as compact JSON.
    ToolUse { name: String, input: String },
    /// A tool replied.
    ToolResult { error: bool, text: String },
    /// The run ended. `text` is Claude's final answer.
    Finished { text: String, cost_usd: Option<f64> },
}

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Claude Code stopped: {0}")]
    Claude(String),
}

impl Session {
    /// The arguments `claude` runs with. The prompt goes over stdin, not here.
    pub fn args(&self) -> Vec<String> {
        let mut server_args = vec![
            "mcp".to_owned(),
            "--repo".to_owned(),
            self.repo.display().to_string(),
            "--log".to_owned(),
            self.log_dir.join("mcp.jsonl").display().to_string(),
        ];
        if self.frame_task {
            server_args.push("--frame-task".to_owned());
        }
        let config = json!({
            "mcpServers": {
                "shot": {
                    "type": "stdio",
                    "command": self.shot.display().to_string(),
                    "args": server_args,
                }
            }
        });
        [
            "--print",
            "--output-format",
            "stream-json",
            "--verbose",
            "--no-session-persistence",
            "--disable-slash-commands",
            "--setting-sources",
            "",
            "--strict-mcp-config",
            "--mcp-config",
            &config.to_string(),
            "--tools",
            "",
            "--allowedTools",
            "mcp__shot",
            "--permission-mode",
            "dontAsk",
            "--append-system-prompt",
            crate::agents::SYSTEM,
        ]
        .map(str::to_owned)
        .to_vec()
    }

    /// Runs Claude to the end, calling `on_step` for every step. Returns the final answer.
    pub fn run(
        &self,
        claude: &Claude,
        mut on_step: impl FnMut(Step),
    ) -> Result<String, BridgeError> {
        let io = |path: &Path| {
            let path = path.to_owned();
            move |source| BridgeError::Io { path, source }
        };
        fs::create_dir_all(&self.log_dir).map_err(io(&self.log_dir))?;
        let args = self.args();
        let sent = self.log_dir.join("sent.txt");
        fs::write(
            &sent,
            format!(
                "argv: {}\n{}\nprompt over stdin:\n{}\n",
                claude.path.display(),
                args.iter()
                    .map(|a| format!("  {a:?}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                self.prompt
            ),
        )
        .map_err(io(&sent))?;
        let mut child = Command::new(&claude.path)
            .args(&args)
            .current_dir(&self.log_dir)
            // No telemetry, error reports or update checks from Claude Code either ([0005]).
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env("DISABLE_TELEMETRY", "1")
            .env("DISABLE_ERROR_REPORTING", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(io(&claude.path))?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        stdin
            .write_all(self.prompt.as_bytes())
            .map_err(io(&claude.path))?;
        drop(stdin);
        let mut stderr = child.stderr.take().expect("stderr is piped");
        let errors = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        let steps_path = self.log_dir.join("steps.jsonl");
        let mut steps_log = fs::File::create(&steps_path).map_err(io(&steps_path))?;
        let mut answer = None;
        let stdout = child.stdout.take().expect("stdout is piped");
        for line in BufReader::new(stdout).lines() {
            let line = line.map_err(io(&claude.path))?;
            writeln!(steps_log, "{line}").map_err(io(&steps_path))?;
            let Ok(event) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            for step in steps(&event) {
                match &step {
                    Step::Finished { text, .. } => answer = Some(text.clone()),
                    // Without the server Claude could only guess, so the run stops here.
                    Step::Started { tools, .. }
                        if !tools.iter().any(|t| t.starts_with("mcp__shot__")) =>
                    {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(BridgeError::Claude(format!(
                            "the shot MCP server did not start, check that `{} mcp` runs",
                            self.shot.display()
                        )));
                    }
                    _ => {}
                }
                on_step(step);
            }
        }
        let status = child.wait().map_err(io(&claude.path))?;
        let errors = errors.join().unwrap_or_default();
        match answer {
            Some(text) if status.success() => Ok(text),
            _ => Err(BridgeError::Claude(
                errors
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .map_or_else(|| format!("exit status {status}"), str::to_owned),
            )),
        }
    }
}

/// The steps in one line of Claude Code's `stream-json` output.
pub fn steps(event: &Value) -> Vec<Step> {
    let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
    match event["type"].as_str() {
        Some("system") if event["subtype"] == "init" => vec![Step::Started {
            model: text(&event["model"]),
            tools: event["tools"]
                .as_array()
                .map(|t| t.iter().map(text).collect())
                .unwrap_or_default(),
        }],
        Some("assistant") => content(event)
            .filter_map(|block| match block["type"].as_str()? {
                "text" => Some(Step::Text(text(&block["text"]))),
                "tool_use" => Some(Step::ToolUse {
                    name: text(&block["name"]),
                    input: block["input"].to_string(),
                }),
                _ => None,
            })
            .collect(),
        Some("user") => content(event)
            .filter(|block| block["type"] == "tool_result")
            .map(|block| Step::ToolResult {
                error: block["is_error"].as_bool().unwrap_or(false),
                text: match &block["content"] {
                    Value::String(s) => s.clone(),
                    Value::Array(parts) => parts
                        .iter()
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                },
            })
            .collect(),
        Some("result") => vec![Step::Finished {
            text: text(&event["result"]),
            cost_usd: event["total_cost_usd"].as_f64(),
        }],
        _ => Vec::new(),
    }
}

fn content(event: &Value) -> impl Iterator<Item = &Value> {
    event["message"]["content"].as_array().into_iter().flatten()
}

/// A new log folder for a session: `<repo>/.shot/agent/<seconds>-<pid>-<n>`, ignored by git.
pub fn new_log_dir(repo: &Path) -> PathBuf {
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    repo.join(".shot/agent")
        .join(format!("{seconds}-{}-{n}", std::process::id()))
}
