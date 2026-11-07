# Milestone 07 evidence: overlays and derived cuts

Recorded 2026-11-07 on `task/m07-overlays`. Machine: MacBook Pro, Apple M1 Pro, 32 GB, macOS 27, rustc 1.99.1. The milestone 08 agent built, exported and ran the app on the same machine and target dir during most measurements (load average 8 to 12), so timings carry that noise. Screenshots and frames are in `m07/` (git-ignored).

## Result

| Criterion | Evidence | Result |
| --- | --- | --- |
| A derived Short of the 17 min fixture exports and reuses cached segments from main where pixels match | After the main export of a copy of `target-project`, `shot export --cut workflow` (a Short of two clips, 5.2 to 24.8 s in the `face` layout and 300 to 318.5 s in `split`, 38.1 s) drew from 77 main chunks: **77 reused, 0 rendered**, then rendered its own 77 chunks in 10.8 s. The fixture's own `intro-hook` Short: 59 of 59 main chunks reused. The X clip `setup`: 30 of 30. The ignored test `a_short_reuses_main_chunks_and_a_parameter_re_renders_only_its_chunks` asserts the same on a short cut (8 of 8 reused) | Pass |
| A link card appears at the word where its URL is said | Demo project below: narration "lives at github dot com slash coderscantina slash shot studio", transcribed by whisper as `github.com slash cotterscantina slash shot studio.`; with two caption fixes `shot overlays find` wrote `overlay link-card from=12_650_000` with `url "https://github.com/coderscantina/shot-studio"`, 12.65 s being the start of the word `github.com`. Frame at 14 s: `m07/demo-14.png`. Unit tests `finds_written_and_spoken_urls_at_their_first_word` and `the_card_shows_from_the_word_where_the_url_is_said` (no card 1 µs before the word, a card from it until 6 s later) | Pass |
| Face reframing keeps the face inside the 9:16 frame on the camera fixture | Vision finds no face in the fixture's flat-shaded face (0 of 83 samples on `target-project`, also checked on single frames), so on the fixture the window stays centered. Tested instead on a generated clip Vision does detect: the Apple Color Emoji person with medium skin tone, 910 px, moving 800 px left to right over 10 s at 1080p60. `reframing_keeps_the_face_inside_the_short` (ignored test): Vision found the face in 22 of 22 samples and every detected face box lies inside the window on all 20 sampled frames inside the clip. Exported Short at 1, 5 and 9 s: `m07/short-face-follow.png` | Pass on the generated clip; the fixture face is not detectable |
| Changing a template parameter re-renders only the segments that use it | `lower-third` text changed in the 17 min copy: main re-rendered **10 of 1885** chunks (the 3 to 8 s the overlay covers) in 10.2 s; the Short re-rendered 6 of 77 chunks, the X clip none, `intro-hook` 9 of 59, all from cached main chunks. The ignored test changes one `link-card` URL: 2 of 10 main chunks and 2 of 8 Short chunks | Pass |

## How it works

- **Templates** (`shot_render::overlay`, decision [0018](../decisions/0018-overlay-templates-as-typed-specs.md)): a `TemplateSpec` per template with typed parameters, checked at export. `enter`/`exit` keyframes ease every animated template. Templates: `link-card`, `lower-third` (the target fixture already used it), `highlight`, `b-roll`, `code-diff`, `blur`, `bleep`. A file an overlay names enters the chunk key with its size and modification time.
- **Rules** (`shot overlays find`, `overlay_rules::run`): link cards from URLs said, written or spoken, read with the caption fixes applied; b-roll slots over idle screen of at least 8 s (04's `still_ranges`) and at every B marker (05's `read_markers`); highlights where at least three spoken words are on screen at that moment, read with Apple Vision text recognition (`shot_analyze::vision`, cached in `.shot/cache/text/`). Each rule skips what its template already covers.
- **Bleep and blur from the transcript**: `shot overlays bleep|blur --take --phrase` find the words; blur covers where Vision finds them on screen, from when they are said for `--hold` seconds. Blur averages the screen over 32 px blocks in the compose shader (a second screen layer), bleep silences the voice before the voice chain and adds a 1 kHz tone after it.
- **B-roll**: stills fade in with a 6 % push-in through vello; clips cut in hard as a cutaway that replaces the screen layer, in export and in the preview; diffs type out like `code-diff`. `shot overlays b-roll --at --file` or the Overlays pane fills a slot.
- **Derived cuts** (`shot_render::derived`, decision [0017](../decisions/0017-derived-cuts-from-main-chunks.md)): a derived frame is a window of the main frame, scaled to 1080x1920 or 1080x1080, captions burned in with 04's cues on 06's vector pass (Word pop: the spoken word on a lime pill). Export renders missing main chunks first, then the derived chunks from them. The derived key extends its contract hash with the base chunk key, the window per frame and the caption per frame. Shorts follow the largest face, sampled every 0.5 s with Vision (`.shot/cache/faces/`).
- **CLI**: `shot derived add|list|remove`, `shot export --cut <name>` (prints the base reuse), `shot overlays find|bleep|blur|b-roll|check`.

