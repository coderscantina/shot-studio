# Milestone 12: Styled monitor and watch builds

## Styled monitor (items 1 and 2)

Branch `task/m12-monitor`, details and method in [m12/monitor.md](m12/monitor.md). Measured 2026-11-08 on the reference MacBook Pro while other agents built and measured on the same machine.

Streaming analysis writes `word`, `url`, `click`, `typing` and `marker` elements while the take records, and the Record screen's monitor paints capture's live frames through `compose` and the same render graph as the painter, with the take's style rebuilt from the live elements and the lock: layouts, zooms on clicks, cards when a URL is said, captions and chapter bands. No cuts, muted. A Styled and Raw switch sits in the stage header.

| Criterion | Result |
| --- | --- |
| At most one dropped frame per 5 min in the recorded tracks with the styled monitor on | **Met.** 0 dropped screen frames in 39,628 frames (11.0 min), one take of 5 min and 11 of 25 to 34 s, recording the real 2560x1440 display while the 30 s screen fixture played full screen. Camera gaps (0 to 2 per take) come from the device and show in raw takes too. |
| A URL said shows its card within 500 ms on the monitor | **Not met.** 0.98 to 1.84 s, median 1.31 s, from the end of the word in a `say` clip fed through the mic tap path to the first monitor frame with the card, 11 takes. The monitor shows the card on the refresh the URL arrives; the wait is whisper's 1 s decode step plus the decode. Meeting 500 ms needs a faster hearing path (see the report). |
| CPU with the monitor on and off | Styled adds 3.1 to 4.7 points of one core: 21.3% raw to 24.4% styled with the app visible, 10.6% to 15.3% with the app covered. A frame takes 1.2 to 2.2 ms p50 on the live thread. |

Screenshots of the Record screen at 1600x1000 in dark: `m12/monitor-band.png`, `monitor-click-zoom.png`, `monitor-captions.png`, `monitor-card.png`, `monitor-zoom-card.png`, `monitor-raw.png`.
