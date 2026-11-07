# 0019: Semantic video, elements and style rules

Status: Proposed. Settled with Mike in the effects grill on 2026-11-07. Supersedes [0013](0013-review-queue-in-cut-kdl.md) and [0014](0014-auto-edits-as-written-nodes.md), and moves [0016](0016-product-effects-as-format-settings.md)'s settings into style rules.

## Problem

Every automatic effect so far responds to something the video means: a zoom to a click, a link card to a URL said, a cut to a silence, a chapter card to a chapter. 0014 writes each response as a node in `cut.kdl`, so a person edits instances. That scales badly once chapter cards, SFX, transitions, music cues, pops and callouts join: a rebrand or a pacing change means rewriting hundreds of nodes, and the file mixes what was said with how it looks.

## Constraints

- The edit stays honest: what the export shows is in a file, readable in a diff (0014).
- A person can change or delete any single instance.
- An edit elsewhere never moves an instance (0014's "every other jump cut" problem).
- Builds are reproducible: the same files give the same pixels, whatever model version is installed.
- Rough cut within 5 s of stop, preview equals export.

## Choice

Shot Studio is a semantic video editor: it turns a recording into a document of meaning, and cascading style rules render that document into every cut.

- **Elements** are the facts of a take: words, sentences, silences, fillers, retakes, emphasis, URLs and code said, clicks, typing, idle screen, faces and expressions, script beats and chapters (local analysis); claim, punchline, aside, topic shift, product mention, needs a source (Claude, from the transcript only, per [0005](0005-privacy-text-only-to-claude.md)); labeled record markers; and tags written in `script.md`.
- Elements are **committed per take** in `elements/<take>.jsonl` in the video folder. Builds read the file and never re-analyse. Re-analysis is an explicit command whose diff a person reviews.
- An element's **identity** is its kind, take and media time range. A re-cut never moves it. After re-analysis an override follows the nearest element of its kind, and `shot check` lists overrides that lost theirs.
- **Content and style**: a cut a person makes (`remove by=manual`) is content. Everything else is style: pacing (which silences, fillers and retakes go), layouts, zooms, cards, overlays, SFX, transitions, music and captions.
- **Style rules** are KDL over a closed set of selectors (element kinds and attributes) and a closed set of effects, as strict as the other files. They cascade: channel brand, then the video's format, then the video, then an override on one element.
- **The lock**: a build writes `cut.lock`, a committed, generated file listing every instance with the element and rule that made it, in a stable order. Git diffs of the lock show what the export shows. Nobody edits it by hand.
- **Review**: findings are elements with a confidence. The pacing style removes those at or above its threshold, the queue lists the unsure ones, and K or C writes a keep or cut override on the element.
- **Shipped videos** freeze their lock and resolved style at the release tag. A rule change reaches unshipped videos only, and `shot restyle <slug>` rebuilds a shipped one on purpose as a new release.
- **Edit scope**: every change to a look in the app offers a scope, only here (an override, the default), this video, this format or the channel, and every instance shows the rule that made it.

## Rejected alternatives

- **Written nodes with smarter rules** (0014 as is): every new effect adds nodes, and a rebrand or a pacing switch rewrites them all.
- **Hybrid**, cuts and zooms written, decorations styled: two models for one idea, and pacing is exactly where styles pay off most.
- **Elements in the cache**: a new whisper model or a second Claude run could re-cut a video without anyone noticing.
- **Elements folded into the lock**: mixes facts with results, so a rule change and a re-analysis produce one tangled diff.
- **Text anchors for identity**: survives re-transcription better, breaks when a phrase repeats.
- **Selectors with conditions** (`every=2`, `not`): brings 0014's ordinal problem back. Start closed and grow on evidence.

## Consequences

- `cut.kdl` shrinks to chapters, takes, manual cuts, overrides and derived cuts. The `review`, `ignore`, rule-made `remove`, `zoom ... by=` and rule-made `overlay` nodes move into the lock, and existing files migrate once.
- `shot-analyze` writes elements instead of nodes. `shot-render`'s auto-zoom and overlay rules and `shot-analyze`'s auto-cut rules become style rules in a new style engine in `shot-core` (pure, no IO, like timeline evaluation).
- The renderer and the chunk hash read resolved instances from the lock, so caching works as before.
- The app gets provenance on every instance, the scope choice on every change and a rule editor.

## Affected contracts

[Semantic video draft](../contracts/semantic-video.md), then [project files](../contracts/project-files.md) when milestone 10 merges it. `shot-core`, `shot-analyze`, `shot-render`, `shot-app`, `shot-cli`, `shot-mcp`, `shot-ship`.
