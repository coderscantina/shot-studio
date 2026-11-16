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
fn reads_a_video_file_from_its_tip() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    board.move_card("alpha", Status::Scripting).unwrap();

    let video = board.read_file("alpha", "video.kdl").unwrap().unwrap();
    assert!(video.contains("status scripting"));
    assert_eq!(board.read_file("alpha", "cut.kdl").unwrap(), None);
    assert!(matches!(
        board.read_file("alpha", "../beta/video.kdl"),
        Err(Error::InvalidPath(_))
    ));
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
fn tags_a_shipped_video_with_its_release_date() {
    let channel = Channel::new(&[("alpha", VIDEO), ("beta", VIDEO)]);
    let board = channel.board();
    let day = shot_core::Date::new(2026, 11, 7).unwrap();

    board.move_card("alpha", Status::Editing).unwrap();
    assert!(matches!(
        board.tag_release("alpha", day),
        Err(Error::NotShipped(_))
    ));

    let shipped = board.move_card("alpha", Status::Shipped).unwrap().unwrap();
    let first = board.tag_release("alpha", day).unwrap();
    assert_eq!(first.name, "v26.11.7");
    assert_eq!(first.commit, shipped);

    board.move_card("beta", Status::Shipped).unwrap();
    let second = board.tag_release("beta", day).unwrap();
    assert_eq!(second.name, "v26.11.7.2");

    let releases = channel.board().releases().unwrap();
    assert_eq!(releases, vec![second, first]);
    assert_eq!(releases[1].slug.as_deref(), Some("alpha"));
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
fn reads_a_proposal_file_where_it_branched_and_at_its_head() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    board
        .create_proposal(
            "alpha",
            "cut",
            "✂️ first cut",
            &files(&[("script.md", "# Shorter\n")]),
        )
        .unwrap();
    board.move_card("alpha", Status::Recorded).unwrap();
    assert_eq!(
        board.proposal_file("alpha", "cut", "script.md").unwrap(),
        (Some("# Title\n".to_owned()), Some("# Shorter\n".to_owned()))
    );
    assert_eq!(
        board.proposal_file("alpha", "cut", "cut.kdl").unwrap(),
        (None, None)
    );
}

#[test]
fn lists_the_files_a_proposal_writes_by_repo_path_at_its_head() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    board
        .create_proposal(
            "alpha",
            "cut",
            "✂️ first cut",
            &files(&[
                ("script.md", "# Shorter\n"),
                ("cut.kdl", "captions clean\n"),
            ]),
        )
        .unwrap();
    board.move_card("alpha", Status::Recorded).unwrap();
    assert_eq!(
        board.proposal_files("alpha", "cut").unwrap(),
        files(&[
            ("videos/alpha/cut.kdl", "captions clean\n"),
            ("videos/alpha/script.md", "# Shorter\n"),
        ])
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
fn accepting_or_rejecting_removes_the_proposals_render_cache() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    board.move_card("alpha", Status::Scripting).unwrap();
    let cache = |name: &str| {
        let dir = channel
            .dir
            .path()
            .join("videos/alpha/.shot/cache/proposals")
            .join(name);
        std::fs::create_dir_all(dir.join("chunks")).unwrap();
        dir
    };
    for name in ["tighter", "longer"] {
        board
            .create_proposal(
                "alpha",
                name,
                "💡 edit",
                &files(&[("script.md", "# Other\n")]),
            )
            .unwrap();
    }
    let (tighter, longer) = (cache("tighter"), cache("longer"));
    board.accept_proposal("alpha", "tighter").unwrap();
    assert!(!tighter.exists() && longer.exists());
    board.reject_proposal("alpha", "longer").unwrap();
    assert!(!longer.exists());
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

#[test]
fn a_channel_proposal_branches_off_and_merges_into_the_default_branch() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    board.move_card("alpha", Status::Scripting).unwrap();
    let (main, video) = (channel.sha("main"), channel.sha("video/alpha"));
    let rule = "on click {\n    zoom 130\n}\n";

    let proposal = board
        .create_repo_proposal(
            "alpha",
            "gentler-click",
            "🔍 zoom less on clicks",
            &files(&[("formats/opinion.kdl", rule)]),
        )
        .unwrap();
    let repo = channel.repo();
    let head = repo.find_commit(proposal.head.parse().unwrap()).unwrap();
    assert_eq!(head.parent_id(0).unwrap().to_string(), main);
    let diff = board.proposal_diff("alpha", "gentler-click").unwrap();
    assert_eq!(diff.len(), 1);
    assert_eq!(diff[0].path, "formats/opinion.kdl");

    let accepted = board.accept_proposal("alpha", "gentler-click").unwrap();
    assert_eq!(accepted.head, proposal.head);
    assert_eq!(channel.sha("main"), proposal.head);
    assert_eq!(channel.sha("video/alpha"), video);
    // Every video reads the channel's files from the default branch.
    let read = board
        .read_repo_file("alpha", "formats/opinion.kdl")
        .unwrap();
    assert_eq!(read.as_deref(), Some(rule.as_bytes()));

    let mixed = board.create_repo_proposal(
        "alpha",
        "both",
        "x",
        &files(&[("formats/opinion.kdl", "x"), ("videos/alpha/cut.kdl", "y")]),
    );
    assert!(matches!(mixed, Err(Error::MixedProposal)));
}

