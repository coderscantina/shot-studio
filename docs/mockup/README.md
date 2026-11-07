# Mockup

The screens the app is built to, copied from the [Shot Studio mockup canvas](https://claude.ai/artifact/P6kQUAKTd96uCBaHTURQsa) on 2026-11-07. Open [index.html](index.html) in a browser to see the canvas, or one `*.dc.html` file for a single screen. The rules behind them are in [design.md](../design.md).

| File | Screen | Size |
| --- | --- | --- |
| `Main.dc.html` | Board | 1440x900 |
| `Script.dc.html` | Script | 1440x900 |
| `Record.dc.html` | Record | 1440x900 |
| `Review.dc.html` | Cut, with working Keep and Cut | 1440x900 |
| `Edit.dc.html` | Edit, Text workspace | 1600x1000 |
| `EditPicture.dc.html` | Edit, Picture workspace | 1600x1000 |
| `EditTimeline.dc.html` | Edit, Timeline workspace | 1600x1000 |
| `Product.dc.html` | Product demo | 1440x900 |
| `Ship.dc.html` | Ship, with working cut picker | 1440x900 |

- The `*.dc.html` files and `canvas.json` are byte-for-byte copies of the canvas files. Change a screen on the canvas, then copy the file here again, so the two stay identical.
- `support.js` is ours. It stands in for the canvas runtime, so `Review` and `Ship` render and react in a plain browser. `index.html` is generated from `canvas.json`.
- The 1600x1000 Edit workspaces are what milestone 03 compares the app against. The 1440x900 screens are fluid and stretch to the window.
- Fonts load from Google Fonts. Icons are inlined from [Hugeicons](https://hugeicons.com) (MIT).
