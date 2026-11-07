# Technical briefing

Revision 1, agreed with Mike on 2026-11-07. Nothing described here is built or measured yet.

This document owns the product scope and architecture. File formats live in [the project files contract](contracts/project-files.md), sequence in [the milestone index](milestones/README.md), and the reasons behind choices in [decisions](decisions/).

## Product contract

Shot Studio is a macOS desktop app for recording and editing developer videos: reactions and opinions in the style of Theo or ThePrimeagen, plus product tours, feature showcases and tutorials. It treats a video like a software project. The plan lives on a Kanban board backed by git, the script is the spec, auto cut works like a linter, review works like a PR, rendering is an incremental build, and shipping is a tagged release.

- Website: shot.app
- macOS app: `shot.app`, one bundle that also carries the CLI
- CLI: `shot`. MCP server: `shot` (stdio)
- Repo: `github.com/coderscantina/shot-studio`
- Bundle id: `com.coderscantina.shotstudio`
- Mockup: [Shot Studio canvas](https://claude.ai/artifact/P6kQUAKTd96uCBaHTURQsa), 7 linked screens (private until shared)
- Background research: Christoph Rumpel's self-built editor "Creditor" (Laravel, React, OBS, Remotion, FFmpeg). Its 17 min 4K export took 34 min at first and 14 min after tuning. ScreenFlow takes about 5 min for the same video, which is the baseline to beat.

### The bar

Three things set it apart from Descript, Screen Studio, ScreenFlow and DaVinci Resolve:

- **The rough cut is ready when you press stop.** Transcription and analysis run while you record, so there is no processing step.
- **The edit is a plain text file in git.** `cut.kdl` holds ranges, layouts and effects. Alternate cuts (Shorts, X clips) are derived from the main cut, agents propose changes you review as a diff, and every change is reversible.
- **4K export beats realtime, and re-exports only redo what changed.** See [targets](#targets).

Everything runs locally. Claude does judgement work (script drafts, unsure cuts, Shorts picks, caption fixes), never the heavy lifting.

### Audience

Developers who publish on a schedule and record the same few formats again and again. First user: the Coder's Cantina channel. The four formats it ships with:

| Format | Prompter | Default layouts | Typical length |
| --- | --- | --- | --- |
| Reaction | Beats | Face + content, face | 10 to 20 min |
| Opinion | Beats | Face, split | 8 to 15 min |
| Product tour | Full script | Window on background, cursor zoom | 2 to 5 min |
| Tutorial | Full script | Screen, face corner | 8 to 15 min |

Each format is a `format.kdl` you can edit: pacing, layouts, caption style, auto-cut thresholds.

### How developer habits map to video

| Habit | In Shot Studio |
| --- | --- |
| Issue board | Kanban board over a git repo. Each card is a video folder and a branch, moving a card is a commit |
| Spec | `script.md`: chapters as headings, beats as bullets |
| Pair programming | The script agent drafts and edits `script.md` with you |
| Running tests | A recording session, with retakes and markers |
| Linter with auto-fix | Auto cut: confident cuts applied, unsure ones queued |
| PR review | The review queue, plus agent proposals merged like PRs |
| Source code | `cut.kdl`, readable in a diff and by an agent |
| Branches | Derived cuts: main video, Shorts, X clip |
| `cargo build` | Render from cached segments |
| CI | `shot check`: loudness, captions, frames, links, chapters, thumbnail |
| Release | `shot ship`: upload, chapters, description, git tag |

### First release includes

The first release is the whole concept, built in the order of the [milestone index](milestones/README.md).

**Plan on the board**

- Columns: Ideas, Scripting, Ready to record, Recorded, Editing, Shipped.
- The board is one channel repo (for example `coderscantina/videos`). Each video is a folder with `video.kdl`, `script.md`, `cut.kdl`, `format.kdl` and `sources/`. Media is git-ignored.
- Moving a card commits the new status. Claude's work shows up as commits on the video's branch, so `git log` is the activity feed.
- Shipped videos are CalVer git tags `vYY.M.D` (`v26.11.7`), with `.N` for a later release the same day.
- Ideas come from notes, Linear issues, saved links or comments. "Plan the week with Claude" proposes which ideas to script next against the cadence.

**Write the script with an agent**

- You give a brief: the source, your take, a target length, a tone.
- The agent reads the sources, picks passages worth reacting to, learns your voice and pace from past scripts, and pulls git diffs you want to demo. It shows each step it took.
- It offers alternatives where taste matters, for example three hooks to choose from.
- Edits arrive inline as suggestions: Accept (Tab), Reject (Esc), Another take.
- Slash commands: `/tighten`, `/more opinion`, `/b-roll notes`, `/fact check`.
- Script checks: hook length, spoken length estimate, every beat has a source, stretches without a visual.
- A switch sets what goes to the prompter: beats for reactions and opinions, full script for tours and tutorials.

**Record with the Elgato Prompter**

- Screen, camera, mic, system audio and cursor/keys record as separate tracks. The cursor is saved as events, not burned in.
- The prompter shows the current beat large, the previous one dim, the next one smaller. Words turn white as you say them.
- Scroll modes: follow your voice (live transcript), steady, or pedal (Stream Deck). It shows time left and the retake count.
- Hotkeys: A again, N next beat, M marker, Z zoom here, B b-roll here, 1 to 4 layout.
- Inline retakes are caught while you record ("the the first" keeps the clean version).
- Files are fragmented MP4, so a crash never loses a take.

**Auto cut**

- Rules, each with a switch and a count: silences, inline retakes, misspoken words and self-corrections, pauses mid-sentence, filler words (learning which ones are part of your voice), idle screen.
- Pacing: Loose, Natural (0.25 s of breath around cuts), Tight (jump-cut style).
- Confident cuts apply right away. Unsure ones go to a review queue with Keep (K) and Cut (C).
- An Edited/Untrimmed preview toggle shows what the rules removed.

**Auto edit**

- Jump cuts following the pacing setting.
- Punch-in zooms, 112% on emphasis and on every other jump cut.
- B-roll from the article snapshot, git diffs, screen recordings and your own library. It covers idle stretches over 8 s and every `B` marker.
- Auto captions with word-level timing. Claude fixes misheard names (Laravel, werkstatt, gpui). Styles: Word pop, Clean, Off. Burned in for Shorts and X clips, a sidecar file for long-form.
- The transcript is the main editing surface: delete words to cut, select a phrase to add a highlight, zoom or b-roll.
- Reaction effects: layouts (face, face + content, split, content), on-screen highlights matched to the phrase you react to (OCR on the source), link cards from URLs you say (GitHub PR, repo, tweet, HN, npm), bleep and blur from the transcript.
- Product video effects: auto zoom from clicks, typing or cursor, click ripples, cursor size, keystroke overlay, window padding, corners, shadow and background, device frames, and code from git (type out a real commit range, then highlight the line you talk about).
- Audio: denoise, EQ, compressor, -14 LUFS loudness, a music bed that ducks under your voice.

**Ship**

- Derived cuts: the main 16:9 video, Shorts at 9:16 reframed on your face, a 1:1 X clip. Each is a few lines in `cut.kdl` and reuses rendered segments from main.
- `shot check` runs per cut. Passing checks stay quiet, only problems are highlighted.
- Thumbnail candidates are the frames with the strongest expressions.
- Title and description with word-accurate chapters are generated, then edited by you.
- `shot ship` uploads the main video and Shorts to YouTube and tags the release in git. The X clip and any other cut export as files you post yourself. See [0009](decisions/0009-ship-to-youtube.md).

### Out of scope for the first release

Unlimited free-form tracks, a library of transitions, full color grading, keyframes on every property, plugins, a light theme, X upload, Windows and Linux. Linux capture through PipeWire may come later.

## Architecture

One headless core with three front ends: the gpui app, the `shot` CLI and the `shot` MCP server. Clicking, scripting and agents all go through the same code. See [0001](decisions/0001-headless-core-three-front-ends.md).

```
crates/
  shot-core/      project model, cut.kdl, timeline evaluation (no IO)
  shot-capture/   ScreenCaptureKit, AVFoundation camera, CoreAudio mic, cursor events
  shot-media/     VideoToolbox decode/encode, fMP4 mux, FFmpeg fallback
  shot-render/    wgpu render graph + vello overlays (preview and export share it)
  shot-audio/     voice chain, ducking, loudness
  shot-analyze/   transcription, VAD, auto-cut rules, faces, scene and idle detection
  shot-prompter/  Elgato Prompter output, voice-follow scrolling
  shot-board/     git-backed board: video.kdl, status commits, tags, proposals
  shot-ship/      shot check rules, YouTube upload, release tags
  shot-app/       gpui UI, bundled as shot.app
  shot-cli/       binary `shot`
  shot-mcp/       stdio MCP server `shot`
```

The `shot` crate name on crates.io is taken, so every crate is prefixed `shot-` and only the binary is called `shot`.

Rust edition 2024, `rust-version = "1.99"`, Cargo resolver 3, shared dependencies in `[workspace.dependencies]`. gpui, gpui-component, vello and parley are pinned to exact versions. gpui comes from the `gpui-pre` crates.io snapshot of a Zed commit, the same one gpui-component builds against, all recorded in [sources/manifest.json](../sources/manifest.json).

| Layer | Choice | Why |
| --- | --- | --- |
| UI | gpui with `gpui-component`, restyled with our tokens | Metal rendering and custom elements for timeline, waveform and transcript. A webview would add a frame copy on every preview frame. See [0002](decisions/0002-gpui-and-zero-copy-preview.md) |
| Preview | VideoToolbox decode into IOSurface, shown as a Metal texture in a gpui surface | No CPU copy. Proven first by [milestone 02](milestones/02-preview-spike.md) |
| Capture | ScreenCaptureKit and AVFoundation via `objc2`, no OBS | Separate tracks, cursor as data |
| Recording | Fragmented MP4, HEVC with a 0.5 s GOP, ProRes for camera | Crash safe, scrubbing without proxies |
| Encode/decode | VideoToolbox, FFmpeg (`rsmpeg`) for muxing and odd inputs | Hardware video engines |
| Composition | One wgpu render graph, WGSL effects, LUT only for color | Preview equals export |
| Overlays | vello + parley, typed templates with parameters and keyframes | Replaces Remotion without Node. Agents fill a template spec instead of writing code |
| Transcription | `whisper-rs` large-v3-turbo with Metal, streaming, word timestamps. Parakeet via `ort` as the faster option | Local and free. Separate tracks make speaker detection unnecessary |
| Analysis | Silero VAD (`ort`), transcript similarity for retakes, frame diffs for idle screen, Apple Vision for faces | Auto cut, reframing, thumbnails |
| Audio | DeepFilterNet, biquad EQ and compressor, sidechain ducking, `ebur128` | Built-in voice chain |
| Storage | Project folder in git, SQLite via sqlx in `.shot/cache` for peaks, thumbnails, transcripts and render cache | Delete the cache and nothing is lost |
| Export | Parallel chapter renders, hardware encode, stream-copy concat, segments cached by content hash | Incremental builds |
| AI | The local `claude` CLI, run headless with the `shot` MCP server attached | No API key in the app. See [0003](decisions/0003-claude-through-local-cli.md) |

### Patterns copied from holocron

| Holocron pattern | In Shot Studio |
| --- | --- |
| Short `AGENTS.md`, `CLAUDE.md` pointing at it | Same |
| Milestone briefs, decision records, evidence reports, `STATUS.md` | Same layout under `docs/` |
| Module ownership table, completion report, worktree per subagent | Same, roles adapted in [the development workflow](development-workflow.md) |
| `cargo fmt`, `clippy -D warnings`, `cargo test` as the checks | Same, CI on `macos-latest` instead of Ubuntu |
| CalVer `YY.M.D` with `release.sh` and a tag-checked release workflow | Same, producing a signed and notarized DMG instead of a container image |
| Web app, generated TypeScript types, Docker, storage budgets | Not copied |

## Data and compatibility

- **Project files** are plain text in the channel repo: `video.kdl`, `script.md`, `cut.kdl`, `format.kdl`. Their grammar is owned by [the project files contract](contracts/project-files.md).
- **Media** sits next to the project files and is git-ignored. Recordings are fragmented MP4.
- **Cache** is `.shot/cache` (SQLite plus segment files). It is always safe to delete.
- **Agent proposals** are branches named `proposal/<video>/<slug>`. Accept merges, reject deletes the branch. See [0004](decisions/0004-proposals-as-branches.md).
- **Models** download on first use into `~/Library/Application Support/Shot Studio/models`, pinned by URL and SHA-256. See [0008](decisions/0008-models-download-on-first-use.md).
- **Import** of other editors' projects is not in scope. Plain media files can be added to `sources/` as b-roll.

## Security and privacy

See [0005](decisions/0005-privacy-text-only-to-claude.md).

- Claude receives text: transcripts, scripts, `cut.kdl`, `format.kdl`, `video.kdl` and the sources you attach. Single still frames go only when you trigger a frame task (thumbnail pick, Shorts reframe check).
- Raw audio and video never leave the machine. There is no telemetry.
- Shot Studio stores no Anthropic API key. The YouTube OAuth token lives in the macOS Keychain.
- No secrets, tokens or real recordings are committed to this repo. Test media is generated, see [0007](decisions/0007-generated-fixtures-only.md).

## Targets

Measured on the reference machine: MacBook Pro, Apple M1 Pro, 32 GB, macOS 26 or newer. The workload is the generated 17 min 4K60 project from [milestone 01](milestones/01-contracts-and-fixtures.md).

| Target | Value |
| --- | --- |
| Rough cut and transcript ready after pressing stop | 5 s or less |
| Full export, 17 min 4K60 | under 5 min |
| Re-export after one chapter changed | under 20 s |
| Preview while scrubbing 4K60 | no dropped frames |
| App memory, excluding the loaded model | under 2 GB |
| Seek in the 4K60 preview until a frame shows | under 100 ms |
| Voice-follow lag, word spoken to highlight | under 300 ms at the median |
| Exported loudness | -14 LUFS integrated, within 0.5 LU |

Relaxing a target is Mike's decision.

## Release

See [0006](decisions/0006-mit-open-source-and-distribution.md).

- MIT license, public repo under `coderscantina`.
- macOS 26 or newer, Apple Silicon only.
- CalVer `YY.M.D`, tags created by `release.sh`, release notes from the gitmoji CHANGELOG.
- A signed and notarized DMG on GitHub Releases, plus a Homebrew cask in a coderscantina tap. Auto update comes after the first release.

## Risks

- **gpui** is young and lightly documented. The video surface path is unproven until the spike runs.
- **vello and parley** still change between versions. Pin them and keep the template format independent of the renderer.
- **Scope**: the effect list stays this short until a real video needs more.
- **Elgato Prompter**: whether it mirrors the image for the beam splitter is unknown until tested on the device ([milestone 05](milestones/05-record-comfort.md)).
- **Voice follow** may be too slow on streaming whisper and need a keyword matcher on top ([milestone 05](milestones/05-record-comfort.md)).
