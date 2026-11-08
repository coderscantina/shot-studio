# Milestone 10 items 5 and 6: migration and the renderer on the lock

Branch `task/m10-migrate`. Measured on the reference machine while other agents built and measured in the same target dir, so timings may read a little high.

## What changed

- **Automatic edits are style rules.** The four shipped presets carry the rules their settings stand for (`shot_core::implied_rules`): the auto-cut rules with the pacing's breath (`on silence min=600_000 { cut breath=50_000 confidence=85 }` and the rest), the 112 % punch-in on emphasis, the auto-zoom triggers (`on click { zoom 160 hold=1_000_000 lead=500_000 }`), link cards on `url`, highlights on `ocr`, b-roll slots on idle screen of 8 s and on B markers. The review queue is the lock's `review` instances. A test holds the preset files equal to the generated rules.
- **`shot migrate --video <dir>`** analyses takes without an elements file, then `shot_core::migrate` maps the nodes per the contract's [migration mapping](../../contracts/project-files.md#migration-of-todays-nodes), builds the lock and compares the old and the new timeline segment by segment. It writes nothing when they differ. A channel format without rules gets the rules of its settings.
- **Timeline, renderer and chunk hash read the lock.** `Timeline::styled(project, &lock)` subtracts the union of the lock's cuts from the kept ranges and puts the lock's zooms and overlays next to a person's. `shot_render::lock` reads `cut.lock`; export and `shot check` use it. The chunk hash is unchanged code: it hashes the segments' values, so a lock zoom and a written zoom of the same scale, range and center share a chunk.
- **Freezing.** `shot style build`, `shot export` and `shot check` rebuild a migrated video's lock unless it shipped. `shot style build --all` builds every migrated video of the channel and lists shipped ones as frozen. `shot restyle <slug>` rebuilds a shipped video's lock and says to ship it again. A build refuses a video whose `cut.kdl` still holds rule nodes; `shot autocut`, `shot autozoom` and `shot overlays find` refuse a migrated video.
- **`shot check`** reports each orphan override of the lock under a new `overrides` check.
- **One URL finder and one on-screen text matcher**, in shot-analyze (`elements::urls`, `elements::read_outs`); shot-render's link card and highlight rules call them, its copies are deleted.
- **Channel proposals** (`brand/`, `formats/`) branch off and merge into the default branch; channel files are read from it for every video. A proposal mixing channel and video files is refused.
- **Sound mix** (added mid-task): the export's audio runs a `shot_audio::Mix` instead of the `MusicBed`, with the cut's music bed as a cue at 0 and the lock's `music` and `sfx` instances placed at their output samples. The loudness trials step along the measured slope, since loud effects make the limiter take more as the gain grows.

### Breath and edges

Today's pacing trims breath only off voiced sides; the engine kept breath on both sides except at `t=0`. The engine now keeps breath on a side only where kept material lies beyond it, which equals today's pacing at the take's start and end and everywhere else on the fixture. Chapter and video edges sit on the first and last moments the cuts leave, so `on chapter-start` sounds where the chapter plays, not inside a cut silence. `LOCK_VERSION` stays 1: no lock built by the old engine shipped, and the example lock is unchanged.

## 17 min fixture

A copy of the demo channel with `target-project` as `videos/full-recording-session` (media symlinked), transcribed, rough cut with `shot autocut` (22 silence, 3 pause, 6 filler, 2 retake and 1 misspoken removals, 2 review items, 1 jump-cut zoom) and analysed (1751 elements), then exported, migrated and exported again.

