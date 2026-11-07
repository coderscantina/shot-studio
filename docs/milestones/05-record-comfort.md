# Milestone 05: Record comfort

## Outcome

Mike records reading from the Elgato Prompter: the current beat large, the previous dim, the next smaller, words turning white as he says them. Scrolling follows his voice, runs steady or steps by Stream Deck pedal, and hotkeys drop markers and layout changes into the take.

## Dependencies and ownership

Requires 03. Analysis owns `shot-prompter`. App owns the prompter window settings and hotkeys in `shot-app`.

## Work

1. Test on the device whether the Elgato Prompter mirrors the image for the beam splitter. Mirror in software only if it doesn't. Record the answer in the report.
2. Prompter output window on the prompter display: beats or full script from `script.md` per `format.kdl`, time left, retake count.
3. Voice follow from the live transcript. Measure the lag from spoken word to highlight. If it is over 300 ms, add a lighter keyword matcher on top of streaming whisper.
4. Steady scroll and Stream Deck pedal mode.
5. Hotkeys during recording: A again, N next beat, M marker, Z zoom here, B b-roll here, 1 to 4 layout. Each writes a marker into the take.
6. Inline retake toast while recording.

## Acceptance criteria

- The prompter output reads correctly through the beam splitter on the real device.
- Voice-follow lag measured on the `say` fixtures and on a live read stays under 300 ms at the median.
- Every hotkey produces a marker that 04 and 07 consume.

## Evidence and handoff

Report in `docs/evidence/m05-record-comfort.md` with the mirroring answer, lag measurements and photos or screenshots in `docs/evidence/m05/`. The live read and device test need Mike at the device, say so in the report. Consumed by 07 (b-roll and zoom markers).
