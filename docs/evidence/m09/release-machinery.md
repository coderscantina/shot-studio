# Milestone 09 items 1 to 3 and 6: release machinery

Date 2026-11-08, branch `task/m09-release`. Everything a release needs except Mike's Apple credentials and the tap repo. Signing and notarization were not run: no Developer ID or notary key was available.

## What Mike adds

1. Signing secrets: on 2026-11-08 the workflow switched to the coderscantina secrets Kessel already signs with (`MACOS_CERTIFICATE`, `MACOS_CERTIFICATE_PASSWORD`, `MACOS_SIGN_IDENTITY`, `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`), notarizing with the Apple ID and app-specific password. Nothing new is needed if the org shares them with `shot-studio`.
2. The `coderscantina/homebrew-tap` repo with `packaging/homebrew/shot-studio.rb` copied to `Casks/shot-studio.rb`. After each release, paste `version` and `sha256` from the release run's summary.
3. An `origin` remote: this clone has none yet. `release.sh` says so and prints the push command.

## Release flow

`./release.sh` (from holocron, CalVer `YY.M.D`, `.N` for the Nth release of a day, crate version `YY.M.D-N`): sets the workspace version, runs `cargo update --workspace` and `scripts/licenses.sh`, writes the CHANGELOG section from gitmoji commits since the last tag, commits `🔖 release YY.M.D` and tags `vYY.M.D-<short sha>`, holocron's scheme. Unlike holocron it never pushes; it prints `git push --atomic origin HEAD:main refs/tags/<tag>`. `--dry-run`, `--yes` and a highlights file work as in holocron.

`.github/workflows/release.yml` on `v*` tags:

- `check` (ubuntu): the tag must match the crate version and name its commit, the CHANGELOG must have the section, then it waits for `ci.yml` on the release commit. Copied from holocron.
- `dmg` (macos-latest, Apple Silicon): imports the certificate into a throwaway keychain, writes the notary key, runs `scripts/bundle.sh`, writes the cask's `version` and `sha256` to the run summary, creates the GitHub release with the DMG and the CHANGELOG section. Missing secrets skip signing or notarization with a `::warning::`, and the release is then a draft, so the cask never points at a DMG Gatekeeper rejects.

Actions are pinned by SHA with the versions ci.yml and holocron use.

`scripts/bundle.sh` is the one code path for CI and local builds: release build of `shot-app` and `shot`, `Shot Studio.app` (bundle id `com.coderscantina.shotstudio`, `LSMinimumSystemVersion` 26.0, arm64, the `shot` CLI next to `shot-app` in `Contents/MacOS` where `agent::shot_executable` already looks, `LICENSE` and `THIRD-PARTY-LICENSES.md` in `Resources`), codesign with the hardened runtime, notarize and staple the app (zip), DMG with an Applications link via `hdiutil`, then sign, notarize and staple the DMG. Packaging files are in `packaging/macos/`: `Info.plist`, `entitlements.plist`, a placeholder `AppIcon.svg` and the `AppIcon.icns` made from it.

### Entitlements and sandbox

- Hardened runtime with `com.apple.security.device.camera` and `com.apple.security.device.audio-input`, usage strings `NSCameraUsageDescription` and `NSMicrophoneUsageDescription`.
- Screen recording has no entitlement and no Info.plist usage string outside the App Sandbox; macOS asks on the first ScreenCaptureKit capture (`CGRequestScreenCaptureAccess`). So there is no screen recording string, contrary to the brief.
- No App Sandbox: the app opens video folders and channel repos anywhere on disk, and the Claude bridge starts the user's `claude` from `~/.local/bin` and others, which starts our `shot mcp`. A sandbox blocks both.
- No JIT or library validation exceptions: everything is linked statically, and runtime Metal shader compilation (gpui `runtime_shaders`, whisper.cpp) needs none.

### Unsigned builds are signed ad hoc

Without `SIGN_IDENTITY` the app is signed ad hoc (`codesign --sign -`) with the same entitlements, instead of left as the linker left it. The linker-signed executable inside an unsealed bundle fails `codesign --verify` with "code has no resources but signature indicates they must be present" and carries the identifier `shot_app-<hash>`, so camera and microphone prompts would not attach to the bundle id. Gatekeeper still rejects it (`spctl`: rejected) and the DMG stays unsigned. This differs from the brief's "codesign --verify reports it unsigned".

