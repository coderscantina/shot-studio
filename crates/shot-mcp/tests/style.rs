//! The semantic tools over the example channel of `fixtures/examples/channel`: an agent reads a
//! lock and proposes an override, a rule at the channel layer reaches only unshipped videos,
//! and the disk loader writes the lock the contract shows.

use std::collections::BTreeSet;
use std::path::Path;

use git2::{IndexAddOption, Repository, RepositoryInitOptions};
use rmcp::model::{CallToolRequestParams, ClientConfig};
use rmcp::{ClientHandler, ServiceExt};
use serde_json::json;
use shot_core::{Cut, ElementKind, Id, Micros};
use shot_mcp::ShotServer;
use shot_mcp::style::{self, Disk};
use tempfile::TempDir;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/examples/channel"
);

#[derive(Clone, Default)]
struct Client;

impl ClientHandler for Client {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default()
    }
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The example channel with the files the lock hashes, the import's style, `fridays` editing
/// and a `launch` video that is already shipped, all committed on `main`.
fn channel() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    copy(Path::new(FIXTURE), root);
    for file in [
        "brand/fonts/Display-Bold.otf",
        "brand/logo.svg",
        "brand/quote-card.json",
        "brand/stinger.json",
    ] {
        std::fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
        std::fs::write(root.join(file), file).unwrap();
    }
    std::fs::create_dir_all(root.join("styles")).unwrap();
    std::fs::write(
        root.join("styles/cantina.kdl"),
        "colors background=\"#0b0d10\"\non chapter-start {\n    transition cut\n}\non chapter-end {\n    sfx tick\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("videos/fridays/video.kdl"),
        "title \"Fridays\"\nstatus editing\nformat opinion\n",
    )
    .unwrap();
    std::fs::write(root.join("videos/fridays/script.md"), "# Fridays\n").unwrap();
    copy(&root.join("videos/fridays"), &root.join("videos/launch"));
    std::fs::write(
        root.join("videos/launch/video.kdl"),
        "title \"Launch\"\nstatus shipped\nformat opinion\n",
    )
    .unwrap();

    let repo =
        Repository::init_opts(root, RepositoryInitOptions::new().initial_head("main")).unwrap();
    let mut config = repo.config().unwrap();
    config.set_str("user.name", "Mike").unwrap();
    config.set_str("user.email", "mike@example.com").unwrap();
    let mut index = repo.index().unwrap();
    index.add_all(["*"], IndexAddOption::DEFAULT, None).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = repo.signature().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .unwrap();
    dir
}

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

