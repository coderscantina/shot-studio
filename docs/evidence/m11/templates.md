# Milestone 11, work items 5 and 6: templates on the motion system

Measured 2026-11-08 on the reference MacBook Pro M1 Pro, release build, while another milestone agent was building on the same machine (load average 4 to 9). Frames and clips are in `docs/evidence/m11/` (git-ignored).

## What was built

All in `crates/shot-render`, plus the template and feel-control sections of the [project files contract](../../contracts/project-files.md#overlay-templates), the bundled assets in `sources/manifest.json` and `RENDERER_VERSION` 4 in shot-core.

- **Theme** (`theme.rs`): what the brand gives the templates, read from `cut.lock`'s resolved brand, never from the cascade. The preset of the video's format (`motion` node, by default reaction reactive, opinion editorial, product tour launch, tutorial calm) with the brand's `preset` feel controls, the four brand colors, the three fonts, the declared Lottie templates loaded and checked, and the chapter titles a chapter band shows. A video without a lock draws the defaults.
- **Brand fonts reach the renderer** (`typeface.rs`): a `Face` is a font file by content, registered with parley under a unique family name so no installed font stands in. Bundled OFL variable fonts: Geist, Geist Mono, Inter, JetBrains Mono (1.4 MB). A brand font file is read from the channel repo. Template text uses the heading face at 700, body at 500, mono at 400; captions and keystrokes keep the system font.
- **The 07 templates rebuilt**: `link-card` and `lower-third` are cards (`overlay/card.rs`): one sheet that travels in from the left, pops by scale in reactive, sits on frosted glass and tilts in launch, motion blurred, with a soft shadow away from the light; their titles are kinetic type a stagger later, frozen once settled so the sheet texture is reused. `highlight` pops, wipes on (editorial mask) or grows in by preset. `b-roll` stills settle in from a little closer and push in; clips still cut hard. `code-diff` rises in on the springs in the brand's mono font and accent. `enter` and `exit` are duration hints via `Motion::with_hints`. `blur` and `bleep` take neither and still switch hard.
- **New templates**: `chapter-band` (title band, title and "Chapter N" from the cut when not given), `end-card` (backdrop, headline, frames for two 16:9 videos and the subscribe button inside YouTube's inner 90%, `EndScreen::of`), `callout` (ring on a point or outline on a box, label beside it, `callout::snap` to the OCR box under a point or the nearest click within 160 px and 1.5 s), `pop` (bundled animated emoji, a word on an accent pill, or a brand sticker).
- **Brand Lottie templates and re-skins**: a declared template is checked by its slots (`Theme::check`) and drawn full frame; declaring `link-card`, `lower-third`, `chapter-band` or `end-card` re-skins it, its slots filled from what the code template would show. Color slots named after a brand color default to it. Fills are cached per value set.
- **Chunk keys**: `overlay::key_chunks` (replacing `key_assets`) adds the theme key (preset, feel, colors, font hashes and weights) only to chunks where a branded template draws, plus a brand Lottie's file hash and a chapter band's title where those draw.
- **`export_decorated` is gone**: the reel, golden frames, clips and timings now go through real template overlays (`reel.rs`).
- **Animated emoji**: 11 Noto animated emoji (CC BY 4.0) that pass the velato import check, 4 of them after dropping spatial position curves velato ignores anyway.

## Bugs found on the way, both fixed

The golden frames found two sources of frames that differ by history, neither in my templates:

1. **vello 0.11 drops its image atlas** whenever it renders a scene of solid fills only (its resolver returns an empty atlas size, the renderer replaces the atlas texture), while its image cache still counts earlier images as uploaded. The next frame with that image draws it blank: a b-roll still went missing after seeking past a frame with a card's shadow mask. `vector::keep_atlas` puts a transparent gradient into every scene. This also hits bitmap glyphs, so it is my best guess for the 4K glyph drops the motion report mentioned; I saw no glyph drops in any 4K template frame after it.
2. **Atlas position and segment order change pixels.** vello samples an image with float coordinates that include its atlas offset, so the same still drawn from another atlas slot differed by up to 2 code values. B-roll stills are now sheets drawn by vello at 1:1 (exact texels) and scaled by our material shader. And vello sums a pixel's coverage over segments in an order that varies between runs, so a partly covered pixel can flip its last bit: a card's shadow mask behind glass did that at one frame in about half the runs. Shadow masks of rectangles are now snapped to whole texels with square corners (the blur hides the corners). Text and shapes elsewhere keep vello's partial coverage, so a rare one-bit flip remains possible; the golden test did not hit one in three full runs.

## Golden frames

`reel::tests::every_template_frame_is_the_same_reached_by_playing_or_by_seeking`: every template on a 24 s reel with a smooth zoom over a 4K source, all 1440 frames at 60 fps per preset, then 60 pseudo-random seeks per preset alternating between the same renderer and a fresh one. **Passes for all four presets**, twice in a row after the fixes (it failed at reactive frame 1012, editorial 1022 and launch 43 before them).

## Frames per template per preset

`reel::tests::frames_per_template_per_preset` writes `templates-<preset>-<template>.png` (1920x1080 over the screen fixture). What they show:

| Template | reactive | editorial | launch | calm |
| --- | --- | --- | --- | --- |
| chapter-band | band slid in, title words popping past full size | title rising out of a mask word by word | band on frosted glass, tilted on its way, glyphs rising | band fading in with a short slide |
| lower-third | card popped in by scale | card slid in, name masked in | glass card, tilt settling | quiet fade |
| link-card | badge, label, title and mono URL on a dark card, accent stripe | same, title masked in | glass card, title rising glyph by glyph | same, faded |
| highlight | box overshooting then settling | box wiping on from the left | box growing in | box fading in |
| callout | ring and "Deploy button" pill beside the box, following the zoom | same, pill sliding in | pill on its spring | pill fading |
| pop | Noto fire animating, scaled past full size | same, smaller overshoot | same, slower | fade with a small scale |
| code-diff | panel risen in, lines typed in Geist Mono, accent for added lines | same | same | same |
| b-roll | still settling in and pushing | same | same | same |
| end-card | backdrop, headline, subscribe circle and two video frames rising one after another | same | same, glyph by glyph | same |
| blur | the rectangle averaged, switching hard in every preset | | | |

The template clips `templates-<preset>.mp4` are 24 s of the fixture take at 4K60 through the export with every template. I looked at 4K frames of them for glyph drops: none.

These frames are for Mike to judge; nothing here is scored.

## Export timing

`reel::tests::the_full_fixture_with_templates_exports_faster_than_realtime`: the whole fixture main cut (56,274 frames, 15.6 min) in the launch preset (glass, tilt, glyph by glyph text), with every template of the reel every 60 s of the take, on a fresh cache with 3 jobs:

- **97.4 fps, 1.62x realtime** (render 577.5 s, join 0.5 s). Same as the motion reel's 97.1 fps: the hardware encoder stays the limit.

## Preview cost

`reel::tests::preview_frames_with_templates_fit_the_frame_budget`: compose and draw at 1920x1080 over the screen fixture, every frame of the reel:

| Preset | p50 | p99 | max |
| --- | --- | --- | --- |
| reactive | 2.9 ms | 5.2 ms | 16.5 ms (the first frames, while textures are made) |
| editorial | 2.9 ms | 5.1 ms | 6.3 ms |
| launch | 3.0 ms | 5.7 ms | 6.8 ms |
| calm | 2.9 ms | 5.4 ms | 6.5 ms |

## Changing the channel accent

A copy of the target project as `videos/target` in a scratch channel repo (media symlinked), analysed and migrated with `shot migrate`, with a brand of `colors accent="#c8f25a"` and `on chapter-start { overlay chapter-band for=2_500_000 }`, built with `shot style build` (8 chapter bands in the lock, plus the fixture's own lower third):

| Export | Chunks rendered |
| --- | --- |
| first, fresh cache | 1885 of 1885, 56,274 frames, 92 fps (while the test suite ran) |
| after `accent="#ff7a59"` and `shot style build` | **57 of 1885**, 1,566 frames, 24.2 s |

57 is what the branded overlays cover: 8 chapter bands of 2.5 s and a 5 s lower third, on the half-second chunk grid. Every other chunk was reused.

## Editor bench

`shot-app --bench-edit <copy> --glass` on a copy of the target project (unmigrated, so its lower third draws with the editorial preset's theme the engine now loads by itself), 5 runs of 60 s and 60 seeks, load average 9 at the start and 3 at the end: **PASS**, 0 dropped of 17,995 frames, 0 wrong seek frames, seek p50 14.1 ms, p95 31.9 ms, max 504 ms (the cold first seek), peak footprint 340 MB. shot-app built cleanly against this branch; the other agent's app changes were not merged into it.

## Not verified

- Brand fonts from a file in a real channel repo through the export: tested through `Theme::new` with Inter as a file, not with a real `brand/fonts/*.otf` export.
- SVG or PNG logos: the end card does not draw the brand logo yet.
- The license of the Noto animated emoji: decision 0023 says CC BY 4.0 and I recorded that; the brief said OFL or Apache. I could not read the license off Google's page (it renders with JavaScript). Mike should confirm before shipping.
- Sounds per motion phase and beat sync are not wired to templates (items 10 and later).
