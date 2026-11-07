# Project files contract

This contract owns the files a video consists of and what each one means. [Milestone 01](../milestones/01-contracts-and-fixtures.md) writes the grammar, examples and parse tests into this document. Until then, this lists what is agreed.

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

## Proposals

An agent proposal is a branch `proposal/<video>/<slug>` off the video's branch, holding a commit that changes these files. Accept merges it, reject deletes it. See [0004](../decisions/0004-proposals-as-branches.md).
