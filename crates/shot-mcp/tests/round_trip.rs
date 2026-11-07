use std::path::Path;

use git2::{IndexAddOption, Repository, RepositoryInitOptions};
use rmcp::model::{CallToolRequestParams, ClientConfig};
use rmcp::{ClientHandler, ServiceExt};
use serde_json::json;
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
            "create_proposal",
            "list_proposals",
            "list_videos",
            "move_card",
            "proposal_diff",
            "read_video",
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
