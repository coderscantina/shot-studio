# Milestone 16 evidence: script editor and clip ingest

Source: `m16/integration` at the merge of `m16/editor` (4c3acf3) and `m16/ingest` (f73465f). Machine: Apple M1 Pro, macOS 27.0.1. Built by two streams in their own worktrees, merged without conflicts.

## Checks on the merged branch

- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test -p shot-editor -p shot-app -p shot-media -p shot-capture -p shot-cli -p shot-core -p shot-prompter`: all pass. `shot-editor` 66 tests, including the contract's example `script.md` round-tripping byte for byte through load, an edit and its undo, and a real `NSSpellChecker` test.
- `SHOT_FIXTURES=$HOME/.cache/shot-studio-fixtures cargo test -p shot-capture -- --ignored ingest`: 2 pass. The `phone-clip` take is upright, 30 fps constant, a keyframe every 15 frames, `mic.mp4` 48 kHz mono; a cancelled ingest leaves no take folder.
- `bash -n` and `shellcheck` on `scripts/fixtures.sh`: clean (stream B).
- The shot-app watchdog test failed once in each stream under load and passed on rerun, the known flake.

## Script editor

`crates/shot-editor`, 9,184 lines with tests (5,906 of code), against about 9,400 lines of the Druck originals for the same parts. The text core, line display and spell checker are copies of Druck's without directives, citations, footnotes and placeholders; the view, element and layout are new.

Checked in the app on a copy of `demo-channel` (the target project is not a channel repo, so it has no Script screen), driven through `SHOT_CONTROL=1`: typing a chapter and a beat updated the outline and spoken length, `script.md` had the text after the pause, undo restored the earlier text, a `###` heading marked its line, `befor` was underlined and fixed from its suggestion, accepting a proposal after typing kept both, and a bare-title video got an empty `## ` chapter. Screenshots at 1600x1000 in `docs/evidence/m16/script-editor-*.png`.

Choices beyond the brief:
- Continue to record commits `script.md`, because Record's `take_script` writes the branch's script over the folder's and would lose uncommitted edits.
- The Full script / Beats switch is gone; it only switched the old read-only document.
- Learn spelling uses the macOS dictionary instead of Druck's project word list file.

Not verified: IME composition, reloading an outside change on window activation, saving on quit, scrollbar drag and mouse-drag selection. The control socket cannot drive them.

## Clip ingest

`shot_media::clip::ClipReader`, `shot_capture::ingest`, `shot ingest`, and an `Import` entity in `crates/shot-app/src/takes/import.rs` wired to Import clip and file drop on the Record screen and file drop on the Takes view. AVFoundation's fixed frame duration only caps the rate, so a variable-rate clip came out 0.43 s short over 20 s; the reader fills the constant-rate grid itself with the frame on screen at each slot. The take append moved to `shot_prompter::appended` rather than `shot-core`, which cannot depend on `shot-capture` and `shot-prompter`; the app's `folder.rs` and `live.rs` call it.

End to end on a copy of the target project, release build: `shot ingest` on the phone clip added `t02` as its own chapter in `cut.kdl` and `cut.lock` (3 silence cuts), its transcript matches the spoken text word for word, and `shot export` rendered the whole video (949 s) in 661 s with the clip and its captions at the end.

| Clip | Transcode | Analysis | Total |
| --- | --- | --- | --- |
| `phone-clip` (20 s, 1080p portrait H.264, HLG tags) | 2.3 s | 5.5 s | 8.4 s |
| 60 s 4K60 HDR HEVC | 39 s | 5 s | 45 s |

The 4K ingest peaked at 2 GB of memory, part of it likely the speech model.

Not verified:
- HDR tone mapping: the output is tagged BT.709 and looks plausible next to the source, but nothing proves AVFoundation tone-maps HLG rather than converting the tags.
- Import clip and drop in the app: the control socket cannot switch screens, and a native file picker or drag and drop cannot be scripted. Only the CLI-made take was seen in the Edit screen.
