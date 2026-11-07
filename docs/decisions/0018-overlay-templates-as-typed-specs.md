# 0018: Overlay templates are typed specs in the renderer

Status: Proposed

## Problem

`cut.kdl` holds overlays as a template name and free parameters. Milestone 07 adds link cards, highlights, b-roll, blur and bleep, and asks for typed templates with parameters and keyframes whose spec is independent of the renderer version. Someone has to own the parameter names and kinds, decide what a wrong overlay does, and say how files an overlay names reach the cache key.

## Constraints

- `shot-core` parses any overlay; the contract leaves names and parameters to the templates.
- Parameters are text, integers or booleans; no floats.
- Changing a template parameter re-renders only the chunks that show the overlay.
- Automatic rules (link cards, highlights, b-roll slots) write overlays into `cut.kdl` like auto zoom writes zooms ([0014](0014-auto-edits-as-written-nodes.md)), and must not undo a person's changes when they run again.

## Choice

- Each template is a `TemplateSpec` in `shot_render::overlay`: name, parameters with kind and whether required, and whether it animates. Export checks every overlay of the output against its spec and stops with the template and parameter at fault. The inspector reads the same specs.
- Keyframes: every animated template takes `enter` and `exit` in microseconds and draws through `Keyframes`, eased values over media time. `blur` and `bleep` switch hard, since a fade would reveal what they hide.
- A spec only grows by optional parameters. Renderer changes bump `RENDERER_VERSION`, specs stay, so an old `cut.kdl` reads and renders the same way.
- Templates in the first release: `link-card`, `lower-third`, `highlight`, `b-roll`, `code-diff`, `blur`, `bleep`. Link cards render from the URL alone, offline.
- A file an overlay names (`b-roll`, `code-diff`) adds its size and modification time to the key of every chunk the overlay overlaps.
- Rules skip what an overlay of their template already covers. B-roll rules write empty slots (`b-roll` without `file`) that draw nothing until a person picks a still, clip or diff.
- `b-roll` clips replace the frame's screen layer with the clip (a cutaway), so they need no new source in the render graph. `blur` is a second layer of the screen averaged over blocks in the compose shader. `bleep` silences the voice before the voice chain and adds the tone after it.

## Rejected alternatives

- **Templates as data files** (a template language in `formats/`): more flexible, but every template here needs code (OCR boxes, cutaways, audio), and a second language would need its own versioning.
- **Silently skipping unknown overlays**: a typo would ship a video without the overlay.
- **Per-template versions in the hash**: needs `shot-core` to know templates; a renderer version bump is rare and costs one re-render.
- **Fetching link titles from the network**: not reproducible, slow, and a privacy question; the URL alone gives a good card.

## Consequences

- A `bleep` overlay enters the chunk hash like any overlay, so it re-renders the video chunks it overlaps although it changes only audio.
- An empty b-roll slot exports as the frame underneath; the app lists it as a slot to fill.
- Video b-roll draws no take content under it, so a cutaway cannot fade.

## Affected contracts

[Project files](../contracts/project-files.md#overlay-templates), `shot-render` (`overlay`, `overlay_rules`, `frame`, `export`, `audio`, `voice`), `shot-cli` (`shot overlays`), `shot-app` (Overlays panel).
