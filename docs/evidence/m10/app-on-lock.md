# Milestone 10: the app and the ship tools on the lock

Branch `task/m10-applock`. Measured on the reference machine while other agents built in the same target dir, so timings may read a little high.

## What changed

- **The app opens migrated videos.** `VideoFolder` treats a video with a `cut.lock` as migrated: `format.kdl` is a layer, resolved as the preset with the channel's `formats/<format>.kdl` and the video's layer over it. Opening and every write rebuild the lock through `shot_mcp::style::build_video` and write `cut.lock` when it changed, unless the video shipped. A folder outside a channel repo keeps its lock, as `shot export` does.
- **The screens read the lock.** `style::shown` folds the lock into the take blocks for display only: the lock's cuts become rule removals of their element's kind, its `review` instances the queue, its zooms and overlays sit after a person's. `Timeline::evaluate` of the shown cut equals `Timeline::styled` of the files (a test holds this), so Edit, Cut, Picture and Timeline show the frames export renders. Edits go to the real `cut.kdl`.
- **K and C write overrides.** On a migrated video K writes `override <take> <kind> at=<t> { keep }` and C `{ cut }` on the review item's element. Restoring removed words writes the person's removals back to kept media and a `{ keep }` override for each lock cut in the range. Format changes write only the settings that differ from the channel's, keeping the layer's brand and rules.
- **Stop path.** It already wrote elements; on a migrated video it now skips the node-writing rough cut, writes zoom hotkeys as a person's zooms, and the lock is built on the write. When the background `shot analyze` finishes, the lock is rebuilt with the new elements. A video's first take in a channel repo, recorded into a video that is not migrated yet, migrates it first with `shot_core::migrate`.
- **Rule commands in the app.** On a migrated video the Cut screen's rule switches and pacing only show ("From the format's style rules"), Find overlays explains that the rules place them, and the auto-zoom settings no longer rerun auto zoom. The rule editor is milestone 10 item 7.
- **Ship tools.** `shot_ship::lock::timeline` reads `cut.lock`. `describe` takes the main output, `ship::plan` and `thumbnails::find` evaluate with the lock. `shot describe`, `shot thumbnails` and `shot ship` rebuild the lock first, like export and check. The Ship screen's check stamp includes `cut.lock`.
- **Fixtures.** Demo videos are written migrated: an empty `format.kdl` and `cut.lock` from `ready` on, the demo cuts' rule removals become a person's (no elements exist to reproduce them). The locks are committed text built with `shot style build` (and `shot restyle` for the shipped video); a `shot-app` test checks they equal a build.
- **Preview audio.** `score.rs` builds export's sound: voice chain, bleep tones, the bed and the lock's music and sfx through `shot_audio::Mix`, and `Finish` at the gain to -14 LUFS. A seek starts the chain, mix and limiter 1 s early and drops that second; a playing music cue starts at the place in its track export has reached. The gain comes from a pass over the whole cut on its own thread (1.2 s for the 4:50 cut) and plays from the next seek.

## 17 min fixture

A copy of the regenerated demo channel with `target-project` as `videos/full-recording-session` (media symlinked), transcribed, rough cut with `shot autocut`, analysed (1751 elements) and migrated: 27 nodes reproduced by rules, 1 override, 9 manual cuts, 14 instances, 2 review items. Then in the app (`app-cut-queue.png`, `app-cut-answered.png`, `app-edit.png`, 1600x1000, dark):

| Action | `cut.kdl` | `cut.lock` |
| --- | --- | --- |
| K on the filler at 3:41 | `+ override t01 filler at=676_200_000 { keep }` | `- review from=676_200_000 ...` |
| C on the filler at 4:15 | `+ override t01 filler at=802_270_000 { cut }` | the `review` became `cut ... layer=override override=802_270_000` |
| Delete "we record" | `keep` split by `remove from=23_570_000 to=24_090_000 by=manual` | unchanged |
| 3 (Split) on six words | `+ layout split from=28_250_000 to=30_460_000` | unchanged |

