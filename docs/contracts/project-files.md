# Project files contract

This contract owns the files a video consists of and what each one means. `shot-core` implements it: it parses every file from text and writes it back with no IO. The examples below are the files in `fixtures/examples/`, and the `shot-core` tests check that each one parses, writes back byte-identical and stays identical to what this document shows.

## Channel repo layout

```
<channel repo>/
  .gitignore            ignores media/ and .shot/
  videos/
    <slug>/
      video.kdl         status, format, target date, title
      script.md         chapters as headings, beats as bullets
      cut.kdl           the edit: ranges, layouts, effects, derived cuts
      format.kdl        copied from a format preset, editable per video
      sources/          links, article snapshots, notes (text, in git)
      media/            recordings and b-roll (ignored)
        <take>/         one folder per take, named by its take id
          events.jsonl  cursor and key events, see below
      .shot/cache/      SQLite and rendered chunks (ignored, safe to delete)
  formats/              the four shipped presets, editable per channel
```

## What each file owns

- **`video.kdl`**: the card on the board. Status is one of `ideas`, `scripting`, `ready`, `recorded`, `editing`, `shipped`. Moving a card rewrites this field and commits.
- **`script.md`**: the spec. `#` is the title, `##` a chapter, a bullet a beat. Beats link to a source in `sources/`. The prompter reads this file.
- **`cut.kdl`**: the edit. It references recordings by take id and media time, never by frame index of an output. It holds:
  - kept ranges per take, with the rule or person that decided each removal
  - the review queue: findings a rule was not sure about, and the Keep decisions on them
  - layout changes, zooms with what wrote them, captions style, caption fixes, overlays (template name plus parameters), the music bed
  - derived cuts (`short`, `x-clip`) as a few lines on top of the main cut
- **`format.kdl`**: pacing, default layouts, caption style, auto-cut thresholds, prompter mode, and the product video settings: auto zoom, zoom motion, cursor, keystrokes, window styling.

## Rules every module follows

- Every UI action that changes the edit becomes a change to one of these files. There is no hidden edit state.
- Files are written in a stable order so a diff shows only what changed.
- `shot-core` parses and evaluates these files with no IO. Unknown nodes are an error with a line number, never silently dropped.
- The cache is derived from these files and media only. Deleting `.shot/cache` loses nothing.
- A rendered chunk's cache key is a hash of everything that affects its pixels: source media ids, ranges, effects, template parameters, format settings and renderer version.

## Common rules for the KDL files

