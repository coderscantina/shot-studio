# 0002: gpui UI with a zero-copy video preview

Status: Adopted by Mike on 2026-11-07, to be confirmed by the [milestone 02](../milestones/02-preview-spike.md) measurements.

## Problem

The preview has to scrub 4K60 without dropped frames, and the timeline, waveform and transcript need custom elements.

## Constraints

- No CPU copy of frame pixels between decode and display.
- Preview and export share one render graph.
- Mike prefers Rust with gpui for desktop apps.

## Choice

gpui, with `gpui-component` restyled with our tokens for standard widgets. Frames decode through VideoToolbox into IOSurface and reach gpui as a Metal texture. gpui is pinned to the exact `gpui-pre` crates.io snapshot of a Zed commit that gpui-component builds against, so only one gpui is compiled.

## Rejected alternatives

- Tauri or another webview: every preview frame would cross into the webview with a copy.
- AppKit or SwiftUI: splits the codebase across two languages and loses the shared Rust core in the UI.

## Consequences

The spike is a go/no-go gate. If it fails, 03 is gated and the UI choice goes back to Mike with the measurements. gpui upgrades are deliberate, one pin change at a time.

## Affected contracts

`shot-media` decode output, `shot-render` input textures, the `shot-app` preview element.
