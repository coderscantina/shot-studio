# Milestone 12 items 3 and 4: watch builds and proposal previews

Branch `task/m12-watch`, merged with `milestone/10-12` at `c5dd010` (the audio follow-ups). Measured 2026-11-08 on the reference MacBook Pro M1 Pro, release build, while two other agents built and measured on the same machine.

## Verdict

| Criterion | Result |
| --- | --- |
| `shot build --watch` re-exports one changed chapter of the 17 min fixture in under 20 s after a save | **FAIL, 31.4 to 32.8 s.** The save is picked up in under 1.5 s and the changed chapter renders 2 chunks, but every export now spends about 22 s finishing the whole cut's audio before the join can end. See [where the time goes](#where-the-time-goes) |
| A proposal preview plays without changing the video branch's lock or cache | **PASS.** `cut.lock` checksum and the chunk folder's listing are identical before and after `shot build --proposal` and after playing the proposal in the app |
| One save builds once | **PASS.** A plain write and an editor-style save (temp file renamed over `cut.kdl`, then `chmod`) each built once; no second build within 20 s |

## What was built

- **`shot build --video <dir> [--watch]`** (`crates/shot-cli/src/build.rs`). One build rebuilds a migrated video's lock (unless it shipped, as `shot export` does), exports the main cut and every derived cut to `.shot/export/<cut>.mp4`, prints one line, `built main (2 of 1886 chunks rendered), intro-hook (0 of 59 chunks rendered) in 28.6 s`, then runs `shot check` on each cut and prints only its problems. Without `--watch` it exits non-zero when there are problems.
- **Watching** uses `notify` 8.2.1 (FSEvents, file events, no latency; the version already in the lockfile). It watches the video folder, the take folder `media/` where it lies elsewhere (a link in the fixtures), and the channel's `brand/`, `formats/` and `styles/`. Hidden files, editor backups (`~`), `.tmp` files, `.shot/` and `cut.lock` (which the build writes) do not count. After the first event that counts it waits until the files are quiet for 150 ms, then compares a fingerprint of every input file (path, size, modification time) with the one of the last build and builds only when it differs. So the events of one save, and late duplicates, build once, and a save during a build builds again after it.
- **Proposal previews.** `shot build --video <dir> --proposal <name>` reads the files the proposal writes (`Board::proposal_files`, new), puts them over the video's files (`shot_mcp::style::Patched`), builds the lock like `shot style build` and writes it to `.shot/cache/proposals/<name>/cut.lock`. Then `shot_render::export::render_proposal` renders every cut's missing chunks into `.shot/cache/proposals/<name>/chunks/`. A chunk the video's or the channel's cache already holds is read from there and never written; nothing is joined, no audio is encoded. The branch's `cut.lock` and `.shot/cache/chunks/` are never opened for writing.
- **Playing a proposal in the app.** The inline card has Play. It builds the proposal's cut and lock in memory the same way, plans it with `export::preview_plan_with` and its sound with `export::preview_score_with` (the audio follow-ups' path, given the proposal's lock), and plays from 2 s before the card's first change. The button turns into "Back to the cut" (Esc); Esc returns to the branch's plan with the playhead on the same media. While it plays, Reject drops its Esc hint. Any change to the files also returns to the branch.

### Public API

```rust
// shot-board
impl Board {
    pub fn proposal_files(&self, slug: &str, name: &str) -> Result<BTreeMap<String, String>>;
}
// shot-render::export
pub fn proposal_dir(video_dir: &Path, name: &str) -> PathBuf;
pub fn render_proposal(video_dir: &Path, project: &Project, lock: &Lock, name: &str, cut: &str,
    options: ExportOptions, progress: &(dyn Fn(Progress) + Sync)) -> Result<ExportReport, ExportError>;
pub fn preview_plan_with(video_dir: &Path, project: &Project, lock: Option<&Lock>) -> Result<ExportPlan, ExportError>;
pub fn preview_score_with(video_dir: &Path, project: &Project, plan: &ExportPlan, lock: Option<&Lock>) -> Result<voice::Score, ExportError>;
pub fn theme(video_dir: &Path, lock: Option<&Lock>, project: &Project) -> Result<Theme, ThemeError>;
// shot-app::sound
impl Sound { pub fn set_plan_with(&self, map: &EditMap, dir: PathBuf, project: Project, lock: Option<Lock>, frame: u64); }
```

`export`, `preview_plan` and `preview_score` keep their signatures and read the folder's lock as before.

## Setup