#[test]
fn starts_a_channel_with_the_format_presets_and_a_clean_tree() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cantina");
    let board = Board::init(&path).unwrap();
    assert!(board.cards().unwrap().is_empty());

    let repo = Repository::open(&path).unwrap();
    let head = repo.head().unwrap();
    assert_eq!(head.shorthand().unwrap(), "main");
    let commit = head.peel_to_commit().unwrap();
    assert_eq!(commit.summary().unwrap(), Some("🎉 start the channel"));
    let tree = commit.tree().unwrap();
    assert!(tree.get_path(Path::new("formats/opinion.kdl")).is_ok());
    assert!(
        repo.statuses(None).unwrap().is_empty(),
        "nothing left to commit"
    );

    assert!(matches!(Board::init(&path), Err(Error::NotEmpty(_))));
}

#[test]
fn adds_a_video_as_a_branch_and_refuses_a_taken_slug() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let main = channel.sha("main");
    let board = channel.board();
    let video = files(&[("video.kdl", VIDEO), ("script.md", "# Beta\n")]);
    board.add_video("beta", &video).unwrap();

    assert_eq!(channel.sha("main"), main, "the default branch stays put");
    assert_eq!(
        channel.file_at("video/beta", "videos/beta/script.md"),
        "# Beta\n"
    );
    let slugs: Vec<String> = board.cards().unwrap().into_iter().map(|c| c.slug).collect();
    assert_eq!(slugs, ["alpha", "beta"]);

    for taken in ["alpha", "beta"] {
        assert!(matches!(
            board.add_video(taken, &video),
            Err(Error::VideoExists(_))
        ));
    }
    assert!(board.add_video("gamma", &files(&[("../x", "")])).is_err());
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Paths `git status` would list in the repo at `path`, untracked files included.
fn status(path: &Path) -> Vec<String> {
    let repo = Repository::discover(path).unwrap();
    let mut options = git2::StatusOptions::new();
    options.include_untracked(true).recurse_untracked_dirs(true);
    repo.statuses(Some(&mut options))
        .unwrap()
        .iter()
        .map(|e| e.path().unwrap().to_owned())
        .collect()
}

#[test]
fn opening_a_video_creates_its_worktree_once_and_reuses_it() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();

    let folder = board.open_video("alpha").unwrap();
    let worktree = channel.dir.path().join(".shot/worktrees/alpha");
    assert_eq!(
        folder.canonicalize().unwrap(),
        worktree.join("videos/alpha").canonicalize().unwrap()
    );
    assert!(folder.join("video.kdl").is_file());
    let repo = Repository::open(&worktree).unwrap();
    assert_eq!(
        repo.head().unwrap().name().unwrap(),
        "refs/heads/video/alpha"
    );

    std::fs::write(folder.join("cut.kdl"), "chapter \"Intro\"\n").unwrap();
    assert_eq!(board.open_video("alpha").unwrap(), folder);
    assert!(folder.join("cut.kdl").is_file());
    assert!(status(channel.dir.path()).is_empty());
}

