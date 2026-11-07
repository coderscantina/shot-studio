use std::path::Path;

use git2::{IndexAddOption, Repository, RepositoryInitOptions};
use rmcp::model::{CallToolRequestParams, ClientConfig};
use rmcp::{ClientHandler, ServiceExt};
use serde_json::json;
use shot_core::{Id, Micros, TimeRange, Transcript, Word};
use shot_mcp::ShotServer;
use tempfile::TempDir;

#[derive(Clone, Default)]
struct Client;

impl ClientHandler for Client {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default()
    }
}

fn channel() -> TempDir {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init_opts(
        dir.path(),
        RepositoryInitOptions::new().initial_head("main"),
    )
    .unwrap();
    let mut config = repo.config().unwrap();
    config.set_str("user.name", "Mike").unwrap();
    config.set_str("user.email", "mike@example.com").unwrap();
    for (slug, kdl) in [
        (
            "alpha",
            "title \"Friday deploys\"\nstatus ideas\nformat opinion\n",
        ),
        (
            "broken",
            "title \"Broken\"\nstatus nowhere\nformat opinion\n",
        ),
    ] {
        let folder = dir.path().join("videos").join(slug);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("video.kdl"), kdl).unwrap();
        std::fs::write(folder.join("script.md"), "# Title\n").unwrap();
    }
    let mut index = repo.index().unwrap();
    index.add_all(["*"], IndexAddOption::DEFAULT, None).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = repo.signature().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .unwrap();
    dir
}

/// Serves `repo` over an in-memory pipe and returns a connected client.
async fn connect(repo: &Path) -> rmcp::service::RunningService<rmcp::RoleClient, Client> {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server = ShotServer::new(repo.to_path_buf());
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    Client.serve(client_io).await.unwrap()
}

async fn call(
    client: &rmcp::service::RunningService<rmcp::RoleClient, Client>,
    tool: &'static str,
    args: serde_json::Value,
) -> (bool, String) {
    let result = client
        .call_tool(
            CallToolRequestParams::new(tool).with_arguments(args.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text())
        .map(|t| t.text.as_str())
        .collect::<String>();
    (result.is_error == Some(true), text)
}

#[tokio::test]
async fn lists_the_tools_and_leaves_review_to_the_human() {
    let dir = channel();
    let client = connect(dir.path()).await;
    let mut names: Vec<_> = client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "activity",
            "caption_fixes",
            "create_proposal",
            "describe",
            "explain_instance",
            "list_proposals",
            "list_videos",
            "move_card",
            "proposal_diff",
            "propose_cut",
            "propose_override",
            "propose_review",
            "propose_rule",
            "propose_script",
            "read_elements",
            "read_lock",
            "read_transcript",
            "read_video",
            "review_queue",
            "rule_scope",
            "script_checks",
            "thumbnail_frames",
        ]
    );
}

#[tokio::test]
async fn reads_moves_and_proposes_through_text_tools() {
    let dir = channel();
    let client = connect(dir.path()).await;

    let (error, text) = call(&client, "list_videos", json!({})).await;
    assert!(!error);
    assert!(text.contains("ideas (1)\n  alpha  opinion  \"Friday deploys\""));
    assert!(text.contains("unreadable (1)\n  broken  videos/broken/video.kdl:2:"));

    let (_, text) = call(&client, "read_video", json!({ "slug": "alpha" })).await;
    assert!(text.contains("== script.md ==\n# Title\n"));
    assert!(text.contains("== cut.kdl ==\n(missing)"));

    let (error, text) = call(
        &client,
        "move_card",
        json!({ "slug": "alpha", "status": "ready" }),
    )
    .await;
    assert!(!error && text.starts_with("moved alpha to ready"));
    let (_, text) = call(&client, "activity", json!({ "limit": 5 })).await;
    assert!(text.contains("alpha") && text.contains("move alpha to ready"));

    let (error, _) = call(
        &client,
        "create_proposal",
        json!({
            "slug": "alpha", "name": "tighter-intro", "subject": "✂️ tighten the intro",
            "files": { "script.md": "# Shorter\n" },
        }),
    )
    .await;
    assert!(!error);
    let (_, text) = call(&client, "list_proposals", json!({})).await;
    assert!(text.contains("alpha/tighter-intro  ✂️ tighten the intro"));
    let (_, text) = call(
        &client,
        "proposal_diff",
        json!({ "slug": "alpha", "name": "tighter-intro" }),
    )
    .await;
    assert!(text.contains("+# Shorter"));
}

#[tokio::test]
async fn failures_are_tool_errors_with_the_reason() {
    let dir = channel();
    let client = connect(dir.path()).await;

    let (error, text) = call(
        &client,
        "move_card",
        json!({ "slug": "alpha", "status": "done" }),
    )
    .await;
    assert!(error && text.contains("expected one of: ideas"));
    let (error, text) = call(
        &client,
        "move_card",
        json!({ "slug": "broken", "status": "ready" }),
    )
    .await;
    assert!(error && text.contains("videos/broken/video.kdl:2:"));
    let (error, text) = call(&client, "read_video", json!({ "slug": "../x" })).await;
    assert!(error && text.contains("not a valid name"));
}

