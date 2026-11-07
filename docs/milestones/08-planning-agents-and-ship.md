# Milestone 08: Planning, agents and ship

## Outcome

Videos are planned on a Kanban board over the channel repo, scripts are written with an agent, outside agents work through the `shot` MCP server, `shot check` gates every cut, and `shot ship` uploads to YouTube and tags the release.

## Dependencies and ownership

The board and proposal tasks require 01 only. The script agent requires 03, `shot check` requires 04, `shot ship` requires 07. Core owns `shot-board`. Agents owns `shot-cli`, `shot-mcp` and `shot-ship`. App owns the Board, Script and Ship screens.

## Work

1. Board: open a channel repo, list videos by `video.kdl` status, move a card as a commit, show `git log` as the activity feed, shipped videos as tags.
2. Proposals as branches `proposal/<video>/<slug>`: create, show the diff, accept as merge, reject as delete ([0004](../decisions/0004-proposals-as-branches.md)).
3. The `shot` MCP server over stdio. Tools return text, frames only from explicit frame tasks ([0005](../decisions/0005-privacy-text-only-to-claude.md)).
4. The `claude` CLI bridge: run `claude` headless with the `shot` MCP server attached, stream its steps into the app, and show a clear setup screen when Claude Code isn't installed ([0003](../decisions/0003-claude-through-local-cli.md)).
5. Script agent: brief, sources, hook options, inline suggestions (Tab, Esc, Another take), slash commands, script checks. Ideas from notes, Linear issues and saved links, and "Plan the week with Claude".
6. `shot check` per cut: loudness, captions, frames, links, chapters, thumbnail. Passing checks stay quiet.
7. Thumbnail candidates from expression strength, title and description with word-accurate chapters.
8. `shot ship`: YouTube upload of main and Shorts with chapters, description and thumbnail, OAuth token in Keychain, X clip and others exported as files, release tag in git ([0009](../decisions/0009-ship-to-youtube.md)).

## Acceptance criteria

- On the demo channel repo, moving a card produces one commit, and the board rebuilds from git alone.
- An agent proposal shows as a diff, merges on accept and leaves no branch on reject.
- An outside Claude Code session can list videos, read a transcript and propose a cut through MCP.
- A network capture of a script agent session shows only text leaving the machine.
- `shot ship` against a private test upload publishes with chapters and creates the tag.

## Evidence and handoff

Report in `docs/evidence/m08-planning-agents-and-ship.md` with the git log of the demo repo, the MCP transcript, the network capture summary and screenshots in `docs/evidence/m08/`. The YouTube upload needs Mike's OAuth client, say so if it was unavailable. Consumed by 09.
