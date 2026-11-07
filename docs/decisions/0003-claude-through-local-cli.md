# 0003: Claude through the local claude CLI

Status: Adopted by Mike on 2026-11-07

## Problem

Script drafts, unsure cuts, Shorts picks and caption fixes need Claude. The research on Christoph Rumpel's editor found that handing context to a subscription app by hand was fragile.

## Constraints

- No API key handling in an open source desktop app if it can be avoided.
- Outside agents and the app should use the same tools.

## Choice

The app runs the local `claude` CLI (Claude Code) headless with the `shot` MCP server attached, and streams its steps into the UI. Shot Studio stores no Anthropic API key. Without Claude Code installed, AI features show a setup screen and everything else works.

## Rejected alternatives

- Direct Messages API calls with a key in Keychain: key handling, per-token cost and a second tool path.
- Both CLI and API key: twice the surface to test.
- No in-app AI: the script agent and review help are core to the product.

## Consequences

Claude Code is a soft dependency. The MCP server is the only way Claude touches a project, so its tools are the AI contract.

## Affected contracts

`shot-mcp` tool list, the bridge in `shot-cli`, [0005](0005-privacy-text-only-to-claude.md).
