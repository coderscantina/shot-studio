# Milestone 09: Release gate

## Outcome

Shot Studio's first release is downloadable as a signed and notarized DMG and through Homebrew, every target is measured on the reference machine, and one real video goes through the whole workflow from board to ship.

## Dependencies and ownership

Requires 01 to 08 and 10 to 12. Core owns `release.sh`, `.github/workflows/release.yml` and the cask. Every role fixes the failures in its modules.

## Work

1. `release.sh` from holocron: CalVer `YY.M.D`, CHANGELOG section from gitmoji commits, commit, tag. Pushing waits for Mike.
2. Release workflow: check the tag against the crate version, wait for CI, build the app bundle with the CLI inside, codesign, notarize, staple, upload the DMG to GitHub Releases.
3. Homebrew cask in `coderscantina/homebrew-tap`.
4. Measure every [target](../technical-briefing.md#targets) on the reference machine with the generated 17 min 4K60 project.
5. Run one full video on the fixture project: board card, script, record, auto cut, edit, derived cuts, check, ship to a private upload.
6. Public README, website link to shot.app, community files reviewed.

## Acceptance criteria

- Every target passes on the reference machine in a release build.
- The DMG installs on a clean macOS 26 machine without Gatekeeper warnings, and `brew install --cask` works.
- The full workflow run completes with no manual file edits.
- CI and the release workflow are green on the release commit.

## Evidence and handoff

Report in `docs/evidence/m09-release-gate.md` with the target table, notarization log summary, workflow run links and screenshots in `docs/evidence/m09/`. Signing needs Mike's Developer ID, say so if it was unavailable.