#[test]
fn media_stays_in_the_main_checkout_and_is_linked_from_the_worktree() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let folder = channel.board().open_video("alpha").unwrap();

    write(&folder.join("media/t01/screen.mov"), "frames");
    write(&folder.join(".shot/cache/index.db"), "rows");
    let main = channel.dir.path().join("videos/alpha");
    assert_eq!(
        std::fs::read_to_string(main.join("media/t01/screen.mov")).unwrap(),
        "frames"
    );
    assert!(main.join(".shot/cache/index.db").is_file());
    assert!(status(&folder).is_empty());
}

#[test]
fn a_card_shows_uncommitted_edits_from_its_worktree() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    let folder = board.open_video("alpha").unwrap();

    std::fs::write(
        folder.join("video.kdl"),
        VIDEO.replace("status ideas", "status editing"),
    )
    .unwrap();
    let cards = channel.board().cards().unwrap();
    assert_eq!(cards[0].video.as_ref().unwrap().status, Status::Editing);
    assert!(
        channel
            .file_at("video/alpha", "videos/alpha/video.kdl")
            .contains("ideas")
    );
}

#[test]
fn commits_only_the_named_files_of_a_worktree() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    let folder = board.open_video("alpha").unwrap();
    std::fs::write(folder.join("cut.kdl"), "chapter \"Intro\"\n").unwrap();
    std::fs::write(folder.join("script.md"), "# Changed\n").unwrap();

    let sha = board
        .commit_video_files("alpha", &["cut.kdl"], "♻️ migrate alpha to style rules")
        .unwrap()
        .unwrap();
    assert_eq!(channel.sha("video/alpha"), sha);
    assert_eq!(
        channel.file_at("video/alpha", "videos/alpha/cut.kdl"),
        "chapter \"Intro\"\n"
    );
    assert_eq!(
        channel.file_at("video/alpha", "videos/alpha/script.md"),
        "# Title\n"
    );
    assert_eq!(status(&folder), vec!["videos/alpha/script.md"]);
    assert_eq!(
        board
            .commit_video_files("alpha", &["cut.kdl"], "♻️ again")
            .unwrap(),
        None
    );
}

#[test]
fn accepting_commits_a_persons_edits_to_the_files_it_touches_then_merges() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    let folder = board.open_video("alpha").unwrap();
    write(
        &folder.join("video.kdl"),
        "title \"Mine\"\nstatus ideas\nformat opinion\n",
    );
    write(&folder.join("script.md"), "# Mine\n");
    board
        .create_proposal(
            "alpha",
            "format",
            "✏️ edit",
            &files(&[(
                "video.kdl",
                "title \"Friday deploys\"\nstatus ideas\nformat tutorial\n",
            )]),
        )
        .unwrap();

    board.accept_proposal("alpha", "format").unwrap();

    let repo = channel.repo();
    let merge = repo.revparse_single("video/alpha").unwrap();
    let merge = merge.peel_to_commit().unwrap();
    assert_eq!(
        merge.summary().unwrap(),
        Some("🔀 accept format into alpha")
    );
    let edits = merge.parent(0).unwrap();
    assert_eq!(edits.summary().unwrap(), Some("📝 edits before format"));
    assert_eq!(
        std::fs::read_to_string(folder.join("video.kdl")).unwrap(),
        "title \"Mine\"\nstatus ideas\nformat tutorial\n"
    );
    assert_eq!(status(&folder), vec!["videos/alpha/script.md"]);
}

#[test]
fn accepting_over_a_conflicting_edit_is_refused_with_the_files_and_keeps_the_edits_commit() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    let folder = board.open_video("alpha").unwrap();
    write(&folder.join("script.md"), "# Mine\n");
    board
        .create_proposal(
            "alpha",
            "script",
            "✏️ edit",
            &files(&[("script.md", "# Agent\n")]),
        )
        .unwrap();

    let Err(Error::Conflict { paths }) = board.accept_proposal("alpha", "script") else {
        panic!("expected a conflict");
    };
    assert_eq!(paths, vec!["videos/alpha/script.md"]);
    assert!(channel.has_branch("proposal/alpha/script"));
    assert_eq!(
        channel.file_at("video/alpha", "videos/alpha/script.md"),
        "# Mine\n"
    );
    assert!(status(&folder).is_empty());
}

