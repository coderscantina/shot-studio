# 0008: Models download on first use

Status: Adopted by Mike on 2026-11-07

## Problem

whisper large-v3-turbo is about 1.6 GB, and the VAD and denoise models add more.

## Constraints

- Keep the DMG small.
- Downloads must be verifiable.

## Choice

Models download on first use into `~/Library/Application Support/Shot Studio/models`, pinned by URL and SHA-256 in a manifest in the repo. The app shows progress, and `shot models fetch` does the same from the CLI.

## Rejected alternatives

- Bundling the models in the DMG: about 2 GB to download before anything works.

## Consequences

The first recording needs network once. Tests fetch models into a cache directory, and CI caches it.

## Affected contracts

`shot-analyze` model loading, the models manifest.
