# 0005: Only text goes to Claude, no telemetry

Status: Adopted by Mike on 2026-11-07

## Problem

Recordings contain faces, voices and screens. Some judgement tasks could use images.

## Constraints

- Everything runs locally except judgement work.
- An open source app should make its data flow easy to audit.

## Choice

Claude receives transcripts, scripts, project files and the sources you attach. Single still frames go only when you trigger a frame task (thumbnail pick, Shorts reframe check). Raw audio and video never leave the machine. There is no telemetry. The YouTube OAuth token lives in Keychain.

## Rejected alternatives

- Text only, never frames: weaker thumbnail and reframe checks for little gain.
- Agents read frames and clips freely: hard to audit and unnecessary.

## Consequences

MCP tools that return images are marked as frame tasks and only run from an explicit user action. Milestone 08 proves the flow with a network capture.

## Affected contracts

`shot-mcp` tools, the `claude` bridge, `shot-ship` token storage.
