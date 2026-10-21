# AGENTS.md

Guidance for coding agents and contributors working in this repository.

## Project

notepipe is an extensible engine and command-line tool that turns video and audio into Markdown notes. The core is deliberately light and API-first: speech recognition and language models are reached over HTTP endpoints. Local speech recognition is provided by separately installed adapters.

## Layout

- `src/` — the engine library and CLI.
- `adapters/` — official speech-recognition adapters, each a separate package.
- `i18n/catalog.tsv` — translations of human-readable CLI messages.
- `tests/` — integration tests and fixtures.
- `docs/` — protocol and adapter documentation.

## Conventions

- Keep the core light. Anything that needs local models, GPU runtimes or platform SDKs belongs in an adapter, not in the core.
- Prefer the smallest complete change. Do not add options, abstractions or dependencies that the change does not need.
- Human-readable CLI text goes through `tr!`, with its translations in `i18n/catalog.tsv`. Prompts sent to models and machine-readable output are not translated.
- Keep vendor-neutral defaults. Do not favor one region, language, website or provider in default behavior.

## Before committing

Run the same checks as `.github/workflows/ci.yml`:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --lib
cargo test --test cli_ux --test login_cli --test task_execution
cargo test --features integration --test scene_synthetic   # needs ffmpeg; not run on Windows
```

Write commit subjects as `area: summary` in the imperative mood, one concern per commit. Record user-visible changes under `Unreleased` in `CHANGELOG.md`.
