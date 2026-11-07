use std::collections::BTreeMap;
use std::path::Path;

use git2::{BranchType, Repository};
use shot_board::{Board, CardError, Error};
use shot_core::Status;
use tempfile::TempDir;

const VIDEO: &str = "title \"Friday deploys\"\nstatus ideas\nformat opinion\n";

struct Channel {
    dir: TempDir,
}

impl Channel {
    /// A repo on `main` with the given `(slug, video.kdl)` videos and script.md files.
    fn new(videos: &[(&str, &str)]) -> Self {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init_opts(
            dir.path(),
            git2::RepositoryInitOptions::new().initial_head("main"),
        )
        .unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Mike").unwrap();
        config.set_str("user.email", "mike@example.com").unwrap();
        for (slug, kdl) in videos {
            let folder = dir.path().join("videos").join(slug);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("video.kdl"), kdl).unwrap();
            std::fs::write(folder.join("script.md"), "# Title\n").unwrap();
        }
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = repo.signature().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "🎉 init", &tree, &[])
            .unwrap();
        Self { dir }
    }

    fn board(&self) -> Board {
        Board::open(self.dir.path()).unwrap()
    }

    fn repo(&self) -> Repository {
        Repository::open(self.dir.path()).unwrap()
    }

    fn sha(&self, branch: &str) -> String {
        let repo = self.repo();
        let branch = repo.find_branch(branch, BranchType::Local).unwrap();
        branch.get().peel_to_commit().unwrap().id().to_string()
    }

    fn has_branch(&self, branch: &str) -> bool {
        self.repo().find_branch(branch, BranchType::Local).is_ok()
    }

    fn file_at(&self, branch: &str, path: &str) -> String {
        let repo = self.repo();
        let tree = repo
            .find_branch(branch, BranchType::Local)
            .unwrap()
            .get()
            .peel_to_tree()
            .unwrap();
        let blob = tree.get_path(Path::new(path)).unwrap().id();
        String::from_utf8(repo.find_blob(blob).unwrap().content().to_vec()).unwrap()
    }
}

fn files(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[test]
fn lists_cards_and_reports_parse_errors_per_video() {
    let channel = Channel::new(&[
        ("alpha", VIDEO),
        ("broken", "title \"x\"\nstatus nonsense\nformat opinion\n"),
    ]);
    let cards = channel.board().cards().unwrap();
    assert_eq!(cards.len(), 2);
    let alpha = cards[0].video.as_ref().unwrap();
    assert_eq!(alpha.title, "Friday deploys");
    assert_eq!(alpha.status, Status::Ideas);
    let Err(CardError::Parse { error, .. }) = &cards[1].video else {
        panic!("broken card should not parse");
    };
    assert_eq!(error.line, 2);
    assert!(
        cards[1]
            .video
            .as_ref()
            .unwrap_err()
            .to_string()
            .starts_with("videos/broken/video.kdl:2:")
    );
}

#[test]
fn reads_the_branch_tip_and_falls_back_to_the_default_branch() {
    let channel = Channel::new(&[("alpha", VIDEO), ("beta", VIDEO)]);
    let board = channel.board();
    board.move_card("alpha", Status::Scripting).unwrap();

    let cards = board.cards().unwrap();
    assert!(cards[0].has_branch);
    assert_eq!(cards[0].video.as_ref().unwrap().status, Status::Scripting);
    assert!(!cards[1].has_branch);
    assert_eq!(cards[1].video.as_ref().unwrap().status, Status::Ideas);
}

#[test]
fn moving_a_card_is_one_commit_and_the_board_rebuilds_from_git() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let main = channel.sha("main");
    let sha = channel
        .board()
        .move_card("alpha", Status::Editing)
        .unwrap()
        .unwrap();

    let repo = channel.repo();
    let commit = repo.find_commit(sha.parse().unwrap()).unwrap();
    assert_eq!(commit.summary().unwrap(), Some("🚚 move alpha to editing"));
    assert_eq!(commit.parent_count(), 1);
    assert_eq!(commit.parent_id(0).unwrap().to_string(), main);
    assert_eq!(channel.sha("video/alpha"), sha);
    assert_eq!(channel.sha("main"), main);

    let fresh = channel.board().cards().unwrap();
    assert_eq!(fresh[0].video.as_ref().unwrap().status, Status::Editing);

    assert_eq!(
        channel.board().move_card("alpha", Status::Editing).unwrap(),
        None
    );
    assert_eq!(channel.sha("video/alpha"), sha);
}

