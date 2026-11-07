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

- **Icons** (2026-11-07): the [Hugeicons](https://hugeicons.com) free set (MIT, `@hugeicons/core-free-icons`), stroke rounded at 1.5 px, colored with the text they sit beside. 18 px in buttons, nav and lists, 14 px inside tags. Replaces the one-off inline SVGs of the first mockup.
- **Edit workspaces** (2026-11-07): the Edit screen has three workspaces over the same panes, switched in the title bar or with ⌥1 to ⌥3. Text: chapters, transcript as a document, preview with a selection inspector, overview bar. Picture: 1040 px preview on a dark stage, layout picker (1 to 4), on-screen layers, three rolling transcript lines, overview bar. Timeline: preview and change list on top, lanes below. A menu next to the switcher lists your saved workspaces (⌥4 and up), sets the workspace each format opens in (Opinion opens in Text), and saves or resets a layout; panes resize by their edges and move by their titles. Text and Picture scored 8, the menu 8. Rejected: picking one fixed layout, from round 1's "Combined approach, allow to alter own workspace to match habits". Timeline lanes, one per kind: Layout, Zoom, B-roll, Highlight, Words (spoken words placed by time, cuts as red seams with their length, a hovered cut opens to show the removed words), Voice waveform, Music; lime playhead; a selection spans every lane with its H, Z and Backspace toolbar below. Scored 8, then 9 for the whole workspace. Rejected: the refined lanes (Effects lane shared by zoom, b-roll and highlight, white playhead, ruler ticks, Restore popover, ducking curve), "Keep round 2".
- **Agent proposals in the editor** (2026-11-07): shown inline where they apply, as struck and inserted words with the time saved, and Accept (Tab), Reject (Esc) and Another take. Away from the text, the card says where it applies and offers "Show at" with its timecode (G), the text wraps at 64 characters, and a lime spark marks it on the overview bar (9). The branch behind it stays out of sight. Rejected: the `cut.kdl` diff with a Merge button, scored 4, "Not user centered".
- **Whole-video overview** (2026-11-07): one bar for the full length, with chapter blocks, cut ticks, a frame around what the main pane shows, and the playhead. Workspaces without a full timeline use it. Rejected: the chapter filmstrip with zoom and b-roll ribbons (6, no note), and the overview bar above lanes (7, "More polished, easier to use", kept as the top of the Timeline workspace).
