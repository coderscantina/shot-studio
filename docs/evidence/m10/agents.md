# M10 evidence: agents on the semantic model

Work item 8 of [milestone 10](../../milestones/10-semantic-core.md). Branch `task/m10-agents`.

## What exists

- `shot style build --video <videos/slug>`: reads the video's files, builds the lock with `shot_core::build`, writes `cut.lock` (skips the write when the bytes match).
- `shot style explain --video <dir> --at <seconds> [--take t01]`: instances at a time with the layer and the rule or override behind each, plus the overrides in `cut.kdl` there (an `off` leaves no instance).
- `shot style scope --rule "<selector>"`: instances, elements and videos the rule produces across the channel (`--repo`), shipped videos counted as frozen.
- MCP tools, all text: `read_elements`, `read_lock`, `explain_instance`, `rule_scope`, `propose_rule` (layer `video`, `format` or `brand`) and `propose_override`.
- Every proposal builds the affected videos with and without the change and writes the result into the commit message and the reply: "Changes the lock of 1 of 3 videos: 4 instances added, 2 removed." A `format` or `brand` change reaches every unshipped video, a `video` change or an override only its video.

## Checked

- `cargo test -p shot-mcp --test style` runs against `fixtures/examples/channel`: the disk loader writes exactly the committed `cut.lock`; an MCP round trip reads the lock and proposes an override, and the proposal branch changes only `videos/fridays/cut.kdl` by the one override; a channel format rule reaches the editing video and leaves the shipped copy; an override on a missing element is refused.
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` pass.
- `shot style build` on a copy of the example channel prints `16 instances, 1 orphan overrides` and the file is byte-identical to the fixture lock.

## Decisions inside the contract

- One loader (`shot_mcp::style::build_video`) over a `Repo` trait with three readers: the working tree for the CLI, a video's branch tip for MCP, and a patched view for what-if builds. Elements, brand and format layers come from the committed state in MCP, so uncommitted analysis output is not seen by agents.
- Missing `brand/brand.kdl`, channel format or video `format.kdl` is an empty layer. An import is read from `styles/<id>.kdl` of the repo (the checkout of the pinned commit); nothing fetches it yet.
- Assets the brand names are hashed when the file exists, so a missing file fails the build with the engine's `MissingAsset`.
- Channel-level proposals (`formats/`, `brand/`) branch off the video and merge into its branch when accepted. This needed `Board::read_repo_file` and `Board::create_repo_proposal` in shot-board (repo-relative paths); `create_proposal` is unchanged for callers.
- Rule proposals rewrite the target file in canonical form. A hand-formatted file shows formatting noise in the diff.

## Not verified

- No run against the 17 min fixture: it has no elements files until the analysis agent's writer lands.
- No agent session with a real `claude`; the bridge is unchanged except for two tool names in its system prompt.