A copy of the demo channel in the scratchpad with `target-project` as `videos/full-recording-session` (media linked), transcribed (983 words, 92 s), analysed and migrated with `shot migrate` (1751 elements; 27 manual cuts, 10 overrides, a lock of 2 review items), committed on `main` and `video/full-recording-session`. The cut has the fixture's Short `intro-hook`. The proposal `tighter-plan` is a commit on `proposal/full-recording-session/tighter-plan` that removes "Before we start, here is the plan." (19.29 to 21.63 s) from the intro.

First `shot build`: 1885 of 1885 main chunks and 59 Short chunks rendered, built in 614.9 s; the check then took 43 s more (657.8 s wall, 1.34 GB peak).

## Watch timings

A Python driver starts `shot build --watch`, waits for the first build and its check, deletes one word in `cut.kdl` by splitting its kept range (`keep` / `remove ... by=manual` / `keep`), and timestamps the write and the `built` line on the watcher's stdout. Each save came 75 s after the previous build, so its check had finished.

| Save | Word | Build | Save to `built` |
| --- | --- | --- | --- |
| plain write | "Export" at 775.06 s | main 2 of 1887 chunks, Short 0 of 59, built in 31.1 s | 31.37 s |
| editor save (temp file renamed, then `chmod`) | "git" at 270.28 s | main 2 of 1889 chunks, Short 0 of 59, built in 31.4 s | 32.78 s |
| plain write, during the previous check | "After" at 648.91 s | 28.6 s | 39.10 s, waited for the running check |

No save built twice. A build with nothing changed takes 28.6 to 33.4 s.

### Where the time goes

`shot export --video` with every chunk cached:

```
0 of 1886 chunks rendered, 0 frames in 22.6 s (0 fps), joined in 6.3 s, 28.9 s total
```

milestone 03 measured 2.2 s for the audio and 7 s in all. Sampling this export, the audio thread spends nearly all of its time in `shot_audio::limiter::TruePeakLimiter::process`, then `loudness::Meter::add`, `VoiceChain::process` and `Mix::mix`: the true-peak limiter and its loudness trial passes over the whole 937 s cut. The join interleaves audio after every chunk, so it cannot finish before the audio does. Rendering the 2 changed chunks takes about 2 s alongside. The Short exports in 1.0 s. Reaching 20 s needs the audio finish under about 12 s, which is the audio owner's code (`voice.rs`, `shot-audio`), not this task's.

`shot check` takes about 43 s on the 5.8 GB file. It runs after `built` is printed, but a save during it waits for it.

## Proposal preview and the branch

Checksums of the branch's `cut.lock`, the long listing of `.shot/cache/chunks/` (1954 files) and of the channel's cache, before and after `shot build --proposal tighter-plan`:

```
built proposal tighter-plan: main (2 of 1881 chunks rendered), intro-hook (2 of 55 chunks rendered) in 2.2 s, in .../full-recording-session/.shot/cache/proposals/tighter-plan
d4b3d82e9ac77771c8bc010625a41bb2479a3e834103fb9e5830597a7e8739cb  cut.lock          (before and after)
711dff3ee3f0f95c5b6ff92576b4a529c8f9ae3afe70acfcc036ba5b6194aafc  ls -l chunks/     (before and after)
```

The proposal's space holds its `cut.lock` and the 4 chunks it changes (5.4 MB). The branch's lock checksum was also the same before the app opened, after Play and after Esc (`d4b3d82e...`), and the proposal branch still existed.

### In the app

| Step | Screenshot |
| --- | --- |
| The card with Play, the cut at 15:37, Intro 01:27 | `watch-proposal-card.png` |
| Play: the preview plays the proposal, 15:35 in all, Intro 01:25, "Back to the cut" (Esc) | `watch-proposal-playing.png` |
| Esc: the branch's plan again, 15:37, Play back, the playhead on the same media | `watch-proposal-back.png` |

Differences from the mockup (`docs/mockup/Edit.dc.html`): the mockup has no Play on the card. With Play the four actions wrap: while playing, "Another take" moves to a second row. "Back to the cut" uses the pause icon, as the transport does.

## Not verified

- **The 20 s target**, see above. The proposal preview's own build is 2.2 s.
- **Listening** to the proposal's preview: the sound path is the audio follow-ups' score with the proposal's lock, not listened to.
- **A proposal that deletes a file** is shown with the video's copy of that file. The board's tools only write files, so no proposal does today.
- **Changes to a take** and to `brand/` under `--watch` are covered by the watched folders and the unit test on what counts, not by a timed run.
- **The app's Play on an unmigrated video** plays the proposal's `cut.kdl` without a lock; only the migrated video was run.
- Proposal cache spaces are never deleted, like other stale chunks; accept and reject leave them.
