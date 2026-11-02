# AGENTS.md

Guidance for coding agents and contributors working in this repository.

## Project

notepipe is an extensible engine and command-line tool that turns video and audio into Markdown notes. Other programs can
build on it, and it is useful on its own.

## The core boundary

The core turns a **local media file** into a **note folder**, and talks to the outside world only through standard
protocols: ffmpeg for decoding, OpenAI-compatible HTTP APIs for speech recognition and language models, and the adapter
contracts in `docs/` for anything else.

Before adding code to `src/`, check it against this list. If any answer is yes, the code belongs in an adapter, in
configuration or in documentation — not in the core:

- Does it depend on a particular website, service vendor, model, operating system or package manager?
- Does it need a model download, a GPU runtime or a platform SDK?
- Does it decide what a note says (wording, structure, which content to drop), what it is called or where it lives,
  instead of leaving that to the user?
- Does it favor one language, region, script or notation, or change text that passes through it?
- Is it an option, mode or abstraction that no current caller needs?

## Layout

- `src/` — the engine library and CLI.
- `adapters/` — official adapters (speech recognition and media sources), each a separate package and an extra of
  `notepipe` in `pyproject.toml`.
- `tests/` — integration tests and fixtures.
- `docs/` — adapter contracts and the CLI protocol.

## Conventions

- Prefer the smallest complete change. Delete rather than keep; do not add options, abstractions or dependencies that the
  change does not need.
- CLI messages are English. Prompts are English defaults that users replace; the core's own format instructions are
  fixed and say nothing about content.
- Machine-readable output (`--json`, `note.json`) is a contract: keep field names stable.

## Before committing

Run the same checks as `.github/workflows/ci.yml`:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --lib
cargo test --test cli_ux --test task_execution
cargo test --features integration --test scene_synthetic   # needs ffmpeg; not run on Windows
```

Adapters under `adapters/` have their own tests; run them from the adapter's directory.

Write commit subjects as `area: summary` in the imperative mood, one concern per commit. Record user-visible changes under
`Unreleased` in `CHANGELOG.md`.
