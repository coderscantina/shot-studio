# Milestone 07: Overlays and derived cuts

## Outcome

Reactions get link cards, on-screen highlights and b-roll without leaving the app, and one recording yields the main 16:9 video, a 9:16 Short reframed on the face and a 1:1 X clip, each a few lines in `cut.kdl` that reuse the main video's rendered segments.

## Dependencies and ownership

Requires 04, and 05 for the `B` and `Z` markers. Media owns vello templates in `shot-render`. Analysis owns OCR and faces in `shot-analyze`. Core owns derived cut nodes in the contract and `shot-core`.

## Work

1. Typed overlay templates in vello + parley with parameters and keyframes. The template spec is independent of the renderer version.
2. Link cards from URLs said in the transcript: GitHub PR, repo, tweet, HN, npm.
3. Highlights matched to the phrase you react to, using OCR on the source.
4. B-roll from sources, git diffs, screen recordings and a library, covering idle stretches over 8 s and every `B` marker.
5. Bleep and blur from the transcript.
6. Derived cuts in `cut.kdl`: Shorts at 9:16 reframed on the face with Apple Vision, 1:1 X clip, burned-in captions. Shorts picks go through Claude as text.

## Acceptance criteria

- A derived Short of the 17 min fixture exports and reuses cached segments from main where pixels match.
- A link card appears at the word where its URL is said.
- Face reframing keeps the face inside the 9:16 frame on the camera fixture.
- Changing a template parameter re-renders only the segments that use it.

## Evidence and handoff

Report in `docs/evidence/m07-overlays-and-derived-cuts.md` with cache hit counts, timings and screenshots in `docs/evidence/m07/`. Consumed by 08.
