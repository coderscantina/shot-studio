# Milestone 15 evidence: one video model, one transition type, themes as files

Source: `m15/integration` at the merge of every stream, on top of `main` at milestone 14 (`77e4433`). Machine: Apple M1 Pro, macOS 27.0.1. Fixtures from `scripts/fixtures.sh all` in `$HOME/.cache/shot-studio-fixtures`.

## Streams

| Stream | Branch head | Items |
| --- | --- | --- |
| A one video model | `2e6320c` | 1, 2 |
| D1 video worktrees | `283377f` | 3 (shot-board) |
| D2 worktrees in the front ends | `7431bc3` | 3 (app, `shot`, MCP) |
| D3 proposal accept and read-only MCP | `51db74c` | 3 (Mike's answers of 2026-11-15) |
| B transitions | `3f8a511` | 4, 5, painter part of 11 |
| C themes | `a253925` | 6, 7, 8 |
| E overlays | `8feaa40` | 9, 10, the rest of 11 |

The integration merge of D3 also makes read-only MCP tools read the channel's files (`brand/`, `formats/`, `styles/`) from the main checkout when they read a video from its branch tip, as 0037 says; D3 had them read the default branch.

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| The old target project migrates on open with identical frames; a differing migration refuses | `shot-mcp` open tests: the migrated target project gives the same chunk keys as before; a video whose migration would differ does not open and nothing is written |
| No `is_migrated`, old nodes parsed only in `shot_core::migrate` | `grep -rn is_migrated crates` is empty; `Option<Lock>` is gone; `shot export` on `remove by=silence` is refused with the message |
| A lone folder exports with a lock and the default brand | test in `shot-mcp`: lock from the shipped preset and the default brand, rebuilt on edit |
| Card worktree, edit on the board, migration commit, merge refused | `shot-board` tests (six), `shot-mcp` migration commit test (exactly `cut.kdl`, `format.kdl`, `cut.lock`); after Mike's answer, accept commits a person's edits first and refuses only a real conflict; a copy of the demo channel: `shot style build --video a-tui-in-one-week` made the worktree and one commit |
| Existing transitions unchanged, dip on a layout edge, morph refused on a chapter edge, progress 0 and 1 exact | 84 frames of every kind before and after compared with `golden::assert_match`, all match; focused tests for both spellings, dip keys, morph refusal; the two milestone 14 tests pass |
| A theme switch is a one-line diff; `shot theme use` and MCP equal the app; a format naming a theme renders in it | `shot-core` themes tests, `shot-cli` theme test, MCP `use_theme` test, cascade test for the nearest layer |
| A rule naming a preset writes the overlay into the lock | `shot-core` style test, unknown preset is an error naming it |
| A removed lock overlay stays removed and returns on undo; a link card in a Short is whole | `folder::tests::a_removed_lock_overlay_and_placed_sound_stay_removed_until_undo`; `derived::tests::a_link_card_in_a_short_is_whole` (GPU) checks the card lies inside the Short's safe area and spans over 80% of its width; every template at 16:9 matched before and after at 1280x720 and 4K |
| Line counts | below |

## Checks on the integrated branch

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace --no-fail-fast`: 504 passed, 0 failed (the flaky watchdog test passed this run, it failed in an earlier run on the same tree).
- GPU and fixture tests, `cargo test --release -p shot-render -p shot-core -p shot-mcp -- --ignored`: every test passes but two. `frame_grid` is the known 60 fps gap and is `#[ignore]`d for it. `preview_frames_with_templates_fit_the_frame_budget` missed its p99 budget while debug tests ran beside it; alone it passes at p99 4.4 to 5.2 ms against 16.7 ms.

## Line counts

`.rs` lines under each crate's `src`, `77e4433` and after:

| Crate | Before | After |
| --- | --- | --- |
| shot-analyze | 6165 | 5285 |
| shot-app | 48184 | 47123 |
| shot-board | 935 | 1245 |
| shot-cli | 2708 | 2547 |
| shot-core | 11269 | 12085 |
| shot-fs | 153 | 207 |
| shot-mcp | 3100 | 3926 |
| shot-render | 23303 | 23110 |
| others | unchanged within 20 lines | |

Source is about 300 lines shorter overall; tests grew by about 600. The deletions paid for the brand tools on the CLI and MCP and the worktrees rather than shrinking the code.

## Not verified

- No `shot migrate` on the full 17 min target project, no speech tests (they need the models), no reel export.
- Transitions in the edit preview bench on a quiet machine. Stream B's run (`--bench-edit`, 16 x 60 s, a dissolve on every chapter start) crossed 7 dissolves without a drop, but ended at 4 dropped frames against a limit of 3 while three other streams were building; none was within 28 s of a transition. The rerun on the integrated branch with nothing else running, `--bench-edit --runs 16` on a copy of the plain target project, passes: 0 dropped, 0 wrong seek frames, seek p95 33.4 ms. That project has no transitions, so it does not cross one.
- The app, driven through the control socket on copies of the demo channel and the target project (screenshots in `docs/evidence/m15/`): opening a Recorded card created `.shot/worktrees/<slug>` with `media` and `.shot` linked from the main checkout and committed the migration as one commit (`cut.kdl`, `cut.lock`, `format.kdl`); the channel format the migration rewrote stayed an uncommitted edit in the main checkout. The theme window opens on the Default theme. The migrated lone target project opens in Cut and Edit with the lock's cuts. Not driven: the switch popover (stream B checked it in the app), removing a lock overlay or sound in the Overlays pane and Sound lane, a theme switch from the window, and an accept with uncommitted edits.