/// The files a proposal branch changes against `main`, and the message of its commit.
fn proposal_changes(repo: &Path, branch: &str) -> (BTreeSet<String>, String) {
    let repo = Repository::open(repo).unwrap();
    let head = repo
        .find_branch(branch, git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_commit()
        .unwrap();
    let base = head.parent(0).unwrap();
    let diff = repo
        .diff_tree_to_tree(
            Some(&base.tree().unwrap()),
            Some(&head.tree().unwrap()),
            None,
        )
        .unwrap();
    let files = diff
        .deltas()
        .map(|d| d.new_file().path().unwrap().to_string_lossy().into_owned())
        .collect();
    (files, head.message().unwrap().to_owned())
}

#[tokio::test]
async fn an_agent_reads_the_lock_and_proposes_an_override_for_one_element() {
    let dir = channel();
    let client = connect(dir.path()).await;

    let (error, lock) = call(
        &client,
        "read_lock",
        json!({ "slug": "fridays", "take": "t01", "from_us": 29_000_000, "to_us": 31_000_000 }),
    )
    .await;
    assert!(!error, "{lock}");
    assert!(
        lock.contains(
            "zoom 160 from=30_000_000 to=31_500_000 x=1480 y=910 on=click t=30_000_000 layer=format rule=\"click\""
        ),
        "{lock}"
    );

    let (_, why) = call(
        &client,
        "explain_instance",
        json!({ "slug": "fridays", "take": "t01", "at_us": 30_200_000 }),
    )
    .await;
    assert!(
        why.contains("from rule \"click\" of the format layer, on the click at 30.0 s"),
        "{why}"
    );

    let (_, elements) = call(
        &client,
        "read_elements",
        json!({ "slug": "fridays", "take": "t01", "kinds": ["click"], "from_us": 29_000_000 }),
    )
    .await;
    assert_eq!(
        elements,
        "{\"kind\":\"click\",\"t\":30000000,\"by\":\"events\",\"x\":1480,\"y\":910}\n"
    );

    let (error, reply) = call(
        &client,
        "propose_override",
        json!({
            "slug": "fridays", "name": "calm-click", "subject": "🔇 no zoom on the second click",
            "take": "t01", "kind": "click", "at_us": 30_000_000, "effects": "zoom off",
        }),
    )
    .await;
    assert!(!error, "{reply}");
    assert!(reply.contains("fridays  +0 -1"), "{reply}");

    let (files, message) = proposal_changes(dir.path(), "proposal/fridays/calm-click");
    assert_eq!(files, BTreeSet::from(["videos/fridays/cut.kdl".to_owned()]));
    assert!(
        message.contains("Changes the lock of 1 of 1 videos"),
        "{message}"
    );

    let repo = Repository::open(dir.path()).unwrap();
    let branch = repo
        .find_branch("proposal/fridays/calm-click", git2::BranchType::Local)
        .unwrap();
    let tree = branch.get().peel_to_tree().unwrap();
    let blob = tree.get_path(Path::new("videos/fridays/cut.kdl")).unwrap();
    let cut =
        Cut::parse(std::str::from_utf8(repo.find_blob(blob.id()).unwrap().content()).unwrap())
            .unwrap();
    let before =
        Cut::parse(&std::fs::read_to_string(dir.path().join("videos/fridays/cut.kdl")).unwrap())
            .unwrap();
    let added: Vec<_> = cut
        .overrides
        .iter()
        .filter(|o| !before.overrides.contains(o))
        .collect();
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].kind, ElementKind::Click);
    assert_eq!(added[0].at, Micros(30_000_000));

    let (error, text) = call(
        &client,
        "propose_override",
        json!({ "slug": "fridays", "name": "nothing-there", "subject": "x",
                "take": "t01", "kind": "click", "at_us": 40_000_000, "effects": "zoom off" }),
    )
    .await;
    assert!(error && text.contains("no click element"), "{text}");
}

#[tokio::test]
async fn a_channel_rule_reaches_unshipped_videos_and_leaves_shipped_ones() {
    let dir = channel();
    let client = connect(dir.path()).await;

    let (_, scope) = call(&client, "rule_scope", json!({ "selector": "click" })).await;
    assert!(
        scope.contains("reaches 1 instances on 1 elements in 1 of 1 videos"),
        "{scope}"
    );
    assert!(
        scope.contains("shipped videos keep their lock: 1"),
        "{scope}"
    );

    let (error, reply) = call(
        &client,
        "propose_rule",
        json!({
            "slug": "fridays", "name": "gentler-click", "subject": "🔍 zoom less on clicks",
            "layer": "format", "rule": "on click {\n    zoom 130 hold=1_500_000\n}",
        }),
    )
    .await;
    assert!(!error, "{reply}");
    assert!(
        reply.contains("Changes the lock of 1 of 1 videos: 1 instances added, 1 removed."),
        "{reply}"
    );
    assert!(
        reply.contains("shipped videos keep their lock: 1"),
        "{reply}"
    );

    let (files, message) = proposal_changes(dir.path(), "proposal/fridays/gentler-click");
    assert_eq!(files, BTreeSet::from(["formats/opinion.kdl".to_owned()]));
    assert!(message.starts_with("🔍 zoom less on clicks\n\nChanges the lock of 1 of 1 videos"));
}

#[test]
fn the_disk_loader_builds_the_lock_the_contract_shows() {
    let dir = channel();
    let (disk, slug) = Disk::locate(&dir.path().join("videos/fridays")).unwrap();
    let built = style::build_video(&disk, &slug).unwrap();
    let committed = std::fs::read_to_string(dir.path().join("videos/fridays/cut.lock")).unwrap();
    assert_eq!(built.lock.to_kdl(), committed);
    let orphan = built
        .lock
        .take(&Id::new("t02").unwrap())
        .unwrap()
        .orphans
        .len();
    assert_eq!(orphan, 1);
}

#[tokio::test]
async fn tools_read_a_migrated_video_whose_format_is_a_layer() {
    let dir = channel();
    let client = connect(dir.path()).await;

    let (error, text) = call(&client, "describe", json!({ "slug": "fridays" })).await;
    assert!(!error, "{text}");
}
