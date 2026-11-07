# Design

This document owns the visual rules of the app. The [mockup canvas](https://claude.ai/artifact/P6kQUAKTd96uCBaHTURQsa) shows the seven screens: Board, Script, Record, Cut, Edit, Product demo, Ship.

## Defaults

- **Theme**: dark only in the first release. Tokens are named by role (surface, raised, text, muted, accent, danger) so a light theme can be added later without touching screens.
- **Surfaces**: panels are separated by shade, not borders.
- **Accent**: lime `#C8F25A` for actions and data. Red only for recording and cuts.
- **Type**: Geist, 15 px base. Geist Mono only for timecodes, code and file names.
- **Corners**: 8 to 14 px.
- **Components**: `gpui-component`, restyled with these tokens. Custom elements (timeline, waveform, transcript, preview surface) are built on raw gpui and use the same tokens.
- **Window**: minimum 1280x800.
- **Screenshots**: evidence is captured at 1600x1000 in dark.
- **Keyboard first**: every review action has a single-key shortcut (K keep, C cut, Tab accept, Esc reject), shown next to the button.

## Settled rules

Rules Mike settles, with the date and the rejected variants and why each lost. The `design-variants` skill adds to this list.
