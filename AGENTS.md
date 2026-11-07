# Shot Studio agent guide

Shot Studio is a native macOS app (Rust, gpui) for recording and editing developer videos, with a `shot` CLI and MCP server on one headless core. It is for developers who publish on a schedule, first of all the Coder's Cantina channel, and replaces a ScreenFlow plus DaVinci Resolve workflow.

## Start here

Read [docs/README.md](docs/README.md) and follow its reading order. The technical briefing owns scope, the milestone index owns sequence and status, and the development workflow owns how you take, change and hand back work. Read [docs/STATUS.md](docs/STATUS.md) before starting.

## Rules that apply to every task

- Work from an assigned milestone or brief. Before editing, state the files you own, the interface you consume, the behavior you add and the check that proves it.
- Stay inside your file ownership. Changes to shared files (workspace `Cargo.toml`, the project files contract, design tokens, the MCP tool list) go to their owner as a request.
- Regenerate generated files with the command that makes them. Never edit them by hand.
- Scope changes, relaxed targets and anything touching privacy or secrets go to Mike. Routine choices inside the contract are yours.
- Test media is generated. Never commit real recordings, tokens or model files.
- A subagent in its own worktree commits its checked task to its task branch. Everything else waits until Mike asks for a commit.
- Hand back with the [completion report](docs/development-workflow.md#completion-report).

## Checks

Run the checks for the paths you changed:

- Rust: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
- Fixtures: `bash -n scripts/fixtures.sh` and `shellcheck scripts/fixtures.sh` after editing the generator. Tests that need generated media are `#[ignore]`d: run them with `SHOT_FIXTURES=$HOME/.cache/shot-studio-fixtures cargo test -p <crate> -- --ignored`

Documentation names a command only after the code adds it.

## Demo

Data comes from `SHOT_FIXTURES=$HOME/.cache/shot-studio-fixtures scripts/fixtures.sh all` (about 8 min the first time, then it skips unchanged groups): `target-project/` is the 17 min 4K60 video folder, `demo-channel/` the board repo. See [fixtures/README.md](fixtures/README.md). Run the app on a copy of the target project, never on the shared fixture: `cp -R $SHOT_FIXTURES/target-project /tmp/shot-demo && cargo run --release -p shot-app -- /tmp/shot-demo`. Transcribe the take once with `cargo run --release -p shot-cli -- transcribe --video /tmp/shot-demo --take t01`. `shot autocut --video <dir>` runs the auto-cut rules, `shot autozoom` and `shot code-diff` write product effects, `shot overlays find`, `shot derived add|list|remove` and `shot export --cut <name>` handle overlays and Shorts, `shot check`, `shot thumbnails` and `shot describe` prepare shipping, `shot agent` runs the Claude bridge, `shot export --video <dir>` renders from the terminal, `shot-app --bench $SHOT_FIXTURES/screen-30s.mp4` measures the preview. Recording needs Screen Recording, Camera and Microphone access for the terminal. The `demo` skill reads this section.

## Gotchas

One line for each trap that cost an agent time and that no config reveals.

- In a worktree, set `CARGO_TARGET_DIR=$HOME/.cache/shot-studio-target` and `SHOT_FIXTURES=$HOME/.cache/shot-studio-fixtures`. `git worktree remove` deletes ignored files, so media generated inside a worktree is lost.
- gpui is the crates.io `gpui-pre` snapshot that gpui-component uses. Adding gpui from Zed git builds a second, incompatible gpui.
- Worktrees share artifacts in the common target dir, because cargo hashes workspace crates by their relative path. After merging, if a build sees an older API of a sibling crate, `touch` its sources and rebuild.
- gpui needs `inactive_frame_interval: None` (an unfocused window otherwise drops to 30 fps) and the `font-kit` feature (otherwise no text renders).
- ffmpeg's `sendcmd` filter is not deterministic. Drive animated fixtures with expressions of `t`.

## Writing

Plain words, sentence case headings, straight quotes, no em dashes. Gitmoji commits with a lowercase plain-language subject, for example `✨ cut silences longer than the pacing allows`.
