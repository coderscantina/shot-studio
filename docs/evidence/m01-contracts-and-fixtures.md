# Milestone 01 evidence: contracts, fixtures and workspace

Recorded 2026-11-07 on `integration` at `fd858a7`. Machine: MacBook Pro, Apple M1 Pro, 32 GB, macOS 27.0.1, rustc and cargo 1.99.1 (Homebrew), ffmpeg 9.0.2.

## Result

| Criterion | Evidence | Result |
| --- | --- | --- |
| A clean checkout passes all three checks locally and on CI | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass locally: 23 tests, 1 ignored. CI has not run because `github.com/coderscantina/shot-studio` does not exist yet | Local pass, CI not verified |
| Every contract example parses, writes back byte-identical, and an invalid file reports the right line | `shot-core` unit tests: round trip per file, error file:line:column, presets parse, examples match the contract text. `fixture_projects` tests: all committed KDL is canonical | Pass |
| The fixture generator yields identical checksums twice, and the 17 min project opens as a timeline with chapters | Two forced runs of `scripts/fixtures.sh all` (459 s and 490 s) logged all 21 outputs `unchanged`. `fixture_projects` evaluates the target project to 8 chapters matching `fixtures/spec/chapters.tsv` | Pass |
| The manifests pin every reference and model | `sources/manifest.json`: gpui (`gpui-pre =0.3.11`, Zed `f816e1f`), gpui-component 0.9.0, vello 0.11.0, parley 0.12.0 with revisions and licenses. `models/manifest.json`: whisper large-v3-turbo (HF revision `5359861`, sha256, 1,624,555,275 bytes) and Silero VAD (commit `066409f`) | Pass |

## Commands

```
SHOT_FIXTURES=$HOME/.cache/shot-studio-fixtures scripts/fixtures.sh all
SHOT_FIXTURES=$HOME/.cache/shot-studio-fixtures cargo test -p shot-board -- --ignored
```

## Fixtures

- Output 2.5 GB in `$SHOT_FIXTURES`. Media: libx265 single-threaded, bitexact muxing, speech by `say` voice Samantha.
- Target media: 3840x2160 and 1920x1080 at 60 fps, 1020.000 s, 61200 packets, 1020 `moof` boxes each. The mic runs 1020.021 s because of AAC priming.
- Demo channel: seven videos across all six columns, `video/<slug>` branches, one release tag, one open proposal. Ref shas are fixed. The tag was `v2026.42` at `49a8d40` when this was recorded and became `v26.10.14` (`3ee5e19`) when release tags moved to date CalVer.
- Checksums of the small set, for example `screen-30s.mp4` `1fd7a17b…f426e0a`. The full list is `$SHOT_FIXTURES/checksums.txt`.

## Not verified

- CI on `macos-latest`: no GitHub repo yet.
- Determinism across machines or macOS versions. A different `say` voice version changes the speech checksums.
- That `say` pronounces the filler words as intended. Nobody listened to it.
- The target narration is about 6 min of speech padded with silence, which is lighter than a real 17 min recording for the transcription target.
