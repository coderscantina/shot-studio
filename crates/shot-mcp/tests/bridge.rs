//! The bridge against a fake `claude` that records how it was started and replays a stream.

use std::os::unix::fs::PermissionsExt;

use shot_mcp::bridge::{Claude, Session, Step};
use tempfile::TempDir;

const FAKE: &str = r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
cat > stdin.txt
echo '{"type":"system","subtype":"init","model":"claude-test","tools":["mcp__shot__list_videos"]}'
echo 'not json'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Looking."},{"type":"tool_use","name":"mcp__shot__list_videos","input":{}}]}}'
echo '{"type":"user","message":{"content":[{"type":"tool_result","content":[{"type":"text","text":"ideas (1)"}]}]}}'
echo '{"type":"result","subtype":"success","result":"One idea.","total_cost_usd":0.01}'
"#;

fn fake(dir: &TempDir, script: &str) -> Claude {
    let path = dir.path().join("claude");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    Claude::at(path)
}

fn session(dir: &TempDir) -> Session {
    Session {
        repo: dir.path().join("channel"),
        shot: "/usr/local/bin/shot".into(),
        prompt: "Which videos are ideas?".to_owned(),
        frame_task: false,
        log_dir: dir.path().join("log"),
    }
}

#[test]
fn streams_steps_from_a_text_only_session_and_logs_what_was_sent() {
    let dir = TempDir::new().unwrap();
    let claude = fake(&dir, FAKE);
    let session = session(&dir);
    let mut steps = Vec::new();
    let answer = session.run(&claude, |step| steps.push(step)).unwrap();

    assert_eq!(answer, "One idea.");
    assert_eq!(
        steps,
        [
            Step::Started {
                model: "claude-test".to_owned(),
                tools: vec!["mcp__shot__list_videos".to_owned()],
            },
            Step::Text("Looking.".to_owned()),
            Step::ToolUse {
                name: "mcp__shot__list_videos".to_owned(),
                input: "{}".to_owned(),
            },
            Step::ToolResult {
                error: false,
                text: "ideas (1)".to_owned(),
            },
            Step::Finished {
                text: "One idea.".to_owned(),
                cost_usd: Some(0.01),
            },
        ]
    );
    let log = session.log_dir;
    assert_eq!(
        std::fs::read_to_string(log.join("stdin.txt")).unwrap(),
        "Which videos are ideas?"
    );
    let args = std::fs::read_to_string(log.join("args.txt")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    let after = |flag: &str| args[args.iter().position(|a| *a == flag).unwrap() + 1];
    assert_eq!(after("--tools"), "", "built-in tools are off");
    assert_eq!(after("--allowedTools"), "mcp__shot");
    assert!(args.contains(&"--strict-mcp-config"));
    assert!(after("--mcp-config").contains("\"--log\""));
    assert!(!after("--mcp-config").contains("--frame-task"));
    assert!(
        std::fs::read_to_string(log.join("sent.txt"))
            .unwrap()
            .ends_with("prompt over stdin:\nWhich videos are ideas?\n")
    );
    assert_eq!(
        std::fs::read_to_string(log.join("steps.jsonl"))
            .unwrap()
            .lines()
            .count(),
        5
    );
}

#[test]
fn a_run_without_the_shot_server_stops_at_the_start() {
    let dir = TempDir::new().unwrap();
    let claude = fake(
        &dir,
        "#!/bin/sh\ncat > /dev/null\necho '{\"type\":\"system\",\"subtype\":\"init\",\"model\":\"m\",\"tools\":[]}'\nsleep 5\n",
    );
    let error = session(&dir).run(&claude, |_| {}).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("the shot MCP server did not start")
    );
}

#[test]
fn a_failed_run_reports_claude_codes_last_error_line() {
    let dir = TempDir::new().unwrap();
    let claude = fake(
        &dir,
        "#!/bin/sh\ncat > /dev/null\necho 'Not logged in' >&2\nexit 1\n",
    );
    let error = session(&dir).run(&claude, |_| {}).unwrap_err();
    assert_eq!(error.to_string(), "Claude Code stopped: Not logged in");
}