Export after the edits: 595 chunks, 289.4 s, 180.1 s. Its chapter markers against `shot describe`:

| Chapter | Export marker | `shot describe` |
| --- | --- | --- |
| Intro | 0.00 | 0:00 |
| Install and setup | 33.30 | 0:33 |
| Project structure | 73.22 | 1:13 |
| Recording a take | 113.75 | 1:53 |
| Auto cut | 155.30 | 2:35 |
| Review and notes | 193.35 | 3:13 |
| Export | 228.92 | 3:49 |
| Wrap up | 265.55 | 4:25 |

`describe` marks a chapter where its first word is heard, so Export's first word at 229.0 s rounds to 3:49 while the marker sits 0.1 s earlier. Every other mark matches to the second. The app's overview shows the same 4:49 length.

## Preview against export

A temporary test (removed) ran the preview's `Finished` from five positions for 5 s and compared it with the export's audio decoded by ffmpeg:

| From | Lag | Correlation | Residual under the export | Preview / export RMS |
| --- | --- | --- | --- | --- |
| 5 s | 0 | 0.9992 | 23.1 dB | -14.54 / -14.03 dBFS |
| 37 s | 0 | 0.9992 | 23.6 dB | -13.90 / -13.43 |
| 61 s | 0 | 0.9989 | 23.4 dB | -13.88 / -13.44 |
| 150 s | 0 | 0.9991 | 23.0 dB | -14.32 / -13.82 |
| 260 s | 0 | 0.9992 | 23.8 dB | -14.58 / -14.11 |

The preview is about 0.5 dB quieter: export's limiter trials raise the gain past the first pass's 9.78 dB. The residual is that level step plus AAC. A seek against one stream read from 0 differs 41 to 67 dB under the signal in its first half second and 27 to 56 dB under in its fifth second: the voice chain carries state longer than the 1 s preroll. `PcmReader` reads the export 2112 samples (the AAC priming) late; ffmpeg's decode has no lag.

A/V sync, measured as in [m03 audio](../m03/audio.md) with a temporary log (removed): every newly shown frame against the output frame the sound clock heard one refresh later, on the migrated video.

| Stretch | Frames shown | Skipped | Shown twice | Frame start minus sound heard, min / p50 / max |
| --- | --- | --- | --- | --- |
| Play from 0 | 597 | 0 | 0 | -18.2 / -14.1 / -9.8 ms |
| Seek while playing | 487 | 0 | 0 | -7.8 / -4.4 / -1.1 ms |

`shot-app --bench-edit` on the plain (unmigrated) copy: PASS, 0 dropped frames in 5 x 60 s, seek p95 20.8 ms, 274 MB peak footprint.

## Mockup differences

- Cut on a migrated video: the rule rows have no switches and the pacing control is gone, which the mockup's Review screen has. Waiting for the rule editor design round.
- The lock's overlays show on the Timeline lanes but not in the Overlays pane, which lists a person's overlays to edit.

## Checks

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `bash -n` and `shellcheck scripts/fixtures.sh` pass. `scripts/fixtures.sh demo` into a temp `SHOT_FIXTURES` twice (`--force`): `demo-channel.refs` unchanged. `shot style build --all` on its `ready`, `recorded` and `editing` branches: every lock unchanged. The ignored demo tests of shot-board and shot-mcp pass on it. The shared fixtures were not regenerated.

## Not verified

- Nothing was listened to; the comparison is numeric.
- Recording a new take into a migrated video, and the migration of a video on its first take, were not run with the recorder; the first-take migration has no test.
- The ship screen and `shot ship` were not run on the migrated video (no OAuth client); `ship::plan` is covered by `shot describe` sharing its timeline.
- Thumbnails on the migrated video were not run (the fixture's pixel face is not detected).
