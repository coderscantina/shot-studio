# Milestone 05 evidence: record comfort

Recorded 2026-11-07 on `task/m05-prompter`. Machine: MacBook Pro, Apple M1 Pro, 32 GB, macOS 27, rustc 1.99.1, with the Elgato Prompter attached as a third display ("Elgato Prom.", 1024x600) next to two U2515H. Milestones 04 and 06 agents were building and measuring at the same time. Screenshots are in `m05/` (git-ignored).

## Result

| Criterion | Evidence | Result |
| --- | --- | --- |
| The prompter output reads correctly through the beam splitter on the real device | The output window opens borderless over the whole "Elgato Prom." display, picked by name, without a Space and without moving the main window: `m05/prompter-display.png`, `m05/prompter-recording.png`. The mirror switch flips it in the compositor: `m05/prompter-mirrored.png`. Whether the hardware already flips the image needs Mike at the glass | Open, needs Mike |
| Voice-follow lag under 300 ms at the median on the `say` fixtures and a live read | `say` fixtures fed in real time, all 97 words highlighted: median -20 ms from the word's end, 241 ms from its start, p90 696 ms. Stable transcript words alone: 2.6 s median. Live read not done | Pass on fixtures, live read open |
| Every hotkey produces a marker that 04 and 07 consume | A real take with ⌃⌥N, N, Z, 2, A, M, B, 1 wrote eight `marker` lines with beats and layouts, no `key` lines for them, and `cut.kdl` got `layout split` and `zoom 150 ... by=marker` on append | Pass |

## Mirroring

Open question for Mike: with the window on the Elgato, does the text read correctly through the beam splitter with Mirror off? Mirror defaults to off. If the text is backwards, switch Mirror on in the Record screen's prompter pane; the setting is saved per machine. The answer goes into the technical briefing's risk list.

## Voice follow

The take's transcriber calls a word stable only after two decodes a second apart agree, which puts the highlight 2.5 s behind the voice. So the prompter listens itself: `shot_prompter::Listener` decodes the last 3 s of mic audio with whisper tiny.en on the CPU every 100 ms (the transcriber keeps the GPU), skips silent windows, and hands the words to a local alignment against the script around the current position. A run needs two agreeing words, or one word of four letters or more right at the position, so a stray "the" never jumps ahead, and a repeated phrase is matched at its nearer copy.

`SHOT_FIXTURES=... cargo test --release -p shot-prompter --test follow_lag -- --ignored --nocapture` feeds each fixture in 10 ms chunks at real time to the listener and the transcriber together, as while recording. The spoken time of a word is its end in the offline transcript, which reaches halfway into the pause after it, so word ends are late and the -20 ms is optimistic; 241 ms from the word's start is the conservative number.

| Fixture | Words | Fast path median from word end | Stable words median |
| --- | --- | --- | --- |
| clean | 18 | -20 ms | 2508 ms |
| filler | 24 | -1 ms | 2491 ms |
| long-silence | 10 | 21 ms | 2739 ms |
| mid-pause | 8 | -122 ms | 2733 ms |
| retake | 26 | -174 ms | 2573 ms |
| self-correction | 11 | 193 ms | 4788 ms |
| all | 97 | -20 ms (p90 696 ms, 241 ms from start) | 2615 ms |

The tiny.en model (78 MB) downloads on first use like the others; it is pinned in `shot_prompter::follow_model()` with its size and SHA-256.

## Hotkeys

Carbon `RegisterEventHotKey`, registered from the press of Record to the end of the take. The system hands only the exact combinations to Shot Studio and consumes them, every other key reaches the app in front, and no permission is needed. Defaults are ⌃⌥ plus A, N, M, Z, B, 1 to 4, so typing in a product demo never drops a marker; "Key alone" in the pane switches to plain letters for face videos. The pedal key (default F18, choices F13 to F19 and Page Down) is registered in Pedal mode and acts as N. The event tap leaves these presses out of `key` events so the keystroke overlay never shows them.

`m05/record-recording.png` shows the legend under the stage. Markers carry the cursor at the press and, for A and N, the beat the prompter shows after it. Layout keys pick the format's layouts in order, 1 being the default.

On append, `shot_prompter::apply_markers` writes layout markers as `layout` ranges (until the next layout marker, none for the default layout) and Z markers as `zoom ... by=marker` centered on the cursor, with the format's auto zoom scale and hold or 150 % for 2 s, yielding to other zooms. A, N, M and B stay in the event log for the retake rule, chapters and b-roll.

## Screens

- `m05/record-idle.png`, `m05/record-recording.png`: the Record screen at 1600x1000. The prompter pane replaces the transcript pane when the script has beats, as in the mockup.
- `m05/prompter-said-words.png`: said words in white, in steady scroll.
- Differences from the mockup: no layout picker in the stage header; chapters progress is a text line, not blocks; the "Also shows" row is gone since time left and retakes always show; Mirror, Display, Pedal key and Hotkeys rows are added; with the ⌃⌥ prefix the hotkey legend wraps to two lines at 1600 px.

## Not verified

- The beam splitter test and a live read, both need Mike at the prompter.
- The retake toast on screen: nothing calls it until milestone 04's retake rule does (see below).
- Voice follow in a live recording: the mic heard a quiet room. The fixture test covers the same path except the mic tap.
- A real Stream Deck pedal. Synthetic presses of the hotkeys went through the same Carbon path.

## Handoff

- Retake toast for 04: when the rule catches an inline retake while recording, call `live.lock().caught(shot_prompter::Retake { heard, kept })` on the Record screen's `Arc<Live>` (`PrompterPane::live()`). It counts a retake and shows the toast for 4 s.
- A and N markers for 04 and B markers for 07 are in `events.jsonl`, read with `shot_capture::events::read_markers`.
- `models/manifest.json` should carry `whisper-tiny-en` next to the other pins; the entry is in `follow_model()` until the manifest owner moves it.
