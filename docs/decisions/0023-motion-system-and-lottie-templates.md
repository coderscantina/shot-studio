# 0023: A motion system with springs, materials and Lottie templates

Status: Proposed. Settled with Mike in the production grill on 2026-11-07. Amends [0018](0018-overlay-templates-as-typed-specs.md) and [0021](0021-sound-transitions-and-bookends.md).

## Problem

Overlays animate through one presence value per template, eased by a single smoothstep curve: cards slide and fade, word pop switches hard. The compose shader has only the 32 px block blur used for redaction, with no gaussian or backdrop blur, no motion blur, no masks or perspective, and shadows only for rounded rectangles. The bar is production-grade motion graphics, not plain fades, and designers need a way to hand over brand animations.

## Constraints

- Preview equals export, frame by frame, and any frame renders without the frames before it (seeking, chunk caching).
- Full 4K export stays faster than realtime (96 fps with every overlay at milestone 07), and the preview keeps at most one dropped frame per 5 minutes.
- Templates are typed specs agents fill with parameters ([0018](0018-overlay-templates-as-typed-specs.md)), styles are data ([0019](0019-semantic-video-elements-and-style.md)).
- vello is pinned at 0.11. velato 0.12, the Lottie renderer for vello, needs vello 0.10 and renders a subset of After Effects (no expressions, no AE effects such as glow).

## Choice

- **Four motion presets**, one per format: reactive (reaction: fast springs, pops, stickers), editorial (opinion: bold kinetic type, mask reveals), launch (product tour: glass, depth, slow springs), calm (tutorial: legibility first, minimal motion). The brand tunes each with named feel controls (tempo, bounce, density, stagger) as integers. Raw spring values stay inside the presets.
- **Springs with choreography** for every move, solved in closed form over media time with no simulation state. Children stagger, exits run faster than entrances. Zoom moves use the same springs; `zoom-motion smooth` becomes the spring. `enter` and `exit` keep working as duration hints, so existing overlays render.
- **Materials**: a gaussian blur pass in the compose shader for frosted glass behind cards and soft shadows of any shape; perspective tilt, parallax and shadows that agree with one light; adaptive motion blur on the overlay layer and zoom moves, from 1 sample when still to 8 on fast moves, by the same rule in preview and export.
- **Kinetic type**: per-word and per-glyph springs from parley's glyph positions, and mask reveals. Word pop captions scale and settle.
- **Sound per motion phase**: travel, impact or settle, and exit can each carry a brand sound, timed to the frame the spring settles. This refines 0021's one sound per effect kind.
- **Beat sync**: while a music cue plays, entrances, pops and stingers snap to the nearest beat within about 150 ms, switched per preset. Beats are detected once per music file by a deterministic algorithm and cached by the file's hash. Cuts never move.
- **Transitions**: at chapter edges, a brand stinger (a Lottie covering the cut at its peak), a mask wipe in the brand's shape and a zoom-through join 0021's cut, dissolve and dip. Clip joins in derived cuts stay hard.
- **Lottie now**, through velato patched to vello 0.11 and pinned in `sources/manifest.json`. A brand may use Lottie for stings and logo reveals, to re-skin a built-in template through named slots, for animated emoji and stickers (a curated Noto animated emoji subset, CC BY 4.0, bundled with attribution), and for new templates.
- **Lottie templates are declared**: `brand.kdl` names each one's file and typed slots (text, color, integer, time). The declaration becomes its `TemplateSpec`, checked at export like any template. The file's content hash enters the lock and the chunk hash, so the file is its own version. Text slots take position and motion from the Lottie layer and are drawn by parley in the brand's fonts. Code templates (`highlight`, `b-roll`, `code-diff`, `blur`, `bleep`) stay code.
- **Import check**: adding a Lottie to the brand refuses a file that uses anything velato cannot render and lists every unsupported feature and layer.
- **The gate** is a rendered reel per preset with every template, transition and kinetic type moment at 4K60 over the fixture project. Mike scores it; under 8 is reworked. Scores are recorded in [design](../design.md).

## Rejected alternatives

- **Curated easing curves**: reads as animated, not physical.
- **Per-property keyframe timelines**: the briefing's out-of-scope "keyframes on every property".
- **One preset, or two**: Mike wants a production reference per format.
- **Fixed 8 motion blur samples**: may push full 4K export below realtime. **Analytic blur**: wrong on text and complex shapes.
- **Alpha video for brand assets**: renders everything After Effects can, but cannot fill text and color slots; Mike chose Lottie now.
- **Slots read from layer names**: a renamed layer silently breaks a template. **Best-effort Lottie rendering**: an export could differ from the design without anyone noticing.
- **A measurable checklist as the gate**: Mike judges by the reel.

## Consequences

- 0018's rejection of templates as data files gives way for declared, hashed Lottie templates. Its other rules hold: typed specs, unknown templates and parameters refused, specs grow only by optional parameters.
- The briefing's "a library of transitions" stays out: a channel has its own small set, not a catalog.
- `shot-render` gets the spring solver, the blur pass, motion blur sampling, perspective, glyph-level text and the velato renderer. `shot-audio` gets beat detection. The app's brand screen gets the Lottie import check.
- The 07 templates are rebuilt on the motion system before new templates are added.
- Motion blur and glass cost render time, measured against the export and preview targets in milestone 11.

## Affected contracts

[Semantic video draft](../contracts/semantic-video.md), [overlay templates](../contracts/project-files.md#overlay-templates), `shot-render`, `shot-audio`, `shot-app`, `sources/manifest.json`.
