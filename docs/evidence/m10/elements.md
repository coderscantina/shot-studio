# Milestone 10 item 2: elements

Branch `task/m10-elements`. Analysis writes `elements/<take>.jsonl` while recording and after stop, and `shot analyze` rewrites it on request. Measured on the reference machine while two other agents built in the same target dir, so timings may read a little high.

## What analysis writes

`shot_analyze::elements` maps what milestones 03 to 07 detect to the contract's kinds. The detectors are the existing ones: the auto-cut rules' findings and gaps, the emphasis detection of the punch-in zooms, the idle screen scan, `again_markers`, capture's `read_markers`, and `vision::screen_text` and `face_track` with their caches.

| Kind | From | `by` |
| --- | --- | --- |
| `word`, `sentence` | the transcript, a sentence up to `.`, `?` or `!` | `whisper@large-v3-turbo` |
| `filler` (with `text`), `retake`, `misspoken` | the auto-cut rules with every finding on, confidences as in milestone 04 | `whisper@large-v3-turbo` |
| `silence`, `pause` | every unvoiced gap of at least 300 ms, whole, not trimmed by a breath. A pause's confidence grows past 1 s, the presets' pause `min` | `vad@silero-6.2` |
| `emphasis` | words 6 dB louder than the median word, to the end of the clause, at most 3 s | `prosody@1` |
| `url` (with `url`, `host`) | URLs said, written or spoken | `transcript` |
| `idle` | still screen of at least 3 s, the lowest preset `min` | `screen@1` |
| `click`, `typing` | `down` events; three or more keys less than 1 s apart. What was typed is left out, it may be a password | `events` |
| `marker` | the M, Z and B hotkeys | `events` |
| `beat`, `chapter`, `tag` | beat 1 at 0 and each beat an A or N press moved to; a chapter at its first beat; each `{name}` of a beat until the next beat shows | `script` |
| `ocr` | a phrase of three or more words said while it shows on screen, its box in screen pixels | `vision@1` |
| `face` | the largest face every 0.5 s, its box in camera pixels | `vision@1` |
| `claim`, `punchline`, `aside`, `needs-source`, `topic-shift`, `product` | Claude's reading of the numbered sentences | `claude` |

## When it runs

- **While recording** the app appends each word the transcriber settles to the take's file (`elements::Log`), one line per word, so a crash keeps them. Reading drops a torn last line.
- **At stop**, before the rough cut, the quick pass (everything but Vision and Claude) replaces the file atomically.
- **After the rough cut** the app starts `shot analyze --video <dir> --take <id>` in its own process, with `--claude` when Claude Code is installed. It adds `ocr` and `face` and Claude's elements. Nothing waits on it.
- **`shot analyze --video <dir> [--take <id>] [--claude]`** runs everything again from the caches (or transcribes the mic track when they are missing) and rewrites the file. Without `--claude` it keeps the `by=claude` elements already in the file, so the same inputs give the same bytes. With it, Claude's earlier answer is replaced.

Claude gets the transcript as numbered sentences in the prompt through the existing bridge (text only, [0005](../../decisions/0005-privacy-text-only-to-claude.md)) and answers JSON lines; lines that are not a valid mark are skipped.

## 17 min target take

A copy of `target-project` (media symlinked), transcribed once with `shot autocut`.

| Kind | Count |
| --- | ---: |
| word | 983 |
| sentence | 75 |
| silence | 9 |
| filler | 4 |
| click | 510 |
| typing | 170 |
| **total** | **1751** |

Every other kind is 0 on this fixture, for reasons of the fixture: its speech has no mid-sentence gap of 300 ms or more, no repeated words or corrections and a flat level (no emphasis); it says no URL; the screen never stops moving (as in milestone 04); Vision reads no text on the generated screen and finds no face in the flat-shaded camera face (as in milestone 07); the events log has no markers, so there are no beats, chapters or tags. The unit tests cover each of those kinds on generated input. Claude was not run on the fixture.

| Step | Time |
| --- | --- |
| Quick pass at stop (all local elements, screen.mp4 header scan included) | 10 ms warm, 264 ms with screen.mp4 cold |
| Writing the file (1751 lines) | 2.5 ms |
| `shot_core::build` and writing the lock, example channel rules, 189 instances | 0.2 ms |
| Full pass, cold Vision caches | 114 s: faces 2042 samples about 106 s, OCR 75 samples 7.7 s |
| Full pass, cached | 0.24 s |

So the elements add about 15 ms between the transcriber's last flush and the rough cut; with milestone 04's 0.08 s flush on this take, the rough cut including style evaluation stays far inside 5 s of stop. Two `shot analyze` runs wrote byte-identical files (`cmp`).

## Tests

- `elements::tests`: transcript facts become words, sentences, whole gaps, fillers with text, retakes and URLs; the event log and the prompter give clicks, typing, markers, beats, chapters and tags, and a take without prompter presses has no beats; a crash keeps the logged words, and analysis writes the same bytes twice.
- `elements::mentions`, `elements::screen`, `elements::claude`: URLs said with their host, a phrase read off the screen with its box, the prompt and Claude's answer as elements.
- `crates/shot-cli/tests/analyze.rs`: `shot analyze --claude` against a fake `claude` writes Claude's elements with `by=claude` and sends only the numbered sentences; two runs without `--claude` keep them and write the same bytes.

## Not verified

- The app's record path end to end: no take was recorded (no free mic and display on the shared machine). The code paths compile and are the ones the CLI test runs.
- A real Claude run: only the fake `claude`. The bridge's system prompt asks Claude to propose and to end with prose; the annotation prompt tells it not to, and the reader skips non-JSON lines, but a real answer's shape is unchecked.

## Gaps against the contract

- `code`, `expression` have no local detector yet; `product` comes only from Claude.
- Labeled record markers: capture writes no label for M, so a `marker` is labeled M, Z or B only.
- Cursor dwell, which auto zoom uses, has no element kind.
- Two `{name}` tags on one beat share a `t`, and the contract allows one element per kind and `t`; the second is left out.
- Takes recorded without the prompter get no beats, chapters or tags; placing them would need the transcript aligned to the script.