The three `.kdl` files use [KDL v2](https://kdl.dev).

- **Strict.** An unknown node, an unknown property, a missing required node, a wrong number of arguments, a type annotation or a bad value is an error. Errors name the file, line and column, for example `cut.kdl:15:46: unknown value ...`.
- **Canonical form.** `shot-core` writes nodes in the order this document lists them, properties in the order shown, four spaces of indent, one blank line between top-level blocks in `cut.kdl`, and a trailing newline. A canonical file writes back byte-identical. A file in any other valid form is accepted and comes back canonical, so the first save may reformat it.
- **Comments are not kept.** Notes about a video belong in `script.md` or `sources/`.
- **Time** is integer microseconds of a take's media time, counted from the start of the take. Canonical form groups digits by three, so `12_500_000` is 12.5 s. Ranges are half-open, `from` is included, `to` is not, and `from` must be before `to`. Microseconds keep the files exact and independent of frame rate; the renderer snaps to the output frame grid.
- **Ids** (take ids, template and parameter names, derived cut names) are lowercase ASCII letters, digits and dashes, starting with a letter. They are written bare.
- **Keywords** (statuses, layouts, styles, rule names) are written bare. Free text (titles, text parameters) is always quoted.
- **No floats.** Percentages and other fractional values are integers in a stated unit, so every value round-trips exactly.

## `video.kdl`

| Node | Value | Required |
| --- | --- | --- |
| `title` | text | yes |
| `status` | `ideas`, `scripting`, `ready`, `recorded`, `editing`, `shipped` | yes |
| `format` | `reaction`, `opinion`, `product-tour`, `tutorial`: the preset `format.kdl` was copied from | yes |
| `target-date` | text `"YYYY-MM-DD"`, a valid calendar date | no |

Each node appears at most once and takes one argument.

```kdl
title "Why we deploy on Fridays"
status editing
format opinion
target-date "2026-11-20"
```

## `format.kdl`

| Node | Value |
| --- | --- |
| `prompter` | `beats` or `full-script` |
| `pacing` | `loose`, `natural` (0.25 s of breath around cuts) or `tight` (jump-cut style) |
| `layouts` | one or more layouts. The first is the default wherever `cut.kdl` sets none, the rest are offered on the 1 to 4 hotkeys |
| `captions` | `word-pop`, `clean` or `off` |
| `auto-cut confidence=<0..100>` | block with one node per rule. Results at or above `confidence` percent apply right away, the rest go to the review queue |

Layouts are `face`, `face-content`, `split`, `content`, `screen`, `face-corner` and `window` (a captured window on a background).

The five nodes above are required and appear once. `auto-cut` holds all six rules:

| Rule | Properties |
| --- | --- |
| `silence` | `on=<bool> min=<time>`: silences at least `min` long |
| `retake` | `on=<bool>`: inline retakes ("the the first") |
| `misspoken` | `on=<bool>`: misspoken words and self-corrections |
| `pause` | `on=<bool> min=<time>`: pauses mid-sentence at least `min` long |
| `filler` | `on=<bool>`: filler words |
| `idle-screen` | `on=<bool> min=<time>`: screen without change for at least `min` |

The four presets ship in `crates/shot-core/formats/` (`reaction`, `opinion`, `product-tour`, `tutorial`) and are copied into a channel's `formats/` and then into each video. This example is an opinion video with tighter thresholds than the preset:

```kdl
prompter beats
pacing tight
layouts face split content
captions word-pop
auto-cut confidence=80 {
    silence on=#true min=500_000
    retake on=#true
    misspoken on=#true
    pause on=#true min=800_000
    filler on=#true
    idle-screen on=#false min=8_000_000
}
```

### Product video settings

Optional nodes after `auto-cut`, in this order. A missing node means its default, so a `format.kdl` without them reads as before. The cursor is drawn from the event log, so every one of them changes after recording without re-recording.

| Node | Value | Default |
| --- | --- | --- |
| `auto-zoom <trigger>... scale=<percent> hold=<time>` | the auto-zoom rule. Triggers from `click`, `typing` and `cursor`, each at most once; none means the rule is configured but off. It writes `zoom` nodes of `scale` percent (at least 100) and at least `hold` long into `cut.kdl`, each with its trigger as `by` | no rule |
| `zoom-motion` | `instant` or `smooth`: how every zoom of the video moves in, out and between centers | `instant` |
| `cursor size=<percent> smoothing=<time> ripple=<bool>` | the cursor's size as a percent of the recorded size (0 hides it), the window its path is averaged over (0 draws the raw path), and a ripple on every click | `size=100 smoothing=0 ripple=#false` |
| `keystrokes` | the keystroke overlay: `off`, `shortcuts` (keys with a modifier held) or `all` | `off` |
| `window padding=<px> corners=<px> shadow=<shadow> background="#rrggbb" frame=<frame>` | how the `window` layout places the captured window: padding and corner radius in output pixels, a shadow of `none`, `soft` or `hard`, the background color, and a device bezel of `none`, `macbook` or `iphone` | `padding=48 corners=12 shadow=soft background="#101418" frame=none` |

`auto-zoom` is a rule like the auto-cut rules: it writes nodes and changes no pixels by itself. The other four reach the renderer, and each enters a chunk's hash only where it draws, see [timeline evaluation](#timeline-evaluation). This example is the product tour preset:

```kdl
prompter full-script
pacing tight
layouts window
captions clean
auto-cut confidence=85 {
    silence on=#true min=600_000
    retake on=#true
    misspoken on=#true
    pause on=#true min=1_000_000
    filler on=#true
    idle-screen on=#true min=3_000_000
}
auto-zoom click typing scale=160 hold=1_500_000
zoom-motion smooth
cursor size=140 smoothing=120_000 ripple=#true
keystrokes shortcuts
window padding=64 corners=12 shadow=soft background="#141821" frame=none
```

## `cut.kdl`

The edit of the main cut as a list of chapters, followed by the derived cuts. Top-level nodes, in canonical order:

| Node | Meaning |
| --- | --- |
| `captions <style>` | optional, overrides the format's caption style. The main cut ships captions as a sidecar file |
| `caption-fixes { fix "<heard>" "<said>" ... }` | optional. Each `fix` replaces a run of transcript words that reads `heard`, ignoring case, with `said` wherever captions are shown, and the replacement takes the matched words' timing. `heard` is unique ignoring case, neither text is empty. Claude proposes fixes as text and a person edits them in Caption fixes, never through a proposal |
| `music "<file>" gain=<dB> duck=<dB>` | optional. A music bed under the main cut and its derived cuts: a file relative to the video folder, its gain, and the gain it drops to under speech. Both are at most 0 |
| `chapter "<title>" { ... }` | zero or more, in output order |
| `short <name> { ... }` | a 9:16 derived cut, reframed on the face |
| `x-clip <name> { ... }` | a 1:1 derived cut |

A `chapter` holds `take <id> { ... }` blocks in output order. A take may appear in several chapters and several times in one chapter. Inside a take block, all times are that take's media time, in canonical order:

| Node | Meaning |
| --- | --- |
| `keep from=<time> to=<time>` | a range that goes into the output |
| `remove from=<time> to=<time> by=<who>` | a removed range. `by` is the rule that removed it (`silence`, `retake`, `misspoken`, `pause`, `filler`, `idle-screen`) or `manual` |
| `review from=<time> to=<time> by=<rule>` | a finding the rule was not sure about, waiting in the review queue. The material plays until a person decides |
| `ignore from=<time> to=<time> by=<rule>` | a finding a person kept: K in the queue, or a restored removal. The rule leaves this range alone when it runs again |
| `layout <layout> from=<time> to=<time>` | a layout for that range. Elsewhere the format's default layout applies |
| `zoom <percent> from=<time> to=<time> x=<px> y=<px> by=<what>` | a punch-in zoom, at least `100` percent. `x` and `y`, together or not at all, center it on a point of the screen track in its pixels; without them the layout picks the center. `by` names what wrote it: `jump-cut`, `emphasis`, `click`, `typing`, `cursor` or `marker`. A zoom without `by` is a person's |
| `overlay <template> from=<time> to=<time> { <param> <value> ... }` | a template overlay. Each child is one parameter: a name and one text, integer or boolean value. Integers are written grouped by three like times, so a parameter in microseconds reads as one. Later overlays draw on top |

`keep` and `remove` ranges together are in order and do not overlap; gaps between them are allowed. `review` and `ignore` ranges are in order, do not overlap each other, and lie inside kept material. `layout` ranges are in order and do not overlap, and so are `zoom` ranges. Overlays may overlap. Template names and their parameters are defined by the overlay templates, not by this contract.

Auto cut and auto edit write into the take block and leave a trail, so nothing is asked twice:

- A rule's confident finding becomes `remove ... by=<rule>`, an unsure one `review ... by=<rule>`. Cut (C) in the queue turns a `review` into `remove ... by=<rule>`, Keep (K) into `ignore`. Restoring a removal a rule made also writes `ignore`. A manual cut that touches a `review` or `ignore` drops it: the person has decided about that material.
- A rule that runs again, for example after a threshold changed, skips a finding that overlaps a `remove`, `review` or `ignore` of its kind, and still reports new material.
- Jump cuts are not nodes. The pacing places the edges of each `remove`, and the jump is what is left.
- The punch-in zooms the pacing writes carry `by=jump-cut` or `by=emphasis`, auto zoom's carry their trigger, a Z marker's `by=marker`. A zoom rule that runs again replaces the zooms with its own `by` and leaves every other zoom alone.
- Hotkey markers from the [event log](#cursor-and-key-event-log) enter the cut once, when the take is appended: layout markers become `layout` nodes, Z markers `zoom ... by=marker`, B markers b-roll overlays, A markers feed the retake rule and N markers the chapter boundaries.

A derived cut (`short` or `x-clip`) has a unique name and holds an optional `captions <style>` and one or more `clip <take> from=<time> to=<time>` nodes in output order. It uses only material the main cut keeps inside those ranges, with the main cut's layouts, zooms and overlays, and burns its captions in. Its caption style defaults to the main cut's. Every clip's take must appear in the main cut.

```kdl
captions word-pop

caption-fixes {
    fix "lara vel" "Laravel"
    fix "jeep UI" "gpui"
}

music "sources/bed.mp3" gain=-18 duck=-12

chapter "Hook" {
    take t01 {
        keep from=0 to=4_200_000
        remove from=4_200_000 to=5_100_000 by=silence
        keep from=5_100_000 to=12_000_000
        review from=9_800_000 to=10_400_000 by=filler
        zoom 112 from=8_000_000 to=9_500_000 by=emphasis
    }
}

chapter "The rule everyone repeats" {
    take t01 {
        keep from=12_000_000 to=31_400_000
        remove from=31_400_000 to=33_000_000 by=retake
        keep from=33_000_000 to=58_000_000
        ignore from=45_000_000 to=46_100_000 by=pause
        layout split from=20_000_000 to=40_000_000
        overlay link-card from=22_000_000 to=27_000_000 {
            url "https://github.com/coderscantina/shot-studio/pull/1"
            accent #true
        }
    }
    take t02 {
        keep from=0 to=15_000_000
        remove from=15_000_000 to=16_200_000 by=manual
    }
}

chapter "What we do instead" {
    take t02 {
        keep from=16_200_000 to=44_000_000
        remove from=44_000_000 to=45_000_000 by=filler
        keep from=45_000_000 to=70_000_000
        zoom 112 from=50_000_000 to=52_000_000
    }
}

short friday-deploys {
    clip t01 from=5_100_000 to=12_000_000
    clip t02 from=16_200_000 to=44_000_000
}

x-clip the-rule {
    captions clean
    clip t01 from=12_000_000 to=31_400_000
}
```

A product tour needs no layout, cursor or window nodes here, those come from `format.kdl`. Its zooms are centered: two from auto zoom on the click and the typed text, one from a Z marker. Code from git is an overlay template, `code-diff` in this example: the app snapshots the diff of the chosen commit range into `sources/` (a repo and its tags move, a text file in git does not), types it out over the overlay's range, and highlights the line from the moment the voice reaches the phrase. The template owns its parameter names, as every template does.

```kdl
chapter "Search filters" {
    take t01 {
        keep from=0 to=42_000_000
        zoom 160 from=6_200_000 to=8_400_000 x=2210 y=640 by=click
        zoom 160 from=8_400_000 to=11_000_000 x=1480 y=910 by=typing
        zoom 160 from=30_000_000 to=31_500_000 x=1600 y=900 by=marker
        overlay code-diff from=12_000_000 to=26_000_000 {
            diff "sources/query-parse.diff"
            highlight-line 4
            highlight-at 21_300_000
        }
    }
}
```

## Timeline evaluation

`shot-core` evaluates a project into one output for the main cut and one for each derived cut.

- A main output has one group of segments per chapter. A segment is a kept range of one take with one layout, so a kept range splits wherever a layout change starts or ends. It carries the zooms and overlays that overlap it, with their full ranges.
- A derived output has one group, named after the cut. Its segments are the main segments cut down to each clip's range.
- Export caches rendered video in chunks. A segment is snapped to the output frame grid and cut at every half second of its take's media time, which is the recordings' keyframe grid. So a chunk is at most 0.5 s and starts on the grid or at a segment edge.
- Each chunk has a content hash (BLAKE3) over the renderer version, the output aspect, the burned-in caption style with the caption fixes (or the fact that captions are a sidecar), the take id and the take's media identity, the frame rate and the chunk's frames of the take, the layout, the cursor and keystroke settings when the layout shows the screen (every layout but `face`), the window style in the `window` layout, the zooms with their scale and center, and every overlay overlapping the chunk with its full range and parameters. Instant zooms are cut to the chunk; with `zoom-motion smooth` every zoom of the segment counts whole, because the easing at one frame depends on where the zoom starts and ends and where the next one centers. Output position, chapter titles, zoom provenance, review items and the music bed are not part of it. Deleting a word therefore re-renders only the two chunks at the new edges, and every other chunk is reused at its new position. A new caption fix re-renders the derived cuts and nothing of the main cut.
- A take's media identity hashes the size and modification time of its video tracks and of its event log, since the cursor is drawn from events. A take recorded again under a reused id gets a new identity, so it never matches the old recording's chunks.
- Audio is not cached. Export encodes the whole cut's audio as one AAC stream with the music bed mixed under the voice, so the joins have no encoder delay or padding.

## `script.md`

Markdown with a fixed shape, read line by line:

- `# <title>` is the first heading and appears once.
- `## <chapter>` starts a chapter.
- A bullet (`- ` or `* `) inside a chapter is a beat. Every `[label](target)` link in it is a source, usually a file in `sources/`. The prompter shows the bullet with links replaced by their labels.
- Any other text after a beat, indented or not, is that beat's full script for full-script mode. Lines in a paragraph join with spaces, paragraphs with a line break.
- Blank lines separate paragraphs. Other headings (`###` and deeper), text before the first beat of a chapter and beats before the first chapter are errors with a line number.

`shot-core` only reads this file. The script agent writes it as plain text.

```markdown
# Why we deploy on Fridays

## Hook

- Everyone says never deploy on a Friday. We do it every week. [the post](sources/never-friday.md)
  Everyone says never deploy on a Friday.
  We ship every single week, on Friday afternoon.

  And nothing burns.

## The rule everyone repeats

- Where the rule comes from, per [the article](sources/friday-article.md)
- Why it points at a slow rollback, see [the PR](https://github.com/coderscantina/shot-studio/pull/1) and [our notes](sources/rollback-notes.md)

## What we do instead

- Small diffs, feature flags, one-click rollback [notes](sources/rollback-notes.md)
```

## Take folder

`shot-capture` writes one folder per recording session, `media/<take>/`, named by its take id.

```
media/t03/
  screen.mp4    the display without the cursor: HEVC, display resolution, constant 60 fps
  camera.mp4    HEVC at the camera's format, when a camera was recorded
  mic.mp4       AAC 48 kHz mono, when a mic was recorded
  system.mp4    AAC 48 kHz stereo, when system audio was recorded
  events.jsonl  cursor and key events, see below
```

- **Take ids** are `t` and a number, at least two digits: `t01`, `t02`, ... `t100`. A new take gets the number after the highest take folder in `media/`, so an id is never reused while its folder exists, and a gap left by a deleted take is never filled. Deleting the newest take frees its id for the next recording. Cache keys hash the take's media identity as well as its id, so the new recording never reuses the old one's cache.
- **One clock.** Every track and the event log share the take's media time, 0 at session start. A track may begin a little after 0 when its device delivers its first sample late.
- **Fragmented MP4.** Each file is an initialization segment (`ftyp`, `moov`) followed by `moof` and `mdat` fragments of about 0.5 s, appended as they are encoded. Video has a keyframe every 0.5 s and no B-frames. A killed recording keeps every complete fragment and stays playable.
- A track that is missing was not recorded. Readers do not assume a camera or system audio.

## Cursor and key event log

Capture writes `media/<take>/events.jsonl` next to the take's tracks. The cursor is saved as events, not burned in, so zooms, click ripples, cursor size and the keystroke overlay are render settings.

The file is [JSON Lines](https://jsonlines.org): one JSON object per line, UTF-8, sorted by time, written as events happen so a crash keeps everything before it. Fields:

| Field | Type | Meaning |
| --- | --- | --- |
| `t` | integer | media time in microseconds, on the same clock as the take's tracks |
| `kind` | string | `move`, `down`, `up`, `scroll`, `key` or `marker` |
| `x`, `y` | integer | cursor position in pixels of the screen track, origin top left. On every kind except `key` |
| `button` | string | `left`, `right` or `other`. On `down` and `up` |
| `dx`, `dy` | integer | scroll distance in pixels. On `scroll` |
| `key` | string | the key pressed, lowercase (`a`, `space`, `enter`, `tab`, `escape`, `left`, `f5`). On `key` |
| `mods` | array of strings | held modifiers from `cmd`, `shift`, `alt`, `ctrl`, `fn`, in that order. On `key`, may be empty |
| `mark` | string | the record hotkey pressed: `again` (A), `next` (N), `marker` (M), `zoom` (Z), `b-roll` (B) or `layout` (1 to 4). On `marker` |
| `beat` | integer | the beat the prompter shows after the press, numbered from 1 across the script. On `again` and `next` |
| `layout` | string | the layout chosen, one of the format's `layouts`. On `layout` |

Readers ignore fields they don't know, so capture can add fields without breaking them. A new `kind` is a change to this contract.

```json
{"t":0,"kind":"move","x":1920,"y":1080}
{"t":16667,"kind":"move","x":1932,"y":1074}
{"t":512000,"kind":"down","x":2210,"y":640,"button":"left"}
{"t":598000,"kind":"up","x":2210,"y":640,"button":"left"}
{"t":1200000,"kind":"scroll","x":2210,"y":900,"dx":0,"dy":-240}
{"t":2104000,"kind":"key","key":"p","mods":["cmd","shift"]}
{"t":2350000,"kind":"key","key":"enter","mods":[]}
{"t":4210000,"kind":"marker","x":2210,"y":640,"mark":"again","beat":2}
{"t":9800000,"kind":"marker","x":2210,"y":640,"mark":"next","beat":3}
{"t":12000000,"kind":"marker","x":1480,"y":910,"mark":"zoom"}
{"t":15500000,"kind":"marker","x":1480,"y":910,"mark":"layout","layout":"split"}
```

Markers are what the record hotkeys write. They are facts about the recording, so they live with it and carry the cursor position at the press, which is where "zoom here" points. They are not edit state: the rough cut turns them into `cut.kdl` nodes once, when the take is appended (see [`cut.kdl`](#cutkdl)), and milestone 07 covers `b-roll` markers with b-roll. A take folder without markers is a take recorded without the prompter.

`shot-board` implements these. It writes through refs and trees only, so a checked-out video branch's working tree goes stale after a board write.

- **Video branch**: `video/<slug>`. A card's state is read from its tip, falling back to the default branch for ideas without a branch yet. Moving a card is one commit on this branch, subject `🚚 move <slug> to <status>`.
- **Proposal branch**: `proposal/<slug>/<name>` off `video/<slug>`, holding commits that change the video's files. Paths are relative to the video folder. The diff is against the merge base with the video branch. Accept merges (fast-forward when possible), reject deletes the branch, and a conflicting accept changes nothing and lists the paths. See [0004](../decisions/0004-proposals-as-branches.md).
- **Release tag**: annotated CalVer `vYY.M.D` of the release date (`v26.11.7`), on the tip of a video branch whose status is `shipped`. A later release on the same day gets `.2`, `.3` and so on. Tags sort by their numeric parts. Message `🚀 ship <slug>`.
