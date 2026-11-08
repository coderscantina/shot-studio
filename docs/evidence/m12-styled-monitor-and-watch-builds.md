# Milestone 12: styled monitor and watch builds

Report for [milestone 12](../milestones/12-styled-monitor-and-watch-builds.md). Screenshots are in `docs/evidence/m12/` (git-ignored).

## Watch builds and proposal previews (items 3 and 4)

Branch `task/m12-watch`, details in [m12/watch.md](m12/watch.md). Measured 2026-11-08 on the reference machine with two other agents working.

| Criterion | Result |
| --- | --- |
| `shot build --watch` re-exports one changed chapter of the 17 min fixture in under 20 s after a save | **PASS, 7.8 to 13.5 s** from the write to `built` in 5 of 6 saves on `task/m12-audioperf` (before: 29.8 to 32.8 s), 11.5 s for a save during a check, which now stops the check. One save took 65.1 s with a 9.7 s build, cause not found. The audio finishes in about 6 s instead of 26 s and the join copies the video meanwhile. Details in [m12/audio-speed.md](m12/audio-speed.md) |
| A proposal preview plays without changing the video branch's lock or cache | **PASS**, `cut.lock` and the chunk folder listing have the same checksums before and after building the proposal (2.2 s, 4 chunks in its own space) and after playing it in the app |

- `shot build --video <dir>` rebuilds the lock, exports every cut, prints one `built` line and then only the problems `shot check` finds. `--watch` rebuilds on changes to the video's files, its takes and the channel's style layers (FSEvents through `notify`), once per save.
- `shot build --proposal <name>` builds the proposal's lock and changed chunks in `.shot/cache/proposals/<name>/`, reading every other chunk from the branch's cache.
- The Edit screen's proposal card has Play: the preview plays the proposal's plan and sound, Esc returns to the branch.

Screenshot of the Edit screen playing a proposal: `m12/watch-proposal-playing.png`.

The audio finish and the check were then sped up on `task/m12-audioperf` ([m12/audio-speed.md](m12/audio-speed.md)): an export with every chunk cached takes 6.5 to 7.4 s instead of 32 to 35 s, and a save during `shot check` stops it. The check takes 4.6 s on one 6.4 GB export but still about 40 s on another, open for shot-media.
