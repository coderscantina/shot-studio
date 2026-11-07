# Development workflow

Use this document when taking work, changing a shared interface or closing a milestone. Scope lives in [the technical briefing](technical-briefing.md), acceptance criteria in [the milestone briefs](milestones/README.md).

## Begin an assignment

Read the assigned milestone, its dependencies and the decisions it links. State the files the task owns, the interface it consumes, the behavior it adds and the check that will exercise it. Read the existing code before editing.

One task is one usable behavior, small enough to review in one pass: a recording that survives a killed process, a deleted word that disappears from the export, a proposal branch that merges from the review queue. A milestone holds several tasks, and its gate requires their integrated behavior.

Briefs are written for an agent that knows nothing of earlier conversations.

## Roles and module ownership

Roles are responsibilities, not people. One agent can hold several.

| Module | Owned paths | Owner | Interface and test surface |
| --- | --- | --- | --- |
| Project model | `crates/shot-core`, `docs/contracts/project-files.md` | Core | Parse and write `cut.kdl`, `format.kdl`, `video.kdl`. Evaluate a timeline into render segments with content hashes. Pure functions, no IO |
| Board | `crates/shot-board` | Core | Open a channel repo, list cards, move a card (commit), create and merge proposal branches, tag a release |
| Capture | `crates/shot-capture` | Media | Start and stop a session that writes separate fMP4 tracks and a cursor event log |
| Media | `crates/shot-media` | Media | Decode to IOSurface, encode segments with VideoToolbox, mux and stream-copy concat |
| Render | `crates/shot-render` | Media | One render graph that draws a timeline frame for preview or export, vello overlay templates |
| Audio | `crates/shot-audio` | Media | Voice chain, ducking, loudness measure and normalize |
| Analysis | `crates/shot-analyze` | Analysis | Streaming transcript with word timestamps, VAD, auto-cut rule results with confidence, faces, idle screen, model fetch |
| Prompter | `crates/shot-prompter` | Analysis | Prompter output window, scroll modes, voice-follow position |
| App | `crates/shot-app`, design tokens | App | gpui screens built on `gpui-component`, the preview surface, keyboard map |
| Agent surfaces | `crates/shot-cli`, `crates/shot-mcp`, `crates/shot-ship` | Agents | `shot` subcommands, MCP tools, `shot check` rules, YouTube upload, the `claude` CLI bridge |
| Workspace and release | root `Cargo.toml`, `.github/`, `scripts/`, `sources/` | Core | Checks, CI, fixtures generator, `release.sh`, signing and notarization |

Keep modules deep with small interfaces. The app never parses `cut.kdl` itself, it calls `shot-core`. The CLI and MCP server call the same functions the app does. Add an adapter seam only when a real second implementation exists (FFmpeg fallback next to VideoToolbox is one).

## Shared changes

- **Project files contract**: Core owns it. A task that needs a new node in `cut.kdl` proposes it in the contract first, with an example and a parse test.
- **Workspace `Cargo.toml`**: Core integrates new dependencies and pins. Each deliberate pin gets a one-line comment.
- **Design tokens and shared components**: App owns them. Screen tasks use them instead of local substitutes. Settled visual rules go into [design.md](design.md).
- **MCP tools and CLI subcommands**: Agents owns the tool list. A domain task proposes the tool's input, output and errors alongside the function it wraps.

## Parallel work

Each implementing subagent gets its own git worktree under `.claude/worktrees/` and a task branch. All worktrees share one `CARGO_TARGET_DIR` outside the worktrees. Check free disk before starting several. Task branches merge into one integration branch. At closeout the worktrees and merged branches are removed, and no work exists only in a worktree. Merging the integration branch into `main` needs Mike's go.

## Decisions

Write a record under `docs/decisions/` for a choice that affects several modules: problem, constraints, choice, rejected alternatives, consequences, affected contracts. Only an answer Mike gave is Adopted. A recommendation waiting for him is Proposed and gets a line in the milestone index's open decisions.

Relaxing scope, targets or the privacy rules is Mike's decision. For a failing gate, report the failing workload, the root cause evidence and alternatives that keep the requirement. Mark dependent work gated and carry on with independent tasks.

## Testing

- Correctness runs on small generated fixtures. Capacity and the [targets](technical-briefing.md#targets) run on the generated 17 min 4K60 project. Both come from the fixture generator milestone 01 adds. See [0007](decisions/0007-generated-fixtures-only.md).
- Test through real interfaces: real VideoToolbox, real whisper on a `say` clip, a real git repo in a temp dir. Mock only what can't run in CI, and say so in the report.
- Timing code takes a clock and a media timebase as inputs, so tests control time.
- Golden frames compare render output within a tolerance, never byte for byte.
- Before closing a UI milestone, look at every changed screen at 1600x1000 in dark, check keyboard focus and shortcuts, and store screenshots in `evidence/mNN/`. A screenshot does not verify timing or audio, so those get their own measurements.
- Performance numbers come from release builds on the reference machine and state the fixture, commit and macOS version.

## Completion report

Each task handoff states:

- Behavior delivered and files changed.
- Interface or contract changes.
- Checks run and their outcomes, including failed or unavailable ones.
- Evidence locations and fixture or source revisions.
- Remaining work and the next task that consumes this result.

A milestone closes only when every acceptance criterion has evidence. Set its status to complete and link the report in the index. A task with placeholders, skipped fixtures or unresolved failures stays in progress. Then update [STATUS.md](STATUS.md).
