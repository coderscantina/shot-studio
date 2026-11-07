# Milestone 01: Contracts, fixtures and workspace

## Outcome

The workspace builds and passes its checks on CI, the project files have a written grammar with parse tests, and any agent can generate the test media and the 17 min 4K60 target workload with one command. Every later milestone builds on these.

## Dependencies and ownership

No dependencies. Core owns the root `Cargo.toml`, `crates/shot-core`, `docs/contracts/project-files.md`, `scripts/`, `fixtures/`, `sources/manifest.json` and `.github/`.

## Work

1. Scaffold the cargo workspace: every crate from the [architecture](../technical-briefing.md#architecture) as an empty library or binary, edition 2024, `rust-version = "1.99"`, resolver 3, `[workspace.dependencies]`, MIT license metadata, `publish = false`.
2. Pin gpui to a Zed git revision, `gpui-component`, vello and parley to exact versions. Record name, revision, license and purpose of each in `sources/manifest.json`.
3. Write the grammar of `video.kdl`, `cut.kdl` and `format.kdl` into [the contract](../contracts/project-files.md), with one example per file. Implement parse and write in `shot-core` with round-trip tests and line-numbered errors.
4. Write the four format presets (reaction, opinion, product tour, tutorial) as `format.kdl` files.
5. Add a fixture generator script that produces, outside Git under an ignored directory:
   - short speech clips spoken by macOS `say` with known text, including a retake ("the the first") and a self-correction
   - a 30 s synthetic screen capture with cursor events and a synthetic camera clip
   - the 17 min 4K60 target project with chapters, separate tracks and a `cut.kdl`
   - a demo channel repo with a few videos in every board column
   The generator is deterministic: the same commit produces the same files and checksums.
6. Pin the whisper large-v3-turbo and Silero VAD model URLs and SHA-256 sums in a models manifest in the repo ([0008](../decisions/0008-models-download-on-first-use.md)).
7. Add CI on `macos-latest`: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. Pin actions by SHA, cache with `Swatinem/rust-cache`, monthly grouped dependabot.
8. Add the community files following the b10cks set: LICENSE, CONTRIBUTING (gitmoji, AI-assistance disclosure), CODE_OF_CONDUCT, SECURITY, CHANGELOG, `.editorconfig`, PR template and issue forms.

## Acceptance criteria

- A clean checkout passes all three checks locally and on CI.
- Every example in the contract parses, writes back byte-identical, and an invalid file reports the right line.
- Running the fixture generator twice yields identical checksums, and the 17 min project opens in `shot-core` as a timeline with chapters.
- The manifests pin every reference and model by revision or checksum.

## Evidence and handoff

Report in `docs/evidence/m01-contracts-and-fixtures.md`: commit, generator runtime, fixture checksums, CI run link. Fill in the Checks and Demo sections of `AGENTS.md`. Consumed by every later milestone, first by 02.
