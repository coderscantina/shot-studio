# Milestone 11, work items 1 to 4: motion system, materials, kinetic type, Lottie

Measured 2026-11-08 on the reference MacBook Pro M1 Pro, release build, while two other milestone agents were building and testing on the same machine. Clips and frames are in `docs/evidence/m11/` (git-ignored).

## What was built

All in `crates/shot-render`, as a renderer library with typed inputs. Nothing reads the style language yet.

- **Motion system** (`motion.rs`): closed-form damped springs (`Spring`), the four presets (`Preset`) with feel controls (`Feel`), and choreography (`Motion::pose`, `Motion::phase`). A pose is a pure function of the element's range, its part and the media time. Exits respond in 60% of the entrance's time without bounce, parts stagger in reading order and exit at half the stagger, and `with_hints` fits the springs to `enter` and `exit` durations for item 5. Zoom moves (`zoom-motion smooth`) now run on a critically damped spring fitted to the old 450 ms ease.
- **Materials** (`materials.rs` pure, `material_pass.rs`, `blur.wgsl`, `material.wgsl`): a separable gaussian at a quarter of its width (at most 4x down) for soft shadows of any shape (`SoftShadow`) and frosted glass. `Sheet` draws shapes flat once, caches the texture while they do not change, and places it through a perspective transform: scale, tilt about both axes, sheen from the one `LIGHT`, glass behind it. `parallax` and `drift` move sheets by depth. Shadows take their offset from the same light.
- **Adaptive motion blur**: a 1/120 s shutter around each frame. Sheets average 1 to 8 placements across it in the shader; a zoom's screen layer averages 1 to 8 crops in `compose.wgsl`. The sample count comes from on-screen travel in 4K pixels (`materials::samples`), so preview and export take the same count.
- **Kinetic type** (`kinetic.rs`, drawn in `vector.rs`): parley lays the text out once; words or glyphs are grouped from its clusters and each moves on its own staggered spring with one of four reveals: rise, mask (slides up out of its line), pop (scales past full size and settles) and fade. Word pop captions in derived cuts now scale the spoken word up and let it settle (`kinetic::word_pop`); the pop time enters the derived chunk key.
- **Lottie** (`lottie.rs`): velato 0.12 without its `vello` feature, drawn onto vello 0.11 through our own `RenderSink`. kurbo 0.13 and peniko 0.6 match, so no fork or `[patch]` is needed. The pin and the reason are in the root `Cargo.toml` and in `sources/manifest.json` (revision 8f9cce6, Apache-2.0 OR MIT). `lottie::check` lists every unsupported feature and layer. `LottieTemplate` binds declared typed slots through Lottie's own slot ids (`sid`), never layer names, is hashed by content (blake3), and `Shape::Lottie` draws it with text slots set by parley.
- **Export seam**: `export::export_decorated` takes a `Decoration` that draws on every main-cut frame and whose name enters the chunk keys. It lets the reel and the timing below go through the real export before item 5 rebuilds the templates. Remove it once templates draw these moves.

## Feel controls

The contract's open question, settled here. Raw spring values stay inside the presets.

| Control | 0 | 100 |
| --- | --- | --- |
| `tempo` | entrance response 900 ms | 225 ms, halving every 50 |
| `bounce` | damping ratio 1, no overshoot | 0.4, about 25% overshoot |
| `density` | 30% of a template's travel and tilt | 120% |
| `stagger` | parts move together | 120 ms between parts |

| Preset | tempo | bounce | density | stagger | Text reveal | Glass | Tilt at full density |
| --- | --- | --- | --- | --- | --- | --- | --- |
| reactive | 80 | 60 | 70 | 40 | pop per word | no | 0 |
| editorial | 60 | 20 | 60 | 50 | mask per word | no | 0 |
| launch | 40 | 10 | 30 | 60 | rise per glyph | yes | 14 degrees |
| calm | 30 | 0 | 10 | 20 | fade per word | no | 0 |

## Golden frames

`reel::tests::every_frame_is_the_same_reached_by_playing_or_by_seeking` renders the 6 s reel of every preset at 60 fps over a 4K source with a smooth zoom, then 40 pseudo-random seeks per preset, alternating between the same renderer and a fresh one. The pixels hash identically: **passes for all four presets**.

It found two real defects on the way, both fixed:

- vello's `Compose::Plus` blend renders differently on two fresh renderers, so motion blur by averaging vello layers is not deterministic. Moving overlays are sheets instead, averaged in our shader.
- vello writes straight alpha, whose color is noise where alpha is near zero. Bilinear sampling of straight texels let that noise into glass edges (up to 5 code values, differing between runs). The sheet shader now filters premultiplied texels.

