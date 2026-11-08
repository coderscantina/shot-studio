# 0022: Ship artifacts live in `.shot/`, the release tag is what git keeps

Status: Adopted by Mike on 2026-11-08.

## Problem

`shot ship` reads and writes things that are not project files: the exports, the picked thumbnail, the title and description it uploads, and what it already uploaded (the YouTube video id of each cut, the resumable session of an unfinished upload). Until now the thumbnail went to `.shot/export/main.png` without a contract line, and nothing recorded an upload.

## Constraints

- Every tracked file is parsed by `shot-core`, and its types are settled for this milestone.
- `shot-board` writes through refs only. Outside proposals it has no call that commits an arbitrary file, and its release tag message is parsed as `🚀 ship <slug>`.
- An upload writes its session as it goes and its video id at the end. A commit per step would bury the video's history.
- Re-running `shot ship` after a failure must continue, never upload a cut twice.

## Choice

- Exports, the caption sidecar and the thumbnail sit side by side in `.shot/export/`: `<cut>.mp4`, `main.vtt`, `main.png` (or `.jpg`). The thumbnail belongs to the main cut only.
- The title and description are not stored. They are drafted from the project files on every run (`shot describe`), so the chapters always match the cut that is uploaded. To change them, change `video.kdl`, `script.md` or the chapter titles.
- The upload record is `.shot/ship/record.json`: per cut, the video id once finished, or the session and file size of an unfinished upload, the privacy and whether the thumbnail was set; and the release tag once made. A cut with a video id is skipped.
- In git, a ship is the card in `shipped` and the annotated CalVer tag on that commit.

## Rejected alternatives

- A tracked `ship.kdl` with the video ids: needs a new `shot-core` type and a board call that commits a file. Worth it once the board should link to YouTube, see consequences.
- Video ids in the tag message: breaks how `shot-board` reads the slug from it.
- A stored description file: a second place for chapters that drifts from the cut.

## Consequences

- The video ids are local to the machine that shipped. A fresh clone knows a video shipped (the tag) but not its YouTube id, and `shot ship` there would upload again. If that matters, the follow-up is `ship.kdl` committed with the tag through a new `shot-board` call.
- Deleting `.shot/ship/record.json` makes the next run upload every cut again.

## Affected contracts

[Project files](../contracts/project-files.md#ship-artifacts), `shot-ship`, `shot-cli`, the Ship screen.
