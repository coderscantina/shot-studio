# Milestone 08 evidence: planning, agents and ship

Recorded 2026-11-07 on `task/m08-agents`, work items 3 to 7. Items 1 and 2 (board, proposals) were done before this task in `shot-board`, `shot-cli` and `shot-mcp`. Item 8 is pending, see [shot ship pending](#shot-ship-pending).

Machine: MacBook Pro, Apple M1 Pro, 32 GB, macOS 27, rustc 1.99.1, Claude Code 2.1.318. Milestone 07 built and measured in the same shared target dir at the same time, so timings carry that noise. Screenshots are in `m08/` (git-ignored), 1600x1000 in dark.

The demo data is a copy of `$SHOT_FIXTURES/demo-channel` with one more video, `opinion-target`: the target project's four files committed on `main` and its 17 min take hard-linked into `media/t01/`, transcribed with `shot transcribe` (983 words, 91.6 s).

## Acceptance

| Criterion | Evidence | Result |
| --- | --- | --- |
| An outside Claude Code session can list videos, read a transcript and propose a cut through MCP | [Outside session](#outside-claude-code-session): a plain `claude -p` with only `--mcp-config` pointing at `shot mcp` called `list_videos`, `read_transcript` and `propose_cut`; branch `proposal/opinion-target/outside-tighten` removes `t01 w25-w26 "For example,"` | Pass |
| A network capture of a script agent session shows only text leaving the machine | [Privacy](#privacy-what-leaves-the-machine): `lsof` sampled every 0.5 s during a bridge run, plus the byte logs of six sessions. The `claude` process talked only to Anthropic's API (160.79.104.10:443, 2607:6bc0::10:443), the `shot` processes opened no socket, every MCP reply was text, no image block | Pass, with the limits listed there |
| Proposals render inline and accept/reject work | Script screen screenshots; Esc rejected `tighten` (branch gone), picking hook B merged it and deleted `hook-a` and `hook-c`, Tab on a selected card merged `b-roll-notes` | Pass |
| Screens at 1600x1000 in dark against the mockup | `m08/board*.png`, `m08/script*.png` next to `m08/mockup-*.png`, differences [below](#differences-from-the-mockup) | Pass with listed differences |
| `shot ship` against a private test upload | Not in this task | Pending |

## What was built

**3. MCP server** (`crates/shot-mcp`). New text tools: `read_transcript` (cut order, edited time, word numbers, removed runs as `[um so](filler)`), `propose_cut` (spans of recorded words to remove or restore, applied through `Cut::delete_words` and `Cut::restore`, so a cut can only drop or bring back recorded words), `review_queue`, `propose_review` (keep or cut per item through `Cut::decide`), `caption_fixes` (the dictionary and low-confidence words; fixes are suggested as text, never proposed), `script_checks`, `propose_script` (must parse as `script.md`), `describe`. `create_proposal` now refuses project files that do not parse. `thumbnail_frames` is the only tool that returns images; it is a frame task and refuses unless the server was started with `--frame-task`. `shot mcp --log <file>` appends every byte the server writes to the client.

**4. The `claude` bridge** (`shot_mcp::bridge`, `shot agent`). Runs `claude --print --output-format stream-json --verbose` with the prompt on stdin, `--strict-mcp-config --mcp-config` for `shot mcp`, `--tools ""` (no built-in tools: no Read, Bash or WebFetch), `--allowedTools mcp__shot`, `--permission-mode dontAsk`, `--setting-sources ""`, `--disable-slash-commands`, `--no-session-persistence`, in an empty session folder, with `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`, `DISABLE_TELEMETRY` and `DISABLE_ERROR_REPORTING` set. Flags were taken from the installed `claude --help`. Steps stream as `Started`, `Text`, `ToolUse`, `ToolResult`, `Finished`. A run whose `shot` server did not connect stops at the start with a clear error. Each session writes `sent.txt` (argv and prompt), `mcp.jsonl` (every MCP reply) and `steps.jsonl` (Claude's stream) to `<repo>/.shot/agent/<id>/`, which git ignores. `Claude::find` looks at `$SHOT_CLAUDE`, `PATH`, `~/.claude/local`, `~/.local/bin` and Homebrew; without it the Board and Script screens show a setup notice and everything else works. No API key is stored. Tests use a fake `claude` script.

**5. Script agent, Script and Board screens** (`crates/shot-app/src/script/`, `board.rs`, `agent.rs`). Script: outline with spoken length per chapter, sources, script checks (passing ones as a quiet check line, problems as amber notes, beats without a source counted together), the script from the video branch in Beats or Full script view, and the agent pane with the request, its steps, Claude's answer, the four slash commands and a prompt box (a brief drafts when there is no script yet). Proposals that change `script.md` show inline in place of the beats they replace, struck and inserted words, the spoken time they save or add, Accept (Tab), Reject (Esc) and Another take (rejects, then asks Claude for another take with the rejected diff). Tab and Esc act only while a proposal card is selected. Proposals named `hook-*` show as hook options; picking one accepts it and rejects the others. Board: six columns from `video.kdl`, format filter, next publish date, cadence, drag a card to move it (one commit), release tags on shipped cards, `git log` line, and "Plan the week with Claude" with its steps in a side pane. Script checks are in `shot_ship::script_check` (hook under 15 s, 150 words a minute, every beat has a source, no stretch over 90 s without a source or `B-roll:` note).

**6. `shot check`** (`shot_ship::check`, `shot_ship::media::check_cut`). Prints only problems, exits 1 when there are any. Loudness and true peak of the export (shot-audio's EBU R128 meter) against -14 LUFS ±1 and -1 dBTP; captions: sidecar present, every caption fix applied, no take without a transcript, low-confidence words listed; frames: export sampled every 0.5 s, black for 1 s or more, frozen picture for 3 s or more; links said out loud (`laravel.com`, `shot dot dev`) without a `link-card` overlay within 10 s; chapters: at least 3, each at least 10 s, no `Take tNN` placeholder titles; thumbnail present next to the export.

**7. Thumbnails and description** (`shot_ship::thumbnails`, `shot_ship::describe`, `shot thumbnails`, `shot describe`). Camera frames the main cut shows in a face layout, sampled every 1 s, scored by Apple Vision face capture quality (0.6) and mouth openness from face landmarks (0.4), best ones at least 10 s apart written as PNG to `.shot/cache/thumbnails/`; `--pick <png>` makes a still the thumbnail at `.shot/export/main.png`. The description is the hook beat, chapters at the moment their first word is heard (0:00 for the first) and the links of link cards and the script.

## Outside Claude Code session

`claude -p --strict-mcp-config --mcp-config mcp.json --allowedTools mcp__shot --model sonnet`, with `mcp.json` starting `shot mcp --repo <demo channel>`. The prompt asked it to list the videos, read the transcript of `opinion-target` and propose one cut. Its calls, from the stream:

```
CALL mcp__shot__list_videos {}
RESULT ideas (2) | agents-that-write-changelogs opinion "Agents that write your changelog" (no branch yet) | ...
CALL mcp__shot__read_transcript {"slug": "opinion-target"}
RESULT ## chapter 1 "Intro", take t01 | 0:00.1 w0 Welcome to the channel. In this video we | 0:02.3 w8 walk through a full ...
CALL mcp__shot__propose_cut {"slug": "opinion-target", "name": "outside-tighten", "subject": "Cut the stray \"For example,\" from the intro", "remove": [{"take": "t01", "from_word": 25, "to_word": 26}]}
RESULT created proposal opinion-target/outside-tighten at e11cdde |   removes t01 w25-w26 "For example," (0.5 s)
DONE success, $0.22
```

It also noted three likely mis-transcriptions ("grooved", "earns its key") as text, without proposing them, which is how caption fixes are meant to reach a person.

## Privacy: what leaves the machine

One `/tighten` run through `shot agent` was sampled with `lsof -i` every 0.5 s (no root, so no packet capture):

- The `claude` process of the run (pid 74906) had TCP connections to 160.79.104.10:443 and [2607:6bc0::10]:443 only, both Anthropic API addresses. Another Claude Code session on the machine (pid 78418, not ours) also showed a Google Cloud address; it is a different process.
- The `shot agent` and `shot mcp` processes never had a socket.

The byte logs of every bridge session on the demo repo:

| Session | Prompt bytes | MCP reply bytes | Image blocks | Tools Claude had | Cost |
| --- | --- | --- | --- | --- | --- |
| `/tighten` (CLI) | 145 | 20,195 | 0 | 16, all `mcp__shot__*` | $0.153 |
| `/hooks` (CLI) | 231 | 19,884 | 0 | 16, all `mcp__shot__*` | $0.136 |
| `/b-roll notes` (app, server failed) | 195 | 0 | 0 | none, server failed to start | $0.032 |
| `/b-roll notes` (app) | 195 | 20,188 | 0 | 16, all `mcp__shot__*` | $0.113 |
| Plan the week (app) | 391 | 11,426 | 0 | 16, all `mcp__shot__*` | $0.061 |
| Plan the week (app) | 391 | 13,722 | 0 | 16, all `mcp__shot__*` | $0.048 |

Limits: Claude Code also sends its own system prompt and, with `--setting-sources ""`, may still include the user's global `CLAUDE.md`; both are text. The bridge cannot see inside Claude Code's HTTPS, so "only text" rests on the closed tool set (no built-in tools, only `shot` tools that return text) plus the logs, not on decrypting the traffic. A failed session showed why the server-start check matters: another agent's build had replaced the shared `release/shot` with a binary without `--log`, Claude answered without tools; the bridge now stops such a run at the start.

## shot check, thumbnails and description

On a three-chapter copy of `opinion-target` (5:28 exported in 227 s, 661 chunks):

```
$ shot check --video opinion-short
loudness: true peak is -0.9 dBTP, above the -1 dBTP ceiling
captions: 4 words heard with low confidence: "In" at 0:01, "For" at 0:07, "microphone" at 1:34, "uses" at 1:58; add a caption fix where it was misheard
thumbnail: no thumbnail yet, pick one from `shot thumbnails`
shot: 3 problems in main            (15.0 s)
```

The peak finding is real: ffmpeg's `ebur128=peak=true` on the same file reads I -14.3 LUFS and true peak -0.4 dBFS, so the AAC encode overshoots the finish's -1 dBTP ceiling. After `shot thumbnails --pick` the thumbnail problem is gone. On a crafted clip (3 s test pattern, 2 s black, 5 s still) the frames check reports `black from 0:03 to 0:05` and `the picture freezes from 0:05 to 0:10`.

```
$ shot describe --video opinion-short
A full recording session from first take to export

I press stop, and a few seconds later the rough cut is ready. Here is the whole session.

Chapters
0:00 Intro
1:27 Install and setup
3:36 Project structure
```

`shot thumbnails` found no face: the fixture camera is a synthetic blocky face, and Apple's own Swift `VNDetectFaceRectanglesRequest` finds no face in the same frame either. The Vision calls ran without error; ranking and scoring are unit tested. Scoring on a real face is not verified.

## Script and board runs

- `/tighten` proposed `tighten` (33 beats to 28), shown inline as 12 changes; Esc on the selected card rejected it.
- `/hooks` proposed `hook-a`, `hook-b`, `hook-c`; clicking hook B merged it (`video/opinion-target` created at `0f6bdb6`) and deleted the other two.
- `/b-roll notes` from the app proposed `b-roll-notes` with 14 `B-roll:` beats (screenshot while running and after); Tab on a selected card merged it (`b606d99`).
- Dragging "Agents that write your changelog" to Scripting made one commit, `de50ead 🚚 move agents-that-write-changelogs to scripting`, and the board rebuilt from git.
- Plan the week moved `why-we-left-docker-compose` to scripting (`6f7fa6b`) and explained the next three weeks against two videos a week.

Git log of the demo repo after the runs:

```
* 6f7fa6b 🚚 move why-we-left-docker-compose to scripting
| * b606d99 Add B-roll beats to every chapter of opinion-target
| * 0f6bdb6 Hook B: open with the payoff, the rough cut seconds after stop
|/
| * de50ead 🚚 move agents-that-write-changelogs to scripting
|/
| * e11cdde Cut the stray "For example," from the intro       (proposal/opinion-target/outside-tighten)
|/
* 1cc9fa0 📝 add opinion-target
```

## Differences from the mockup

Board (`board.png`, `board-after-drag.png`, `board-plan-the-week.png` against `mockup-board.png`):

- No "New video" button and no "+ Capture idea": `shot-board` has no call to create a video yet.
- Cards show the branch name, "No branch yet" or the release tag instead of the mockup's agent status lines ("Claude is drafting chapter 3 of 4", "6 cuts need your call").
- Cadence is a constant 2 per week; nothing in the channel repo stores it.
- No "Synced with origin" (the board is local only) and no Commands (⌘K).
- Plan the week opens a side pane with the steps; the mockup does not show where the plan appears.

Script (`script-*.png` against `mockup-script.png`):

- One proposal is one branch, so a multi-change proposal shows a card per change marked "3 of 12, accepted together"; the mockup shows a single change.
- A heavily rewritten line shows struck whole and then inserted, a lightly edited one word by word.
- Chapter headers show no layout ("Face + content"): the script has no layout, the cut does.
- No "Elgato Prompter" button; the Beats / Full script switch changes the view, it does not write `format.kdl`.
- The brief bubble shows the request as typed; there is no separate brief form.
- The agent answer is plain text; Markdown bold is stripped.

## Not done or not verified

- **Linear**: no Linear MCP server or CLI on this machine (`claude mcp list` shows Claude Docs, Gmail, Calendar, Drive, Figma, Blender), so ideas from Linear are skipped. Ideas come from the cards and their `sources/`, read with `read_video`.
- **Cut proposals inline in the Edit screen** (design rule "Agent proposals in the editor") are not built: the Edit screen is not in this task's files. `propose_cut` gives the Edit owner word spans to render.
- **The thumbnail frame task from the app**: the bridge and server support it (`Session::frame_task`, `--frame-task`), but the button belongs on the Ship screen, which this task does not own. The Shorts reframe check waits for 07's derived cuts.
- **Thumbnail scoring on a real face**, see above.
- **A packet capture**: needs root; `lsof` sampling and byte logs stand in.

## shot ship pending

Item 8, `shot ship` (YouTube upload of main and Shorts with chapters, description and thumbnail, OAuth token in Keychain, X clip as a file, release tag), waits for milestone 07's derived cuts and for Mike's Google OAuth desktop client ([0009](../decisions/0009-ship-to-youtube.md)). What it can build on from here: `shot_ship::describe` for the description and word-accurate chapters, `shot check` as the gate, `.shot/export/main.png` as the thumbnail, and `Board::tag_release` for the tag.