#[test]
fn commits_with_the_repo_identity() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    channel.board().move_card("alpha", Status::Ready).unwrap();
    let activity = channel.board().activity(5).unwrap();
    assert_eq!(activity[0].author, "Mike");
}

#[test]
fn activity_is_newest_first_across_videos_and_bounded() {
    let channel = Channel::new(&[("alpha", VIDEO), ("beta", VIDEO)]);
    let board = channel.board();
    board.move_card("alpha", Status::Scripting).unwrap();
    board.move_card("alpha", Status::Ready).unwrap();
    board.move_card("beta", Status::Scripting).unwrap();

    let all = board.activity(10).unwrap();
    assert_eq!(all.len(), 3);
    assert!(all.windows(2).all(|w| w[0].time >= w[1].time));
    assert!(all.iter().all(|a| a.subject.starts_with("🚚 move ")));
    assert_eq!(all.iter().filter(|a| a.slug == "alpha").count(), 2);
    assert_eq!(board.activity(2).unwrap().len(), 2);
}

#[test]
fn tags_a_shipped_video_with_the_release_week() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    let week = shot_board::ReleaseWeek {
        year: 2026,
        week: 46,
    };

    board.move_card("alpha", Status::Editing).unwrap();
    assert!(matches!(
        board.tag_release("alpha", week),
        Err(Error::NotShipped(_))
    ));

    let shipped = board.move_card("alpha", Status::Shipped).unwrap().unwrap();
    let tag = board.tag_release("alpha", week).unwrap();
    assert_eq!(tag.name, "v2026.46");
    assert_eq!(tag.commit, shipped);
    assert!(matches!(
        board.tag_release("alpha", week),
        Err(Error::TagExists(_))
    ));

    let releases = channel.board().releases().unwrap();
    assert_eq!(releases, vec![tag]);
    assert_eq!(releases[0].slug.as_deref(), Some("alpha"));
}

#[test]
fn proposal_shows_a_diff_and_accept_fast_forwards() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    board.move_card("alpha", Status::Scripting).unwrap();
    let before = channel.sha("video/alpha");

    let proposal = board
        .create_proposal(
            "alpha",
            "tighter-hook",
            "✏️ tighten the hook",
            &files(&[("script.md", "# Title\n\n## Hook\n")]),
        )
        .unwrap();
    assert_eq!(channel.sha("video/alpha"), before);
    assert_eq!(
        board.proposals(Some("alpha")).unwrap(),
        vec![proposal.clone()]
    );
    assert!(board.proposals(Some("beta")).unwrap().is_empty());

    let diff = board.proposal_diff("alpha", "tighter-hook").unwrap();
    assert_eq!(diff.len(), 1);
    assert_eq!(diff[0].path, "script.md");
    assert!(diff[0].patch.contains("+## Hook"));

    let accepted = board.accept_proposal("alpha", "tighter-hook").unwrap();
    assert!(accepted.fast_forward);
    assert_eq!(accepted.head, proposal.head);
    assert_eq!(channel.sha("video/alpha"), proposal.head);
    assert!(!channel.has_branch("proposal/alpha/tighter-hook"));
    assert_eq!(
        channel.file_at("video/alpha", "videos/alpha/script.md"),
        "# Title\n\n## Hook\n"
    );
}

