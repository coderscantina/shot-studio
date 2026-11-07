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
      .shot/cache/      SQLite and rendered segments (ignored, safe to delete)
  formats/              the four shipped presets, editable per channel
```

## What each file owns

- **`video.kdl`**: the card on the board. Status is one of `ideas`, `scripting`, `ready`, `recorded`, `editing`, `shipped`. Moving a card rewrites this field and commits.
- **`script.md`**: the spec. `#` is the title, `##` a chapter, a bullet a beat. Beats link to a source in `sources/`. The prompter reads this file.
- **`cut.kdl`**: the edit. It references recordings by take id and media time, never by frame index of an output. It holds:
  - kept ranges per take, with the rule or person that decided each removal
  - layout changes, zooms, captions style, overlays (template name plus parameters)
  - derived cuts (`short`, `x-clip`) as a few lines on top of the main cut
- **`format.kdl`**: pacing, default layouts, caption style, auto-cut thresholds, prompter mode.

## Rules every module follows

- Every UI action that changes the edit becomes a change to one of these files. There is no hidden edit state.
- Files are written in a stable order so a diff shows only what changed.
- `shot-core` parses and evaluates these files with no IO. Unknown nodes are an error with a line number, never silently dropped.
- The cache is derived from these files and media only. Deleting `.shot/cache` loses nothing.
- A segment's cache key is a hash of everything that affects its pixels and samples: source media ids, ranges, effects, template parameters, format settings and renderer version.

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

Every node is required and appears once. `auto-cut` holds all six rules:

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

## `cut.kdl`

The edit of the main cut as a list of chapters, followed by the derived cuts. Top-level nodes, in canonical order:

| Node | Meaning |
| --- | --- |
| `captions <style>` | optional, overrides the format's caption style. The main cut ships captions as a sidecar file |
| `chapter "<title>" { ... }` | zero or more, in output order |
| `short <name> { ... }` | a 9:16 derived cut, reframed on the face |
| `x-clip <name> { ... }` | a 1:1 derived cut |

A `chapter` holds `take <id> { ... }` blocks in output order. A take may appear in several chapters and several times in one chapter. Inside a take block, all times are that take's media time, in canonical order:

| Node | Meaning |
| --- | --- |
| `keep from=<time> to=<time>` | a range that goes into the output |
| `remove from=<time> to=<time> by=<who>` | a removed range. `by` is the rule that removed it (`silence`, `retake`, `misspoken`, `pause`, `filler`, `idle-screen`) or `manual` |
| `layout <layout> from=<time> to=<time>` | a layout for that range. Elsewhere the format's default layout applies |
| `zoom <percent> from=<time> to=<time>` | a punch-in zoom, at least `100` percent |
| `overlay <template> from=<time> to=<time> { <param> <value> ... }` | a template overlay. Each child is one parameter: a name and one text, integer or boolean value. Later overlays draw on top |

`keep` and `remove` ranges together are in order and do not overlap; gaps between them are allowed. `layout` ranges are in order and do not overlap, and so are `zoom` ranges. Overlays may overlap. Template names and their parameters are defined by the overlay templates, not by this contract.

A derived cut (`short` or `x-clip`) has a unique name and holds an optional `captions <style>` and one or more `clip <take> from=<time> to=<time>` nodes in output order. It uses only material the main cut keeps inside those ranges, with the main cut's layouts, zooms and overlays, and burns its captions in. Its caption style defaults to the main cut's. Every clip's take must appear in the main cut.

```kdl
captions word-pop

chapter "Hook" {
    take t01 {
        keep from=0 to=4_200_000
        remove from=4_200_000 to=5_100_000 by=silence
        keep from=5_100_000 to=12_000_000
        zoom 112 from=8_000_000 to=9_500_000
    }
}

chapter "The rule everyone repeats" {
    take t01 {
        keep from=12_000_000 to=31_400_000
        remove from=31_400_000 to=33_000_000 by=retake
        keep from=33_000_000 to=58_000_000
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

## Timeline evaluation

`shot-core` evaluates a project into one output for the main cut and one for each derived cut.

- A main output has one group of segments per chapter. A segment is a kept range of one take with one layout, so a kept range splits wherever a layout change starts or ends. It carries the zooms and overlays that overlap it, with their full ranges.
- A derived output has one group, named after the cut. Its segments are the main segments cut down to each clip's range.
- Each segment has a content hash (BLAKE3) over the renderer version, the output aspect, the burned-in caption style (or the fact that captions are a sidecar), the take id, the source range, the layout, and every overlapping zoom and overlay with its parameters. Output position and chapter titles are not part of it, so editing one chapter only changes that chapter's hashes.

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

## Cursor and key event log

Capture writes `media/<take>/events.jsonl` next to the take's tracks. The cursor is saved as events, not burned in, so zooms, click ripples, cursor size and the keystroke overlay are render settings.

The file is [JSON Lines](https://jsonlines.org): one JSON object per line, UTF-8, sorted by time, written as events happen so a crash keeps everything before it. Fields:

| Field | Type | Meaning |
| --- | --- | --- |
| `t` | integer | media time in microseconds, on the same clock as the take's tracks |
| `kind` | string | `move`, `down`, `up`, `scroll` or `key` |
| `x`, `y` | integer | cursor position in pixels of the screen track, origin top left. On every kind except `key` |
| `button` | string | `left`, `right` or `other`. On `down` and `up` |
| `dx`, `dy` | integer | scroll distance in pixels. On `scroll` |
| `key` | string | the key pressed, lowercase (`a`, `enter`, `tab`, `escape`, `left`, `f5`). On `key` |
| `mods` | array of strings | held modifiers from `cmd`, `shift`, `alt`, `ctrl`, `fn`, in that order. On `key`, may be empty |

Readers ignore fields they don't know, so capture can add fields without breaking them. A new `kind` is a change to this contract.

```json
{"t":0,"kind":"move","x":1920,"y":1080}
{"t":16667,"kind":"move","x":1932,"y":1074}
{"t":512000,"kind":"down","x":2210,"y":640,"button":"left"}
{"t":598000,"kind":"up","x":2210,"y":640,"button":"left"}
{"t":1200000,"kind":"scroll","x":2210,"y":900,"dx":0,"dy":-240}
{"t":2104000,"kind":"key","key":"p","mods":["cmd","shift"]}
{"t":2350000,"kind":"key","key":"enter","mods":[]}
```

## Proposals

An agent proposal is a branch `proposal/<video>/<slug>` off the video's branch, holding a commit that changes these files. Accept merges it, reject deletes it. See [0004](../decisions/0004-proposals-as-branches.md).
