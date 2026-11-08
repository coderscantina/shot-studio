# Milestone 12: styled monitor and watch builds

Report for [milestone 12](../milestones/12-styled-monitor-and-watch-builds.md). Screenshots are in `docs/evidence/m12/` (git-ignored).

## Watch builds and proposal previews (items 3 and 4)

Branch `task/m12-watch`, details in [m12/watch.md](m12/watch.md). Measured 2026-11-08 on the reference machine with two other agents working.

| Criterion | Result |
| --- | --- |
| `shot build --watch` re-exports one changed chapter of the 17 min fixture in under 20 s after a save | **FAIL, 31.4 to 32.8 s** from the write to `built`. The save is seen in under 1.5 s and 2 chunks render, but finishing the whole cut's audio (true-peak limiter and loudness passes) now takes about 22 s of every export, up from 2.2 s in milestone 03 |
| A proposal preview plays without changing the video branch's lock or cache | **PASS**, `cut.lock` and the chunk folder listing have the same checksums before and after building the proposal (2.2 s, 4 chunks in its own space) and after playing it in the app |

- `shot build --video <dir>` rebuilds the lock, exports every cut, prints one `built` line and then only the problems `shot check` finds. `--watch` rebuilds on changes to the video's files, its takes and the channel's style layers (FSEvents through `notify`), once per save.
- `shot build --proposal <name>` builds the proposal's lock and changed chunks in `.shot/cache/proposals/<name>/`, reading every other chunk from the branch's cache.
- The Edit screen's proposal card has Play: the preview plays the proposal's plan and sound, Esc returns to the branch.

Screenshot of the Edit screen playing a proposal: `m12/watch-proposal-playing.png`.

Open for the audio owner: the audio finish has to drop under about 12 s on the 17 min cut for the 20 s target. `shot check` takes about 43 s on the 5.8 GB export, which delays a save that arrives during it.
