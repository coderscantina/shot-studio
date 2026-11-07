# Shot Studio development handoff

A native macOS recorder and editor for developer videos, built in Rust with gpui, with a CLI and MCP server on the same core. It is built for developers who publish on a schedule, starting with the Coder's Cantina channel.

## Read in this order

1. [Technical briefing](technical-briefing.md): scope, architecture, privacy, targets, release.
2. [Project files contract](contracts/project-files.md): the video folder, `video.kdl`, `script.md`, `cut.kdl`, `format.kdl` and the cache.
3. [Milestone index](milestones/README.md): dependency order, parallel work, briefs.
4. [Development workflow](development-workflow.md): ownership, shared interfaces, completion evidence.
5. [Design](design.md): visual rules for the app.
6. [Decisions](decisions/): the records behind the briefing. Read the ones your milestone links.

## Requirements that drive the design

| Constraint | Consequence |
| --- | --- |
| Rough cut ready within 5 s of stop | Transcription and analysis stream during recording, never as a batch step |
| Edit is a text file in git | `shot-core` evaluates `cut.kdl` with no IO, and every UI action becomes a file change |
| 4K60 preview with no dropped frames | Decode lands in IOSurface and reaches gpui as a Metal texture, no CPU copy |
| Full 4K export under 5 min, one chapter under 20 s | Segments render in parallel, cached by content hash, joined by stream copy |
| Preview equals export | One wgpu render graph serves both |
| Text only goes to Claude | MCP tools return text, frames only from explicit frame tasks |
| Open source, signed DMG | No private assets or secrets in the repo, notarization in the release workflow |
| Generated test media | Every test and target runs from fixtures any agent can recreate |

## References

Pinned in [sources/manifest.json](../sources/manifest.json): gpui (Zed), gpui-component, vello, parley, whisper large-v3-turbo and Silero VAD models. Milestone 01 fills in the revisions.

The workflow and docs layout are copied from Mike's holocron project. Background research on Christoph Rumpel's "Creditor" editor is summarized in [the technical briefing](technical-briefing.md#product-contract).

## Evidence and status

Reports go under `evidence/` as `mNN-<slug>.md`, with screenshots in `evidence/mNN/`. A report states the source revision, command, fixture, machine, versions, result and what was not verified. Real recordings, secrets and large workloads stay outside Git.

[STATUS.md](STATUS.md) is the handover between threads.
