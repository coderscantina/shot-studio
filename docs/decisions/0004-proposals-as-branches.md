# 0004: Agent proposals as git branches

Status: Adopted by Mike on 2026-11-07

## Problem

Agents propose changes to `cut.kdl` and `script.md`. They need a review, accept and reject flow with history and undo.

## Constraints

- Reviewing should feel like a PR.
- No custom merge logic where git already does it.

## Choice

Each proposal is a branch `proposal/<video>/<slug>` off the video's branch, with one or more commits. Accept merges it, reject deletes the branch. `git log` shows what was accepted.

## Rejected alternatives

- A proposals file the app applies: needs its own apply and conflict logic.
- Uncommitted working-tree changes: only one proposal at a time, and easy to lose.

## Consequences

`shot-board` owns branch creation, diff, merge and delete. Conflicts between proposals surface as git conflicts in the review queue.

## Affected contracts

[Project files](../contracts/project-files.md#proposals), `shot-board`, the MCP proposal tools.