## Demo project

A 32 s 4K content-layout take generated for this run (not a fixture): a static "TEAM-RULES.md" page with a test pattern moving until 22.5 s, a B marker at 0.5 s, and `say` narration from 5 s. Transcribed with large-v3-turbo, then:

```
$ shot caption-fix --video demo cotterscantina coderscantina
$ shot caption-fix --video demo "shot studio" "shot-studio"
$ shot overlays --video demo find
1 link cards, 2 b-roll slots, 1 highlights added
$ shot overlays --video demo blur --take t01 --phrase swordfish --hold 3
$ shot overlays --video demo bleep --take t01 --phrase swordfish
$ shot overlays --video demo b-roll --take t01 --at 1 --file sources/dashboard.png
```

```kdl
        overlay link-card from=12_650_000 to=18_650_000 {
            url "https://github.com/coderscantina/shot-studio"
        }
        overlay b-roll from=500_000 to=4_500_000 {
            file "sources/dashboard.png"
        }
        overlay b-roll from=22_500_000 to=31_983_333
        overlay highlight from=9_010_000 to=12_650_000 {
            x 1_145
            y 524
            width 1_111
            height 107
            text "never deploy on Fridays."
        }
        overlay blur from=18_970_000 to=22_800_000 {
            x 1_603
            y 1_290
            width 489
            height 144
        }
        overlay bleep from=18_970_000 to=19_800_000
```

Frames: `demo-2.png` (b-roll still), `demo-10.5.png` (highlight around the phrase read out), `demo-14.png` (link card), `demo-20.png` (blurred password), `demo-short-6.png` (its Short, captions burned in). The second b-roll slot covers the idle outro; filled with a clip (`shot overlays b-roll --at 25 --file sources/camera-clip.mp4`, a copy of `camera-30s.mp4`) it cuts away to the clip, `demo-25-clip.png`. The re-export rendered 54 of 64 chunks: the 10 that no overlay touches were reused. Bleep, measured with ffmpeg on the export: 19.1 to 19.6 s is -13.5 dB mean with -13.6 dB of it in a 60 Hz band at 1 kHz; speech around it is -13.4 dB with -31.6 dB in that band.

## Measurements

| What | Number |
| --- | --- |
| Main export of the 17 min copy, cold cache | 1885 chunks, 56274 frames in 580 s (97 fps), 9 min 42 s total, 12 GB of chunks |
| Short `workflow` (38.1 s), main cached | 77 of 77 bases reused, 77 chunks in 8.7 s (263 fps), 10.8 s total including 83 Vision face samples |
| Same Short again | 0 rendered, 0.6 s |
| Same Short on a copy with no cache | 77 bases and 77 derived chunks rendered, 33.4 s total |
| `intro-hook` after `workflow` | 28 of 59 rendered: 31 derived chunks are identical to `workflow`'s and shared |
| Demo, 32 s 4K, all overlays | 1920 frames in 20.0 s (96 fps); the same take without overlays 96 fps |
| Transcribing the 17 min take | 100 s |

## Overlays pane

`m07/edit-timeline-overlays.png` (1600x1000): the Timeline workspace gains B-roll and Highlight lanes and an Overlays pane with Find, the overlays around the playhead with their template's description and parameters, Pick or Next file for b-roll, Remove, a toggle for boolean parameters, and the Shorts and X clips with Export. Differences from the mockup (`EditTimeline.dc.html`):

- The mockup has no Overlays pane; it takes a third column of the top row, so the change list is narrower.
- Lane icons reuse `picture` (b-roll) and `edit` (highlight); the Hugeicons for b-roll and highlight are not in `assets/icons` yet. The b-roll violet is a local constant, not a theme token.
- Text and integer parameters are shown, not editable; editing them goes through `cut.kdl` or the CLI.
- No Music lane, no H and Z selection toolbar, no b-roll or highlight tags in the Text workspace.

## Not verified

- Face reframing on a real face. The fixture face is not detected; the emoji clip stands in.
- The Overlays pane's buttons were not clicked in the running app; each goes through `VideoFolder::write_cut` like the CLI runs above. Find runs off the UI thread.
- Pixel equality of a derived frame with its main frame was not measured; it is the same decoded main chunk, re-encoded once at quality 0.75.
- Preview of derived cuts: the app previews the main cut only.
- Bleep in the preview: the app's sound path does not pass bleeps to `TimelineAudio` yet, so only exports bleep.
