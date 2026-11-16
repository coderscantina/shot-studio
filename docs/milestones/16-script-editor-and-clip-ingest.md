# Milestone 16: Script editor and clip ingest

## Outcome

Mike asked on 2026-11-16 for two things: a proper script editor in the app, based on Druck's Markdown editor ([0038](../decisions/0038-script-editor-from-druck.md)), and footage from outside, a phone clip first of all, ingested into the video with its words transcribed ([0039](../decisions/0039-ingested-clips-are-takes.md)). After this milestone a script is written in the Script screen without leaving the app, and a phone clip dropped on the Record screen is a take that is cut, captioned and lined up with the beats like a recording.

## Dependencies and ownership

Builds on `main` at milestone 15. Two streams in their own worktrees and task branches merge into `m16/integration`. Shared files (workspace `Cargo.toml`, `docs/contracts/project-files.md`, `crates/shot-cli/src/main.rs`, `AGENTS.md`) take small changes in the sections a stream owns. Conflicts are resolved at integration.

| Stream | Items | Owns |
| --- | --- | --- |
| A script editor | 1 to 4 | new crate `crates/shot-editor`, `crates/shot-app/src/script/`, the `shot-editor` lines in workspace and app `Cargo.toml`, the contract's `script.md` section, the Script screen rule in `docs/design.md` |
| B clip ingest | 5 to 8 | `crates/shot-media/src/clip.rs` (new), `crates/shot-capture/src/ingest.rs` (new), `crates/shot-cli/src/ingest.rs` (new) and its line in `main.rs`, the import entry points in `crates/shot-app/src/record.rs`, `takes/` and `folder.rs`, a `phone-clip` group in `scripts/fixtures.sh` and `fixtures/README.md`, the contract's take folder section, the `shot ingest` line in AGENTS.md |

## Work

1. **The text core.** `shot-editor::source`, ported from Druck's `desktop/src/source/` (`buffer.rs`, `blocks.rs`, `inline.rs`, `motion.rs`, `typing.rs`) without its `druck` crate helpers (comments, marks, citations) and statistics. No gpui. Druck's tests for these files come along, cut to what still applies.
2. **The editor view.** `shot-editor::EditorView`, a gpui entity with a custom element modelled on Druck's `editor/element.rs`, `layout.rs` and `display.rs`: per-block shaped and cached lines, live-styled source with link targets and heading markers hidden unless the caret is in them, caret, selection, IME through `EntityInputHandler`, undo and redo, soft wrap, scrolling with a drawn scrollbar, Enter continuing a bullet, Cmd+K wrapping the selection in a link. Colors and fonts come from a small style struct the host passes, which `shot-app` fills from its theme tokens. It emits `Changed` and never writes files.
3. **Spell checking.** Druck's `spell/` (NSSpellChecker, project word list) and the checking half of `editor/spelling.rs`: misspelled words underlined after a pause, suggestions and Learn in a context menu built on gpui-component.
4. **The Script screen edits.** The document pane is the editor on the worktree's `script.md`. Save on a 750 ms pause and on leaving the screen, through an atomic write. Re-parse on change: the outline, sources, checks and spoken length follow, and the line `Script::parse` refuses is marked with its message. A change on disk with no unsaved edit replaces the text as one undo step. Proposals show as cards above the editor; the selected one marks its lines; Tab and Esc as today; Accept saves first. Edit script and the default-editor round trip go. An empty video starts from `# <title>` and one `## ` chapter.
5. **Reading a clip.** `shot_media::clip::ClipReader` opens any file AVFoundation plays and yields upright, constant-rate, SDR BT.709 8-bit IOSurface frames through an `AVAssetReaderVideoCompositionOutput` (track transform applied, HDR tone-mapped, frame duration fixed at 1/30 or 1/60 s), plus its duration, size and whether it has audio.
6. **Ingest into a take.** `shot_capture::ingest(file, video_dir, progress) -> Result<Ingested, IngestError>` creates the next take folder, encodes `camera.mp4` (hardware HEVC, upright size, keyframe every 0.5 s, fragmented like a recording) and `mic.mp4` (mono, 48 kHz, AAC), writes an empty `events.jsonl`, and removes the folder again on any error. Progress reports media time done out of the duration.
7. **`shot ingest <file> --video <dir>`.** Ingests, appends the take to `cut.kdl` as its own chapter the way a recording is appended, analyses it (transcript, speech, elements, `--claude` as in `shot analyze`) and rebuilds the lock. The append logic the app uses moves to `shot-core` if the CLI needs it.
8. **Import in the app.** Import clip on the Record screen opens a file picker; dropping a video file on the Record screen or the Takes view does the same. A progress row shows while it runs and can be cancelled. When done the take is appended and analysed as after a recording (`analyze_later`), and the Takes view shows it.

## Acceptance criteria

- The text core's ported tests pass, and a test round-trips the contract's example `script.md` byte for byte through load, an edit and its undo.
- In the app on a copy of the target project: typing a chapter and a beat in the Script screen updates the outline and spoken length without a reload, `script.md` on disk has the text after the pause, undo brings back the earlier text, a broken heading marks its line, a misspelled word is underlined and fixed from its suggestion, and accepting a proposal after typing keeps both. Screenshots at 1600x1000 in dark.
- The generated `phone-clip` fixture (portrait by track transform, H.264, 29.97 fps variable, HLG HDR if the generator can tag it, stereo 44.1 kHz AAC, speech from `say`) ingests into a take whose `camera.mp4` is upright (height greater than width), 30 fps constant, keyframes every 0.5 s, and whose `mic.mp4` is 48 kHz mono. Its transcript holds the spoken words. A test asserts this through real AVFoundation and VideoToolbox, `#[ignore]`d for the fixture.
- `shot ingest` on the target project copy adds a chapter with the take to `cut.kdl` and the lock, and `shot export` renders it.
- Dropping the fixture on the Record screen adds the take and the Takes view lists it after analysis.
- Ingest time for the fixture and for a 60 s 4K clip, release build, in the evidence.

## Evidence

`docs/evidence/m16-script-editor-and-clip-ingest.md`, screenshots in `docs/evidence/m16/`.
