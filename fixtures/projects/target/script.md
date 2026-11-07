# A full recording session from first take to export

## Intro

- Welcome, and what this video covers: first take to finished export
- The goal: the editor does the boring parts
- The plan: set up, record, auto cut, review, export
- Nothing needs a cloud service, the project is a plain folder in git

## Install and setup

- First launch: speech model download with progress and checksum
- Screen and microphone permissions
- Pick a channel folder, which is just a git repository
- The same commands are available on the command line and to agents

## Project structure

- A video is a folder: script, project file, cut file, media directory outside git
- The script is plain markdown
- The cut file records every editing decision, so it can be reviewed like a pull request
- Git merges these files when two people work on one video

## Recording a take

- Three separate tracks: screen, camera and microphone
- Fragmented files survive a crash
- Markers for a good line, a mistake or a diagram
- Say a stumbled sentence again, the editor keeps the better take
- Press stop and the rough cut is ready in seconds

## Auto cut

- It works like a linter for the recording
- Silence: short pauses stay, long ones are trimmed to the format's pacing
- Filler words and repeated starts are marked, the last complete attempt wins
- Every change is a suggestion with a reason
- Nothing is destructive, the original recording is never touched

## Review and notes

- The editor reads the transcript against the script and writes notes
- A note points at a time in the timeline
- Fix the cut or dismiss the note with a comment
- Open notes block the export

## Export

- Export is a build: chapters are rendered and cached on their own
- Only a changed chapter is rendered again
- Hardware video engines, loudness check at the end
- Full video, short clip or audio only

## Wrap up

- The whole workflow in one sentence
- The project is open source and the test media is generated
- Thanks for watching