#[test]
fn accept_merges_when_the_video_branch_moved_on() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    board
        .create_proposal(
            "alpha",
            "cut",
            "✂️ first cut",
            &files(&[("cut.kdl", "captions clean\n")]),
        )
        .unwrap();
    board.move_card("alpha", Status::Recorded).unwrap();

    let accepted = board.accept_proposal("alpha", "cut").unwrap();
    assert!(!accepted.fast_forward);
    let repo = channel.repo();
    let merge = repo.find_commit(accepted.head.parse().unwrap()).unwrap();
    assert_eq!(merge.parent_count(), 2);
    assert_eq!(
        channel.file_at("video/alpha", "videos/alpha/cut.kdl"),
        "captions clean\n"
    );
    assert!(
        channel
            .file_at("video/alpha", "videos/alpha/video.kdl")
            .contains("status recorded")
    );
}

#[test]
fn reject_leaves_no_branch() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    board.move_card("alpha", Status::Scripting).unwrap();
    let before = channel.sha("video/alpha");
    board
        .create_proposal(
            "alpha",
            "bad-idea",
            "💡 rewrite",
            &files(&[("script.md", "# Other\n")]),
        )
        .unwrap();

    board.reject_proposal("alpha", "bad-idea").unwrap();
    assert!(!channel.has_branch("proposal/alpha/bad-idea"));
    assert!(board.proposals(None).unwrap().is_empty());
    assert_eq!(channel.sha("video/alpha"), before);
    assert!(matches!(
        board.reject_proposal("alpha", "bad-idea"),
        Err(Error::UnknownProposal { .. })
    ));
}

#[test]
fn conflicting_accept_returns_the_paths_and_changes_nothing() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    board.move_card("alpha", Status::Scripting).unwrap();
    board
        .create_proposal(
            "alpha",
            "one",
            "✏️ one",
            &files(&[("script.md", "# One\n")]),
        )
        .unwrap();
    board
        .create_proposal(
            "alpha",
            "two",
            "✏️ two",
            &files(&[("script.md", "# Two\n")]),
        )
        .unwrap();
    board.accept_proposal("alpha", "one").unwrap();
    let tip = channel.sha("video/alpha");

    let Err(Error::Conflict { paths }) = board.accept_proposal("alpha", "two") else {
        panic!("expected a conflict");
    };
    assert_eq!(paths, vec!["videos/alpha/script.md"]);
    assert_eq!(channel.sha("video/alpha"), tip);
    assert!(channel.has_branch("proposal/alpha/two"));
}

#[test]
fn rejects_bad_names_and_paths_and_empty_proposals() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    let ok = files(&[("script.md", "# Other\n")]);
    assert!(matches!(
        board.create_proposal("alpha", "Bad Name", "x", &ok),
        Err(Error::InvalidName(_))
    ));
    assert!(matches!(
        board.create_proposal("nope", "a", "x", &ok),
        Err(Error::UnknownVideo(_))
    ));
    assert!(matches!(
        board.create_proposal("alpha", "a", "x", &files(&[("../beta/script.md", "x")])),
        Err(Error::InvalidPath(_))
    ));
    assert!(matches!(
        board.create_proposal("alpha", "a", "x", &files(&[("script.md", "# Title\n")])),
        Err(Error::EmptyProposal)
    ));
}

/// Runs against the generated demo channel: `SHOT_FIXTURES=<dir> scripts/fixtures.sh demo`, then
/// `SHOT_FIXTURES=<dir> cargo test -p shot-board -- --ignored`.
#[test]
#[ignore = "needs a generated demo channel in SHOT_FIXTURES"]
fn generated_demo_channel_has_a_card_in_every_column() {
    let dir = std::env::var_os("SHOT_FIXTURES").expect("SHOT_FIXTURES is not set");
    let board = Board::open(Path::new(&dir).join("demo-channel")).unwrap();

    let mut columns = BTreeMap::<Status, usize>::new();
    for card in board.cards().unwrap() {
        *columns.entry(card.video.unwrap().status).or_default() += 1;
    }
    for &status in Status::ALL {
        assert!(columns.contains_key(&status), "no card in {status}");
    }

    assert_eq!(board.proposals(None).unwrap().len(), 1);
    let releases = board.releases().unwrap();
    assert_eq!(releases.len(), 1);
    assert_eq!(
        releases[0].slug.as_deref(),
        Some("tailwind-without-the-config")
    );
}
