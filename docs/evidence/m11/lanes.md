# Milestone 11, work item 11: lanes and placement in the app

Branch `task/m11-lanes`, all in `crates/shot-app`. Measured 2026-11-08 on the reference MacBook Pro M1 Pro while two other milestone agents built in the same target dir (load average 6 to 15), so timings may read high.

## What changed

- **Overlays lane.** The B-roll lane is now Overlays, violet, for every template but `highlight` (which keeps its lime lane): b-roll, cards, lower thirds, chapter bands, callouts, pops, blur and bleep. Overlays that overlap share the lane in two rows. Blocks name what they show (the band's or callout's text, the pop's emoji or sticker).
- **Overlays pane.** It now lists the lock's instances around the playhead next to a person's, each with a line saying who made it: "Rule on chapter-start in the brand", "Override in cut.kdl", or "Yours, in cut.kdl". Only a person's rows can be changed (Pick, Remove, boolean toggles). The full provenance UI and the scope choice wait for the design round, as asked.
- **Sound and Music lanes.** A neutral Sound lane below Voice marks each `sfx` instance of the lock at its output time, named by its sound. Music shows the `cut.kdl` bed across the cut and each lock `music` cue until the next, as blocks. Clicking the Sound lane moves the playhead there and opens the sound picker above it.
- **Transitions** are marks down the lanes on their chapter edge with the kind's name in the ruler row ("Dip"). A ruler timecode gives way where the two would overlap.
- **Sounds.** `S`, the Sound tool in the transcript's selection toolbar, the Sound lane and Picture's Sound button open one picker: the video's `sounds/`, the channel's `brand/sounds/`, then the bundled effects, a name found earlier hiding a later one like the lookup. A pick writes `override <take> word at=<t> { sfx <name> }` on the word spoken at the selection's start (or at the playhead, or the nearest word within 1 s). Placing again on that word replaces its sound and keeps the override's other changes. A video without a lock is told to migrate first; a moment without a word nearby is refused with a note.
- **Callouts.** Picture's Callout tool turns the preview into a target: a click maps back through the drawn frame's screen layer (frame and crop, `engine::ScreenMap`) to screen pixels and writes `overlay callout from=<t> to=<t+3 s>` in the take block, snapped with `callout::snap` to the OCR box under the point or the nearest click within 160 px and 1.5 s. A click off the screen recording says so.
- **Pops.** Picture's Pop picker shows the 11 bundled animated emoji (`pop::EMOJI`) and the brand's stickers (its Lottie templates under new names). A pick writes `overlay pop` for 1.2 s at the playhead.
- **Lottie import.** "Add Lottie…" in the Pop picker opens a file dialog, runs `lottie::check` and either lists every finding ("layer `Logo`: expression on position: `wiggle(2, 30)`") and writes nothing, or copies the file to `brand/<name>.json` and declares `template <name> file=...` in `brand/brand.kdl` through `shot_core::Brand`, then rebuilds the lock. Built-in template names and existing names are refused.
- **Theme and Assets.** `VideoFolder::theme` is `Theme::load` of the files, read again after every write (the lock, the brand and the cut's chapter titles feed it). The preview plan carries it and the render thread makes new `Assets` only when it changes, so a lock change always reaches the preview and a view toggle or seek no longer re-reads event logs. The Overlays pane checks with `Theme::check`, so brand templates show as "a Lottie template of the brand" instead of unknown.
- `shot-app --bench-edit` passes the theme into its plan.

`place.rs` holds the pure placement functions, `edit/place.rs` the pickers and tools.

## On a migrated copy of the target project

A scratch channel repo with `target-project` as `videos/target` (media symlinked), transcribed, analysed (1751 elements) and migrated with `shot migrate`, plus a brand with `on chapter-start { overlay chapter-band for=2_500_000; sfx swoosh; transition dip }` and a `music "bed.flac"` cue on `video-start`, a `sounds/ding.flac` in the video and `brand/sounds/chime.flac`. Then in the app: the word "channel." selected and `ding` picked; a callout clicked on the split layout's screen at 4:37 near a click; 🔥 popped there.

`cut.kdl`:

```diff
         layout split from=294_000_000 to=324_000_000
+        overlay callout from=296_600_000 to=299_600_000 {
+            x 1_400
+            y 900
+        }
+        overlay pop from=296_600_000 to=297_800_000 {
+            text "🔥"
+        }
     }
 ...
+override t01 word at=1_630_000 {
+    sfx ding
+}
```

`cut.lock`:

```diff
     music "bed.flac" at=800_000 gain=-20 duck=-12 on=video-start t=800_000 layer=brand rule="video-start"
+    sfx ding at=1_630_000 on=word t=1_630_000 layer=override override=1_630_000
```

The callout snapped to the click at (1400, 900). The preview shows the ring on the click and the fire emoji (`lanes-picture-placed.png`), and an X clip of 0.8 to 5 s and 295.5 to 300 s exported through `shot export --cut` in 7.0 s (18 chunks, 75 fps) draws both (`lanes-export-placed.png`).

The app was driven by a temporary in-app script for these screenshots (synthetic clicks could not reach the window on the shared desktop); it called the same methods the buttons and keys call and was removed before the commit. Clicks, keys and the file dialog themselves were not exercised.

## Editor bench

`shot-app --bench-edit` on the migrated copy (67 overlays, the lock's theme): **PASS**, 1 dropped of 17,995 frames (run 1, load average 14.7 at the start), 0 wrong seek frames, seek p50 12.7 ms, p95 33.7 ms, max 516 ms (the cold first seek), peak footprint 324 MB.

## Screenshots against the mockup

1600x1000 in dark, git-ignored, in this folder: `lanes-text-sound-picker.png`, `lanes-picture-callout-tool.png`, `lanes-picture-pop-picker.png`, `lanes-picture-placed.png`, `lanes-timeline-placed.png`, `lanes-timeline-start.png`, `lanes-timeline-sound-picker.png`, and the mockups rendered with WebKit as `mockup-Edit.png`, `mockup-EditPicture.png`, `mockup-EditTimeline.png`.

- **Timeline**: the mockup predates the effects grill and has B-roll, no Sound lane and a 36 px Music lane. Here Overlays replaces B-roll, Sound (28 px) sits between Voice and Music, and Music is 28 px so all eight lanes fit at 1000 px. The mockup's Music block reads "Bed, ducked under voice"; here it names the file. Transition marks are not in the mockup; design.md only says "a mark on the chapter edge", so their look (a 1 px line at 35% text and a small chip in the ruler) is my choice and worth a look.
- **Sound picker** in Timeline floats over the upper lanes, anchored above the Sound lane at the left of the track, not at the playhead.
- **Text**: the selection toolbar gains Sound (S) and opens the picker under itself, pushing the next paragraph down rather than floating. The mockup's toolbar has Highlight, Zoom and B-roll, still missing as before.
- **Picture**: the mockup has no placement tools; they sit in the right pane under "Place at the playhead" as plain buttons, the pickers inline below. The mockup's caption panel and on-screen layer list with toggles are still missing as before.
- **Overlays pane**: rows of the lock have no actions, and every overlay of a take shows once per row, not grouped by rule.
- The brand's automatic per-phase sounds (`sounds card-settle=...`) are computed by the renderer, not lock instances, so the Sound lane does not show them.

## Checks

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` pass. New tests in `place.rs`: a sound is an `sfx` override on the word spoken then, and refused away from words; a callout snaps to a nearby click and keeps a free point otherwise; a Lottie with an expression is refused with it named and a clean one is copied and declared.

## Not verified

- Real clicks, `S`, Esc and the macOS file dialog for "Add Lottie…" (driven by method calls, see above). The Lottie import ran only in its unit test.
- Listening: the preview and export audio with `ding` were not listened to or measured, and the X clip's audio was not checked for the sound.
- Callout mapping on a zoomed frame and on the `window` layout: tested on `split` only.
