# 0001: One headless core with three front ends

Status: Adopted by Mike on 2026-11-07

## Problem

Clicking in the app, scripting from a terminal and agents working through MCP must produce the same edits, or the text-file promise breaks.

## Constraints

- The edit lives in `cut.kdl`, readable by people and agents.
- Agents must be able to do everything the UI does.

## Choice

All behavior lives in library crates. `shot-app` (gpui), `shot-cli` (binary `shot`) and `shot-mcp` (stdio server `shot`) are thin front ends over the same functions. `shot-core` evaluates the project with no IO. Every crate is prefixed `shot-` because the `shot` crate name is taken on crates.io.

## Rejected alternatives

- UI first with a CLI added later: the CLI would drift from the UI's behavior.
- A local HTTP server between UI and core: an extra process and protocol with no second client that needs it.

## Consequences

Front ends contain no domain logic. A new feature lands as a core function first, then gets its button, subcommand and MCP tool.

## Affected contracts

[Project files](../contracts/project-files.md), the MCP tool list, the CLI subcommands.