| Step | Result |
| --- | --- |
| Export before migration | 593 of 593 chunks rendered, 290.2 s of output, 181.7 s |
| `shot migrate` | 27 nodes reproduced by rules, 1 override (the jump-cut zoom on its word), 9 manual cuts, 10 ms |
| Export after migration | **0 of 593 chunks rendered**, every one reused; the MP4 and the caption sidecar are byte-identical to the first export (`cmp`), golden frames in `docs/evidence/m10/migrate-*.png` |
| `shot style build` twice, and after deleting `cut.lock` | byte-identical locks (`cmp`), 14 instances, no analysis run |
| Pacing change (`on silence min=600_000 { cut breath=250_000 confidence=85 }` in the video's `format.kdl`) | 10 silence cuts move by 200 ms a side; export renders 14 of 597 chunks, the new edges |

The 9 manual cuts are the fixture's hand-written ranges no element explains: three removals with no finding behind them (a pause at 31.4 s, a filler at 136.2 s, the misspoken one at 534 s) and six chapter-start edges written at 0.8 s while the silence ends 30 to 80 ms earlier. They keep the frames; the silences inside them are still the rules'.

Lock diff of the pacing change, first lines:

```
-    cut from=36_160_000 to=96_720_000 on=silence t=36_110_000 layer=format rule="silence min=600_000"
+    cut from=36_360_000 to=96_520_000 on=silence t=36_110_000 layer=video rule="silence min=600_000"
```

## Channel rule, shipped video, overrides

On a copy of the demo channel with `why-we-deploy-on-fridays` (editing) and `tailwind-without-the-config` (shipped) checked out from their branches and migrated:

- Adding `on chapter-start { sfx swoosh gain=-6 }` to `formats/opinion.kdl` and running `shot style build --all` changed both unshipped locks (+2 and +3 `sfx` instances) and left the shipped one byte-identical ("shipped, its lock is frozen at the release"). `shot restyle tailwind-without-the-config` then added its 3.
- K on the unsure filler at 676.2 s (`override t01 filler at=676_200_000 { keep }`) plus a manual cut at 660 s elsewhere: the filler's review item is gone, 0 orphans. With the filler removed from the elements file, `shot check` prints `overrides: override t01 filler at 676.2 s lost its element: ...` and the lock lists `orphan filler at=676_200_000`.

## Rough cut with style evaluation

`shot style build` on the 17 min take (1751 elements, read, built, written) takes 10 ms wall in five runs. With the elements quick pass (10 ms warm, 264 ms cold, from the elements report) and milestone 04's 0.08 s transcriber flush, the rough cut with style evaluation is ready in under 0.4 s of stop. The app's stop path itself still writes nodes (below).

## Sound

`cargo test --release -p shot-render --test true_peak -- --ignored`: the six speech clips alone and with a bundled bed at -18 dB ducking to -12 dB plus a pop at 0 dB every 1.5 s, encoded to AAC and decoded: -13.84 to -14.21 LUFS, at most -1.66 dBTP. Before the slope step, the long-silence clip with pops ended at -14.60 LUFS.

## Not verified, and gaps

- The app still writes the rough cut, zooms and overlays as nodes (`shot_analyze::autocut::rough_cut`, `shot_render::rules::auto_zoom`, `overlay_rules`), and loads `format.kdl` with `Format::parse`, so it cannot open a migrated video, whose `format.kdl` is a layer. The rough-cut timing above is the CLI path.
- `shot describe`, `shot thumbnails`, the ship plan and the app's model evaluate the timeline without the lock: chapter times in a migrated video's description are off by the rule cuts.
- `scripts/fixtures.sh` copies the presets, now with rules, into each demo video's `format.kdl`; a video copy should be an empty layer.
- Jump-cut punch-ins have no rule form; new videos get none. Cursor-dwell zooms have no element kind.
- Highlights from the `ocr` rule span the phrase; the old rule held 1.5 s past it. Overlays a rule reproduces draw after a person's, so a migration whose old order differs where two overlap is refused rather than reordered; the fixture has none.
- The mix has no motion phase sounds and no bumper ranges yet (no renderer draws bumpers). Beats are detected per file on every export. The preview plays no voice chain or music today; it was not changed.
- Nothing branded draws, so no brand field enters a chunk hash yet.
- Nothing was listened to.
