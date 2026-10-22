# Contributing to notepipe

Bug reports, adapters and code changes are welcome.

## Reporting problems

Open an issue with the command you ran, what you expected and what happened. Run with `-v` (or `-vv`) and include the
relevant log lines. Remove API keys, cookies and private URLs first.

## Development setup

You need Rust (see `rust-version` in `Cargo.toml`) and `ffmpeg` on `PATH`. The Python adapters use
[uv](https://docs.astral.sh/uv/).

```sh
git clone https://github.com/3ae3ae/notepipe
cd notepipe
cargo build
cargo run -- --help
```

## Checks

Run the same checks as CI before opening a pull request:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --lib
cargo test --test cli_ux --test task_execution
cargo test --features integration --test scene_synthetic   # needs ffmpeg; not run on Windows
```

Each adapter under `adapters/` is a separate package with its own tests:

```sh
uv run --directory adapters/llama python -m unittest       # Python adapters
cargo test --manifest-path adapters/speech-swift/Cargo.toml # speech-swift adapter (macOS arm64 with Xcode)
```

## Project layout

| Path | Contents |
|---|---|
| `src/` | The engine and the `notepipe` CLI |
| `adapters/` | Official speech and download adapters, each a separate package |
| `tests/` | Integration tests and fixtures |
| `docs/` | [CLI contract](docs/cli.md) and [adapter contracts](docs/adapters.md) |

## What belongs in the core

The core turns a local media file into a note folder and reaches everything else through standard interfaces: ffmpeg,
OpenAI-compatible HTTP APIs and the adapter contracts. Code that depends on a particular website, vendor, model,
operating system or package manager, that needs local models or platform SDKs, or that decides how a note should read
belongs in an adapter, in configuration or in documentation instead. [AGENTS.md](AGENTS.md) has the full checklist.

Prefer the smallest complete change: delete rather than keep, and do not add options, abstractions or dependencies the
change does not need. CLI messages are English; prompts are English defaults that users can replace.

## Writing an adapter

See [docs/adapters.md](docs/adapters.md). Third-party adapters do not need to live in this repository; name the executable
`notepipe-asr-<name>`, `notepipe-ai-<name>` or `notepipe-source-<name>` and publish it as a package.

## Commits and pull requests

- One concern per commit, with a subject like `area: summary` in the imperative mood (for example
  `asr: retry on HTTP 429`), and the reason in the body.
- Add a test for new behavior.
- Record user-visible changes under `Unreleased` in [CHANGELOG.md](CHANGELOG.md).
- `main` changes only through pull requests, and nobody pushes tags. A pull request merges when the `ci` check passes and
  the branch is up to date with `main`; CI runs on pull requests only, so a merge does not run it again.

## Releases

A release is a pull request that changes the version in `Cargo.toml` (and moves `Unreleased` in the changelog under
that version). When it merges, `.github/workflows/release.yml` builds the CLI binaries and wheels and every adapter,
creates the tag `vX.Y.Z` and a GitHub release with the files and their checksums, and publishes the packages to PyPI
through trusted publishing. Only that workflow can create tags: it pushes them with the `RELEASE_DEPLOY_KEY` deploy
key, the one exception to the tag rules. A version that already has a tag is not released again. When a release fails,
re-run its failed jobs; running the workflow by hand releases `main`'s version if it has no tag yet. Versions of
adapters that are already on PyPI are skipped, so bump an adapter's version when it changes; adapters are published
with the next release of notepipe. Each PyPI project
publishes from its own GitHub environment, `pypi` for notepipe and `pypi-<package>` for an adapter, and its trusted
publisher on PyPI names that environment.

The same workflow publishes the crate to crates.io from the `crates-io` environment through trusted publishing, unless
that version is already there.

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