/// Adds a recorded video `beta` with a cut, a review item and a cached transcript.
fn with_recording(dir: &Path) {
    let folder = dir.join("videos/beta");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("video.kdl"),
        "title \"Beta\"\nstatus editing\nformat opinion\n",
    )
    .unwrap();
    std::fs::write(
        folder.join("script.md"),
        "# Beta\n\n## Hook\n\n- We ship on fridays\n",
    )
    .unwrap();
    std::fs::write(
        folder.join("format.kdl"),
        shot_core::FormatKind::Opinion.preset_source(),
    )
    .unwrap();
    std::fs::write(
        folder.join("cut.kdl"),
        "chapter \"Hook\" {\n    take t01 {\n        keep from=0 to=6_000_000\n        review from=1_000_000 to=2_000_000 by=filler\n    }\n}\n",
    )
    .unwrap();
    let mut transcript = Transcript::new(Id::new("t01").unwrap());
    for (i, text) in ["so", "um", "we", "ship", "on", "fridays"]
        .iter()
        .enumerate()
    {
        let from = i as u64 * 1_000_000;
        transcript
            .push(Word {
                text: (*text).to_owned(),
                range: TimeRange::new(Micros(from), Micros(from + 800_000)).unwrap(),
                confidence: if *text == "fridays" { 30 } else { 90 },
            })
            .unwrap();
    }
    shot_analyze::cache::write_cached(&folder, &transcript).unwrap();
    let repo = Repository::open(dir).unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all(["videos"], IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = repo.signature().unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "beta", &tree, &[&head])
        .unwrap();
}

#[tokio::test]
async fn an_agent_reads_a_transcript_and_proposes_a_cut_of_recorded_words() {
    let dir = channel();
    with_recording(dir.path());
    let client = connect(dir.path()).await;

    let (error, text) = call(&client, "read_transcript", json!({ "slug": "beta" })).await;
    assert!(!error, "{text}");
    assert_eq!(
        text,
        "## chapter 1 \"Hook\", take t01\n0:00.0 w0 so um we ship on fridays\n\n"
    );

    let (error, text) = call(
        &client,
        "propose_cut",
        json!({
            "slug": "beta", "name": "no-filler", "subject": "✂️ cut the um",
            "remove": [{ "take": "t01", "from_word": 1, "to_word": 1 }],
        }),
    )
    .await;
    assert!(!error, "{text}");
    assert!(text.contains("removes t01 w1-w1 \"um\" (0.8 s)"));
    let (_, diff) = call(
        &client,
        "proposal_diff",
        json!({ "slug": "beta", "name": "no-filler" }),
    )
    .await;
    assert!(
        diff.contains("+        remove from=1_000_000 to=1_800_000 by=manual"),
        "{diff}"
    );

    let (_, queue) = call(&client, "review_queue", json!({ "slug": "beta" })).await;
    assert_eq!(queue, "#1  t01  filler  1.0 s  \"um\"\n");
    let (error, _) = call(
        &client,
        "propose_review",
        json!({ "slug": "beta", "name": "keep-um", "subject": "keep it",
                "decisions": [{ "item": 1, "decision": "keep" }] }),
    )
    .await;
    assert!(!error);

    let (_, fixes) = call(&client, "caption_fixes", json!({ "slug": "beta" })).await;
    assert!(fixes.contains("\"fridays\" at 0:05"), "{fixes}");
}

#[tokio::test]
async fn scripts_are_checked_and_proposed_only_when_they_parse() {
    let dir = channel();
    with_recording(dir.path());
    let client = connect(dir.path()).await;

    let (_, checks) = call(&client, "script_checks", json!({ "slug": "beta" })).await;
    assert!(checks.contains("a beat in Hook has no source"), "{checks}");
    let (error, text) = call(
        &client,
        "propose_script",
        json!({ "slug": "beta", "name": "hook-a", "subject": "a hook", "script": "## no title\n" }),
    )
    .await;
    assert!(error && text.contains("script.md does not parse"));
    let (error, _) = call(
        &client,
        "propose_script",
        json!({ "slug": "beta", "name": "hook-a", "subject": "a hook",
                "script": "# Beta\n\n## Hook\n\n- Fridays are fine [post](sources/a.md)\n" }),
    )
    .await;
    assert!(!error);
}

#[tokio::test]
async fn frame_tasks_refuse_unless_the_person_started_one() {
    let dir = channel();
    with_recording(dir.path());
    let client = connect(dir.path()).await;
    let (error, text) = call(&client, "thumbnail_frames", json!({ "slug": "beta" })).await;
    assert!(error && text.contains("frame task"), "{text}");

    let stills = dir.path().join("videos/beta/.shot/cache/thumbnails");
    std::fs::create_dir_all(&stills).unwrap();
    std::fs::write(stills.join("t01-2000.png"), b"\x89PNG").unwrap();
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server = ShotServer::new(dir.path().to_path_buf()).with_frame_task(true);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    let started = Client.serve(client_io).await.unwrap();
    let result = started
        .call_tool(
            CallToolRequestParams::new("thumbnail_frames")
                .with_arguments(json!({ "slug": "beta" }).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    assert!(result.content.iter().any(|c| c.as_image().is_some()));
}

/// Needs `SHOT_FIXTURES=<dir> scripts/fixtures.sh demo`; run with `-- --ignored`.
#[tokio::test]
#[ignore = "needs a generated demo channel in SHOT_FIXTURES"]
async fn lists_the_demo_channel() {
    let fixtures = std::env::var_os("SHOT_FIXTURES").expect("SHOT_FIXTURES is not set");
    let copy = TempDir::new().unwrap();
    let source = Path::new(&fixtures).join("demo-channel");
    let status = std::process::Command::new("cp")
        .arg("-R")
        .arg(&source)
        .arg(copy.path())
        .status()
        .unwrap();
    assert!(status.success());

    let client = connect(&copy.path().join("demo-channel")).await;
    let (error, text) = call(&client, "list_videos", json!({})).await;
    assert!(!error);
    for column in [
        "ideas",
        "scripting",
        "ready",
        "recorded",
        "editing",
        "shipped",
    ] {
        assert!(text.contains(&format!("{column} (")), "no {column} column");
    }
}
