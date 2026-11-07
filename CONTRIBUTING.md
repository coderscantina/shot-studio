# Contributing to Shot Studio

Thank you for looking at Shot Studio. A bug report, a fixed typo or a new feature all make the app better for the people who record with it.

This guide covers what you need to get a change merged.

---

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Ways to contribute](#ways-to-contribute)
- [Getting started](#getting-started)
- [Commit messages](#commit-messages)
- [Checks](#checks)
- [Test media](#test-media)
- [AI-assisted contributions](#ai-assisted-contributions)
- [Pull request process](#pull-request-process)
- [Reporting bugs](#reporting-bugs)
- [Suggesting features](#suggesting-features)

---

## Code of Conduct

Be kind, patient and constructive. The full text lives in [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md). Report violations to [hello@coderscantina.com](mailto:hello@coderscantina.com).

---

## Ways to contribute

- **Report a bug.** A clear report with steps to reproduce saves a lot of guessing.
- **Improve the docs.** If something in the briefing or a milestone is unclear, fix it.
- **Review pull requests.** Fresh eyes on a change help.
- **Write code.** Bug fixes, tests and features are welcome. For a feature, open an issue first so we can agree on scope.

---

## Getting started

### 1. Fork and clone

```bash
git clone https://github.com/<your-username>/shot-studio.git
cd shot-studio
```

### 2. Read the plan

Read [docs/README.md](docs/README.md) and the technical briefing it links to. The briefing owns scope, and the milestone index owns the order of work. A change that goes against either needs an issue first.

### 3. Build

Run `cargo build` from the repo root. Rust edition 2024 needs a current stable toolchain, which `rustup` installs.

### 4. Create a branch

Use a short, descriptive name with one of these prefixes: `feat/`, `fix/`, `docs/`, `refactor/`, `test/`, `chore/`.

```bash
git checkout -b fix/preview-stalls-on-seek
git checkout -b feat/prompter-keyboard-hotkeys
```

### 5. Make your change, then open a pull request

Keep one logical change in each pull request.

---

## Commit messages

We use **[Gitmoji](https://gitmoji.dev/)** for every commit. The git log then reads as a list of what changed.

### Format

```
<emoji> <short description in lowercase, imperative present tense>

[optional body]
```

### Rules

- Start every commit with exactly one gitmoji.
- Write the subject in plain lowercase, for example `✨ cut silences longer than the pacing allows`.
- Use the imperative, present tense: "add" not "added".
- Keep the subject line under **72 characters**.
- Separate the subject from the body with a blank line.

### Common emojis

| Emoji | Use for                  |
| ----- | ------------------------ |
| ✨    | New feature              |
| 🐛    | Bug fix                  |
| 📝    | Documentation            |
| ♻️    | Refactoring              |
| ⚡️    | Performance improvement  |
| ✅    | Adding tests             |
| 🏗️    | Architectural change     |
| 🔧    | Config and tooling       |
| 👷    | CI                       |
| 🗑️    | Removing code or files   |
| 🔥    | Removing dead code       |
| 💄    | UI and style changes     |
| 🎉    | Starting a project       |

Full reference: [gitmoji.dev](https://gitmoji.dev/)

### Example

```
✨ add the prompter scroll modes

Follow mode tracks the live transcript. Steady and pedal modes ignore it.
```

Commits that do not follow this format get a request to revise before merge.

---

## Checks

Run these before you push. CI runs the same three commands.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Clippy warnings fail the build, so fix them rather than adding `#[allow]`. Add an `allow` only with a comment that says why.

---

## Test media

Test media is generated. The fixtures come from code, so anyone can recreate them.

- Never commit a real recording, a screen capture, a voice sample or a model file.
- Never commit a token, a key or a `.shot/` cache directory.
- Generated fixtures go in `fixtures/generated/`, which git ignores.

If a test needs a new kind of media, add a generator for it instead of adding a file.

---

## AI-assisted contributions

We welcome AI-assisted contributions. Claude, Copilot, Cursor and similar tools are fine to use, as long as you say so.

### What we ask

**1. Disclose AI usage in your pull request.**

If an AI tool meaningfully shaped your change, say so in one line:

```
> AI assistance: Used Claude to draft the prompter scroll modes, then reviewed and adjusted manually.
```

Autocomplete and minor one-liners do not need a disclosure. If you would credit a human pair programmer, credit the AI.

**2. Understand what you submit.**

You are responsible for the code in your pull request. Before you open it, make sure you can explain what the code does, why it is correct and how you tested it.

**3. Do not submit AI output unreviewed.**

AI code often looks right and still has subtle bugs, security holes or style problems. Run the checks and read the diff.

**4. No invented dependencies.**

Check that every crate, function and API in AI-written code exists at the version we pin. Phantom imports are a common failure.

---

## Pull request process

1. **Open a draft early** if you want feedback before the work is done.
2. **Fill in the pull request template.** Describe why the change exists, not only what the diff does.
3. **Link the issue** with `Closes #123` or `Relates to #456`.
4. **Keep pull requests focused.** One logical change is easier to review and easier to revert.
5. **Respond to review comments.**
6. **Squash noise commits** such as "fix typo" before merge.

A pull request is ready to merge when:

- All CI checks pass.
- A maintainer has approved it.
- Conversations are resolved.

---

## Reporting bugs

Open a [bug report](https://github.com/coderscantina/shot-studio/issues/new/choose). Include:

- The Shot Studio version, or the commit SHA if you built from source
- Your macOS version and Mac model
- Steps to reproduce, as specific as you can make them
- What you expected and what happened
- Logs or screenshots if they help

Say whether the problem hits recording, preview, export or another area. That tells us where to look first.

---

## Suggesting features

Open a [feature request](https://github.com/coderscantina/shot-studio/issues/new/choose). A good request explains:

- The problem you want to solve, not only the solution you have in mind
- Who benefits and how
- Whether you would help build it

Check the existing issues first to avoid duplicates.

---

## Questions

If you are unsure about scope, approach or conventions, ask before you write code. Open an issue and we will answer it there. We would rather answer a question early than ask for a rewrite.

---

<div align="center">
  <p>Maintained by <a href="https://github.com/badmike">badmike</a></p>
</div>