## Homebrew cask

`packaging/homebrew/shot-studio.rb`: `version "<YY.M.D>,<short sha>"` so the URL can rebuild the tag, `app` plus `binary` for `shot`, arm64 and `>= :tahoe`, livecheck on the latest GitHub release tag, zap for the models folder in Application Support. `brew style` passes. Not installed with `brew install --cask`: there is no release to download.

## Credits

- `scripts/licenses.sh` (cargo-about 0.9.3, `about.toml` at the root) writes `THIRD-PARTY-LICENSES.md`: every non-crate source in `sources/manifest.json` with license, attribution, source, use and the license text when one is named, then 512 crates linked on aarch64-apple-darwin (no dev or build dependencies, no workspace crates) grouped by license with full texts. MIT is preferred for dual-licensed crates. It also writes `crates/shot-app/assets/crates.txt`, one line per license, for the About window. 20 s.
- `sources/manifest.json` gained `license_file` on the three font entries and four entries: the app's Geist static fonts, the Hugeicons icons, libgit2 (GPL-2.0 with the linking exception, vendored by libgit2-sys, which cargo-about lists only as MIT OR Apache-2.0) and whisper.cpp and ggml (MIT, vendored by whisper-rs-sys, listed as Unlicense).
- `crates/shot-app/src/about.rs`: the About window. App name, version, MIT license; sources with a named attribution first (Noto Animated Emoji: "Noto Animated Emoji by Google, CC BY 4.0", with the change note from the manifest), then the CC0 sounds and beds, the OFL fonts, Hugeicons, libgit2 and whisper.cpp; then crates per license. Registered in `main.rs`: the app menu (About Shot Studio, Quit ⌘Q) and the About window when the app starts without arguments, as from Finder. One test: the Noto attribution is exact and every crate pinned in the manifest is in `crates.txt`.
- The known gap about the emoji attribution is removed.

Screenshots: `docs/evidence/m09/about.png` (top, Noto first) and `about-crates.png` (scrolled to the crate list), taken from the bundled app at 680x760. Differences from the mockup: it has no About screen; the window follows the design tokens (FRAME, SURFACE pane, Geist, muted labels).

## Verification

- `./release.sh --dry-run`: "Would release 26.11.8 (crate version 26.11.8) with 109 commits since the first commit", the tag line and the grouped section, nothing changed.
- `scripts/bundle.sh`: release build plus bundle in 38 s on a warm shared target. `Shot Studio.app` 100 MB (`shot-app` 65 MB, `shot` 40 MB, both with line-table debug info), DMG 43 MB. Both deleted afterwards.
- `open "dist/Shot Studio.app"` shows the About window; the app menu lists About Shot Studio and Quit Shot Studio.
- `Contents/MacOS/shot --version` prints `shot 26.11.7`, also from the mounted DMG.
- `codesign --verify --strict --deep`: valid ad hoc, `flags=0x10002(adhoc,runtime)`, camera and audio-input entitlements; `spctl --assess`: rejected; the DMG: "code object is not signed at all". All expected without a Developer ID.
- `actionlint` on the workflow and `shellcheck` on the three scripts pass.

## Not verified

- Signing, notarization, stapling and a Gatekeeper-clean install on a clean macOS 26 machine: no credentials.
- The workflow itself: no origin remote, never run on GitHub.
- `brew install --cask`: no release yet.
- Camera, microphone and screen recording prompts from the bundled app, and the Claude bridge under the hardened runtime.

## Follow-ups

- Opening a video folder from Finder: the app only takes a folder as an argument (`open -a "Shot Studio" --args <folder>`), so a Finder launch shows the About window. It needs an open panel or a recent list.
- The app keeps running after its last window closes (gpui's macOS default); with the new menu, ⌘Q quits.
- The tap update could be automated with a token on `coderscantina/homebrew-tap`.
- `strip -S` or `debug = false` for release bundles would shrink the 105 MB of binaries.
- CI could fail when `crates.txt` is stale after a dependency change; today only `release.sh` regenerates it.
- AGENTS.md checks could name `shellcheck release.sh scripts/*.sh` and `actionlint`.
- The real app icon.
