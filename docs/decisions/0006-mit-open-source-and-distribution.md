# 0006: MIT open source, signed DMG and Homebrew, macOS 26+

Status: Adopted by Mike on 2026-11-07

## Problem

How the app is licensed, which systems it supports and how people get it.

## Constraints

- Public repo under `coderscantina`.
- The release flow should match holocron.

## Choice

- MIT license.
- macOS 26 or newer, Apple Silicon only.
- CalVer `YY.M.D` with holocron's `release.sh` and tag-checked release workflow.
- A signed and notarized DMG on GitHub Releases and a Homebrew cask in a coderscantina tap. Auto update comes after the first release.

## Rejected alternatives

- Private tool or paid app first: Mike chose open source.
- MIT OR Apache-2.0: no need for the dual license.
- Source builds only: skips signing but makes the app hard to try.
- macOS 15 or 14 minimum: more fallbacks in capture code for an audience that updates quickly.

## Consequences

Signing needs an Apple Developer ID and a notarization profile before milestone 09. No private assets, restrictively licensed fonts or secrets go into the repo.

## Affected contracts

Workspace metadata, the release workflow, the cask.
