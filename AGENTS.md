# Shot Studio agent guide

Shot Studio is a native macOS app (Rust, gpui) for recording and editing developer videos, with a `shot` CLI and MCP server on one headless core. It is for developers who publish on a schedule, first of all the Coder's Cantina channel, and replaces a ScreenFlow plus DaVinci Resolve workflow.

## Start here

Read [docs/README.md](docs/README.md) and follow its reading order. The technical briefing owns scope, the milestone index owns sequence and status, and the development workflow owns how you take, change and hand back work. Read [docs/STATUS.md](docs/STATUS.md) before starting.

## Rules that apply to every task

- Work from an assigned milestone or brief. Before editing, state the files you own, the interface you consume, the behavior you add and the check that proves it.
- Stay inside your file ownership. Changes to shared files (workspace `Cargo.toml`, the project files contract, design tokens, the MCP tool list) go to their owner as a request.
- Regenerate generated files with the command that makes them. Never edit them by hand.
- Scope changes, relaxed targets and anything touching privacy or secrets go to Mike. Routine choices inside the contract are yours.
- Test media is generated. Never commit real recordings, tokens or model files.
- A subagent in its own worktree commits its checked task to its task branch. Everything else waits until Mike asks for a commit.
- Hand back with the [completion report](docs/development-workflow.md#completion-report).

## Checks

Once milestone 01 scaffolds the workspace, run the checks for the paths you changed:

- Rust: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`

Documentation names a command only after the code adds it.

## Demo

Filled in once the app runs: build command, the generated demo channel repo, fixture project, and how to point the app at them. The `demo` skill reads this section.

## Gotchas

One line for each trap that cost an agent time and that no config reveals.

## Writing

Plain words, sentence case headings, straight quotes, no em dashes. Gitmoji commits with a lowercase plain-language subject, for example `✨ cut silences longer than the pacing allows`.