#[test]
fn accepting_over_conflicts_takes_the_proposals_line_and_keeps_the_persons_other_edits() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    let folder = board.open_video("alpha").unwrap();
    let script = folder.join("script.md");
    write(
        &script,
        "# T\n\n## Hook\n\n- old hook\n\n## Body\n\n- keep\n",
    );
    board
        .commit_video_files("alpha", &["script.md"], "📝 write the script")
        .unwrap();
    board
        .create_proposal(
            "alpha",
            "hook",
            "✏️ hook",
            &files(&[(
                "script.md",
                "# T\n\n## Hook\n\n- agent hook\n\n## Body\n\n- keep\n",
            )]),
        )
        .unwrap();
    write(
        &script,
        "# T\n\n## Hook\n\n- my hook\n\n## Body\n\n- keep, mine\n",
    );

    board
        .accept_proposal_over_conflicts("alpha", "hook")
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&script).unwrap(),
        "# T\n\n## Hook\n\n- agent hook\n\n## Body\n\n- keep, mine\n"
    );
    assert!(status(&folder).is_empty());
}

#[test]
fn accepting_the_draft_then_a_hook_gives_the_draft_with_that_hook() {
    let channel = Channel::new(&[("alpha", VIDEO)]);
    let board = channel.board();
    let folder = board.open_video("alpha").unwrap();
    let draft = |hook: &str| format!("# T\n\n## Hook\n\n- {hook}\n\n## Body\n\n- one\n- two\n");
    for (name, hook) in [
        ("draft", "draft hook"),
        ("hook-a", "hook a"),
        ("hook-b", "hook b"),
    ] {
        board
            .create_proposal(
                "alpha",
                name,
                "✏️ draft",
                &files(&[("script.md", &draft(hook))]),
            )
            .unwrap();
    }

    board
        .accept_proposal_over_conflicts("alpha", "draft")
        .unwrap();
    board
        .accept_proposal_over_conflicts("alpha", "hook-a")
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(folder.join("script.md")).unwrap(),
        draft("hook a")
    );
}

#[test]
fn opening_the_board_removes_worktrees_of_merged_or_deleted_branches_without_changes() {
    let channel = Channel::new(&[("alpha", VIDEO), ("beta", VIDEO), ("gamma", VIDEO)]);
    let board = channel.board();
    let alpha = board.open_video("alpha").unwrap();
    board.open_video("beta").unwrap();
    let gamma = board.open_video("gamma").unwrap();
    write(&alpha.join("media/t01/screen.mov"), "frames");
    write(&gamma.join("sources/note.md"), "keep me\n");

    let repo = channel.repo();
    let merged = board.move_card("alpha", Status::Shipped).unwrap().unwrap();
    let merged = repo.find_commit(merged.parse().unwrap()).unwrap();
    let sig = repo.signature().unwrap();
    let main = repo
        .commit(
            None,
            &sig,
            &sig,
            "🔀 merge alpha",
            &merged.tree().unwrap(),
            &[&merged],
        )
        .unwrap();
    repo.reference("refs/heads/main", main, true, "merge")
        .unwrap();
    for slug in ["beta", "gamma"] {
        repo.find_reference(&format!("refs/heads/video/{slug}"))
            .unwrap()
            .delete()
            .unwrap();
    }
    drop(board);

    channel.board();
    let worktrees = channel.dir.path().join(".shot/worktrees");
    assert!(!worktrees.join("alpha").exists());
    assert!(!worktrees.join("beta").exists());
    assert!(gamma.join("sources/note.md").is_file());
    assert!(
        channel
            .dir
            .path()
            .join("videos/alpha/media/t01/screen.mov")
            .is_file()
    );
    assert_eq!(repo.worktrees().unwrap().len(), 1);
}
