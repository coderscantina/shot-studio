# 0015: Caption fixes are a per-video dictionary

Status: Adopted by Mike on 2026-11-08.

## Problem

Whisper mishears names (Laravel, werkstatt, gpui). Claude proposes fixes as text and a person edits them in Caption fixes, never through a proposal. The transcript is cache, so the fixes need a home in git, and burned-in captions need them in the chunk hash.

## Constraints

- Fixes survive a re-transcription, whose word timings shift a little.
- A fix is one line a person can read and a diff can show.
- Only chunks whose pixels change re-render.

## Choice

A `caption-fixes` block in `cut.kdl` with one `fix "<heard>" "<said>"` per entry. A fix replaces every run of transcript words reading `heard`, ignoring case, with `said`, and the replacement takes the matched words' timing. `heard` is unique ignoring case.

The fixes hash into every chunk of an output that burns captions in (derived cuts with a style other than `off`). The main cut ships a sidecar, so its chunks ignore them.

## Rejected alternatives

- Per-occurrence fixes addressed by take and time: break on re-transcription and repeat the same name ten times.
- Fixes applied to the cached transcript: lost with the cache and invisible in git.
- Hashing only the fixes that match words in a chunk: `shot-core` has no transcript at hash time; hashing all fixes re-renders a Short in seconds and keeps the core IO-free.

## Consequences

Captions in `shot-render` apply the fixes when drawing; the sidecar writer applies them too. A channel-wide dictionary is a later step and would copy the same node shape.

## Affected contracts

[Project files](../contracts/project-files.md#cutkdl), `shot-render` captions, the Claude bridge.