## Demo clips

`reel::tests::demo_clips_per_preset_through_the_export` exports 6 s of the fixture take per preset through `export_decorated` at 4K60 HEVC, with a 1.6x zoom springing in at 1.5 s and out at 4.5 s: `reactive.mp4`, `editorial.mp4`, `launch.mp4`, `calm.mp4`. Each reel has a card on a sheet with a soft shadow, a kinetic headline, a badge flying in, word pop captions and a Lottie sticker.

What the frames show (`launch-*.png`, `reactive-*.png`, read at 0.55, 1.2, 2.6 and 4.95 s):

- 0.55 s: the card is still travelling in from the left, smeared horizontally by motion blur, its text unreadable on purpose. In launch it is tilted and its glass shows the face behind it blurred.
- 1.2 s: the card is at rest. In launch, the frosted glass shows the camera frame behind it as soft brown and blue, the shadow falls down and to the right, away from the top-left light. The badge has landed at the right. The headline is rising glyph by glyph (launch) or word by word.
- 2.6 s: the zoom has sprung in without overshoot. The caption's third word sits on its accent pill, scaled up past its neighbours. The Lottie star is turning, with its "NEW" label set by parley.
- 4.95 s: the card exits faster than it came in, motion blurred again, and the headline is gone.

The reels are for Mike to judge (item 13 is the scored reel); I have not scored them.

## Export timing

`reel::tests::the_full_fixture_with_glass_and_motion_blur_exports_faster_than_realtime`: the fixture's whole main cut (56,274 frames, 15.6 min) with `zoom-motion smooth` and the launch reel, the costliest preset, for 6 s of every 20 s. That is a glass card, shadow, sheets, kinetic headline, caption and Lottie on 30% of the frames. On a fresh cache with 3 jobs:

- **97.1 fps, 1.62x realtime** (render 577.8 s, join 1.7 s).

That matches the 96 fps measured with every 07 overlay at milestone 07: the hardware encoder stays the limit, and glass and motion blur cost no measurable export time at this density.

## Preview cost

`shot-app --bench` was not run for this. It benches the milestone 02 spike player, which draws through its own zoom shader, not through shot-render, so it cannot turn on glass or motion blur. What the editor's preview does per frame is `compose` plus `Renderer::draw` at 1920x1080 on its render thread, measured by `reel::tests::preview_frames_with_glass_and_motion_blur_fit_the_frame_budget` over the 4K screen fixture with a smooth zoom (decode excluded, measured apart in milestone 02):

| Preset | p50 | p99 | max |
| --- | --- | --- | --- |
| reactive | 3.7 ms | 8.1 ms | 22.5 ms (first frames, while the sheet textures are made) |
| editorial | 3.8 ms | 5.3 ms | 5.4 ms |
| launch | 4.0 ms | 5.3 ms | 5.6 ms |
| calm | 3.7 ms | 5.1 ms | 5.5 ms |

A run while the other agents were rendering measured editorial at p99 23.1 ms and max 48.6 ms; the rerun above came right after.

The budget at 60 fps is 16.7 ms and the engine renders up to 6 frames ahead. The first version allocated new shadow and backdrop textures every frame while a card moved (p99 23.7 ms, max 60 ms). Texture sizes now round up to 32 texels, so moving materials reuse their textures.

## Lottie

- A Lottie with an After Effects expression is refused naming it: `layer `Logo`: expression on position: `wiggle(2, 30)``.
- One check lists every unsupported feature at once: expressions, AE effects (`ef`), layer styles (`sy`), text layers without a slot, image, solid, audio, camera and other layers, 3D layers, split rotation, split positions in shape groups, mask modes other than add, inverted masks, mask opacity and expansion, dashes, merge, twist, pucker and bloat, offset path, rounded corners, zig zag, the add and hard mix blend modes, luma mattes, time remap, auto-orient, even-odd fills, spatial position easing. A file that passes is parsed once more under `catch_unwind`, so a velato panic becomes a finding.
- A simple Lottie renders: the reel's star sticker (an 8-point star whose fill color is a `color` slot, with a `text` slot label parented to it) pops in and turns in every clip.

## Not verified

- `shot-app --bench` with glass and motion blur: see preview cost.
- Brand fonts for text slots and kinetic type: brands are not in the renderer yet, so text draws in the system sans.
- Lottie chunk keys: the template hash is there, but no export path draws declared templates yet (milestone 10 and item 6).
