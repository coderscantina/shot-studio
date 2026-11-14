# Milestone 15: One video model, one transition type, themes as files

## Outcome

Mike settled the plan on 2026-11-14 ([0029](../decisions/0029-several-themes-per-channel.md), [0035](../decisions/0035-every-video-on-style-rules.md), [0036](../decisions/0036-one-transition-type.md), [0037](../decisions/0037-a-worktree-per-video-branch.md)). The goal is less code for the same videos: one kind of video instead of migrated and unmigrated, one transition type instead of chapter transitions and layout switches, themes as files a pointer picks, and the brand's tools usable from `shot` and the MCP server. Studio edits land on the video's branch. A few overlay gaps close on the way.

## Dependencies and ownership

Builds on `main` at milestone 14. Streams work in their own worktree and task branch and merge into `m15/integration`. Stream A and stream D's first part run first, because B, C and E edit code A deletes. Shared files (`crates/shot-core/src/lib.rs`, `docs/contracts/project-files.md`, workspace `Cargo.toml`, `crates/shot-app/src/main.rs`, `studio.rs`) take small changes in the sections a stream owns. Conflicts are resolved at integration.

| Phase | Stream | Items | Owns |
| --- | --- | --- | --- |
| 1 | A one video model | 1, 2 | migrate on open, the channel of one, deleting the old grammar outside `shot_core::migrate`, the old-only commands, `crates/shot-app/src/style.rs` and the screens' reads of the lock, the contract's migration section |
| 1 | D1 video worktrees | 3 | worktrees in `crates/shot-board`, the `.shot/worktrees/` layout, media links |
| 2 | B transitions | 4, 5 | `TransitionKind`, `SwitchKind`, `LayoutSwitch` in `shot-core`, the two-sided painter path in `shot-render` (`join.rs`, `layout.rs`, the painter), the app's transition and switch controls, the contract's transitions and scene switch sections |
| 2 | C themes | 6, 7, 8 | `crates/shot-core/src/brand.rs` and the cascade, `crates/shot-app/src/brand/store.rs` moving to `shot-core`, `shot theme`, MCP theme and preset tools, presets in rules, the channel lock, the contract's `brand.kdl`, themes and format layer sections |
| 2 | D2 video worktrees in the app | 3 | opening a card through its worktree in `shot-app` (`board/`, `folder.rs`), `shot` and the MCP server resolving a card to its worktree, the migration commit |
| 2 | E overlays | 9, 10, 11 | removal in `crates/shot-app/src/edit/overlays.rs` and the sound lane, safe-area anchors in `shot-render/src/overlay/`, the app's copies of render and core code (`score.rs`, `style::layer`), the painter's screen layer and decoration hook use |

## Work

1. **Migrate on open.** The app, `shot` and the MCP server migrate a video written before the lock when they open it, through `shot_core::migrate`, before anything else reads it. A migration that would change the frames refuses and the video does not open, with the reason. A video folder outside a channel repo is a channel of one: the shipped format preset is its channel layer and the default brand its brand, and it gets and rebuilds a lock like any other video.
2. **One kind of video.** The old grammar (`remove by=<rule>`, `review`, `ignore`, rule zooms, `auto-cut`, `auto-zoom`, `pacing`) is read only inside `shot_core::migrate`; everywhere else it is refused with a message that says to open or migrate the video. Code paths for an unmigrated video go, including `Option<Lock>` where every video now has one. The app's screens read the lock instead of a cut folded back into the old shape. Commands that only worked on unmigrated videos go, and AGENTS.md's demo section says what replaces them.
3. **A worktree per video branch.** Opening a card checks out `video/<slug>` in `.shot/worktrees/<slug>` (created once, reused), with the video's git-ignored media linked from the main checkout. Edits stay uncommitted; a migration on open lands as its own commit. The board shows a card's uncommitted edits, refuses a proposal merge that touches files with uncommitted changes, and removes worktrees of merged or deleted branches that have no changes. `brand/` is read from the main checkout.
4. **One transition type.** `Transition { kind, length, sound }` in `shot-core`, written the same way on a chapter rule, a layout range's `in` and `out` and the format's `layout-switch`. Kinds `cut`, `dissolve`, `dip`, `stinger`, `mask-wipe`, `zoom-through` on both edges, `morph` on layout edges only (refused at export on a chapter edge). `sound=brand|<name>` on both. Existing files read the same.
5. **One two-sided painter path.** Every kind on either edge draws through one path that shows exactly the outgoing frame at progress 0 and the incoming one at 1, and prefetches the second side. The two `#[ignore]`d transition tests from milestone 14 pass and lose their `#[ignore]`.
6. **Themes as files.** Every look in `brand/themes/<id>.kdl`, `brand.kdl` with `theme <id>` and no look fields. Migration moves an inline look to `themes/default.kdl`. Any format layer may name a theme; the nearest wins, then that layer's `brand` block lays over it. The theme window's switch writes one line.
7. **Brand tools on the core.** Theme, overlay preset and channel layout operations live in `shot-core` and are used by the app, `shot theme list|use|save|rename|delete`, preset placement through `shot overlays`, and matching MCP tools with the four hints. Writes to `brand/` take a channel-level lock through `shot-fs`.
8. **Presets in rules.** A style rule's overlay effect may name a preset (`overlay preset=<id>`); the build resolves it into the instance in the lock. Placed overlays stay plain written nodes.
9. **Remove lock overlays and placed sounds.** Removing a rule-made overlay in the app writes `overlay <template> off` on its element's override; removing a placed sound deletes its `sfx` node. Both undo. How provenance shows stays for the design round.
10. **Overlays in Shorts.** Templates place their parts against the output's safe area instead of fixed 16:9 positions, so a 9:16 derived cut shows every card whole. Overlays tied to the screen recording (`highlight`, `callout`, `blur`) keep following it.
11. **One copy of each.** `shot-render` makes export's score public and `shot-core` its settings diff, and the app's copies go. The painter returns the screen layer it drew, so the editor no longer lays out a paused frame twice, and `--bench-edit --glass` uses `Painter::set_decoration`.

## Acceptance criteria

- Opening the generated target project copy in its old form migrates it on open with identical golden frames before and after; a video whose migration would differ refuses to open with the reason.
- `grep -rn "is_migrated" crates` is empty and no crate outside `shot_core::migrate` parses the old nodes; a `cut.kdl` with `remove by=silence` fed to `shot export` is refused with the message.
- A lone video folder outside a channel exports with a lock and the default brand.
- Opening a card in a test channel repo creates its worktree; an edit shows on the board card; the migration is one commit; a merge into a branch with conflicting uncommitted changes is refused naming the files.
- Every existing transition and switch in the fixtures parses and renders unchanged (golden frames); a layout switch with `dip` and a chapter edge with `morph` behave as specified; the progress 0 and 1 tests pass without `#[ignore]`.
- A theme switch is a one-line diff of `brand.kdl`; `shot theme use light` and the MCP tool do the same as the app; a tutorial format naming `theme light` renders in the light look.
- A rule naming a preset writes the preset's overlay into the lock.
- A removed lock overlay stays removed after a rebuild and returns on undo; a link card in a 9:16 Short is whole (golden frame).
- Line count of the touched crates before and after is in the evidence.

## Evidence

`docs/evidence/m15-one-video-model.md`, screenshots in `docs/evidence/m15/`.
