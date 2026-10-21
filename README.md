# notepipe

[![CI](https://github.com/3ae3ae/notepipe/actions/workflows/ci.yml/badge.svg)](https://github.com/3ae3ae/notepipe/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Turn video and audio into Markdown notes.** notepipe takes a lecture, talk, meeting or podcast and writes a note: the
transcript arranged around screenshots of what was on screen, optionally proofread, summarized or extended by the
language model of your choice.

It is a small, dependable command-line tool, and a component other programs can build on.

## Principles

- **One job, done well.** A recording goes in; a plain note folder comes out.
- **Standard interfaces, no lock-in.** Speech recognition and language models are reached through OpenAI-compatible APIs,
  so any provider, self-hosted server or local engine can be used.
- **Light by default.** No speech recognition or language models are bundled. Local engines and website support are
  separate [adapters](docs/adapters.md) that you install only if you want them.
- **Neutral.** No preferred vendor, website or language. Prompts are defaults you can replace, and notes follow the
  language of the recording.
- **Your data stays yours.** Plain Markdown, images and JSON; a failed run never replaces a good note; nothing is sent
  anywhere you did not configure.

[docs/design.md](docs/design.md) explains these choices and how the pieces fit together.

## What a note looks like

```markdown
---
title: "Introduction to Compilers"
source: "https://www.youtube.com/watch?v=…"
author: "Example University"
duration: "1:12:40"
created: "2026-10-21T09:30:00Z"
generator: "notepipe 0.1.0"
---

# Introduction to Compilers

## Overview
A compiler translates a program in stages, from characters to tokens, trees and finally machine code.

## [03:12](https://www.youtube.com/watch?v=…&t=192s)

![03:12](images/0002.jpg)

The first stage is lexical analysis. The scanner reads characters and groups them into tokens such as identifiers,
numbers and operators.
```

Each note is a folder with `note.md`, the screenshots in `images/`, and `note.json` with the same content as data.

## Install

notepipe needs [ffmpeg](https://ffmpeg.org/download.html) on your `PATH`.

With [uv](https://docs.astral.sh/uv/), run one of these:

```sh
uv tool install notepipe                  # local video and audio files
uv tool install "notepipe[ytdlp]"         # the same, plus URLs through yt-dlp
uv tool install "notepipe[ytdlp,llama]"   # the same, plus local speech recognition with llama.cpp
```

[Adapters](#adapters) are extras. Running the command again replaces the installation, including its extras and `--with`
packages, so list everything you want each time; `uv tool list --show-extras --show-with` shows the current set. pip and
pipx accept the same names.

With Cargo: `cargo install notepipe`. Prebuilt binaries are attached to each
[GitHub release](https://github.com/3ae3ae/notepipe/releases). On Android, install [Termux](https://termux.dev), run
`pkg install rust ffmpeg python`, then `cargo install notepipe`, and add adapters with `pip install <package>`.

## Quick start

```sh
notepipe lecture.mp4
notepipe interview.m4a -o ~/notes
notepipe https://www.youtube.com/watch?v=VIDEO_ID        # with a source adapter
```

Existing subtitles are used when available. Otherwise speech is transcribed: if no speech service is configured yet, the
first interactive run asks for one and saves it. `notepipe doctor` checks your setup.

## Configuration

Settings live in `~/.config/notepipe/config.toml` (`%APPDATA%\notepipe\config.toml` on Windows). `notepipe config init`
writes a commented template. The full list of settings is in [docs/configuration.md](docs/configuration.md).

```toml
[asr]                                      # speech recognition
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini-transcribe"
language = "ko"                            # optional; detected when empty

[llm]                                      # connection shared by all AI stages
base_url = "https://api.openai.com/v1"
model = "gpt-5.6-luna"

[[llm.stages]]                             # run in this order
name = "proofread"

[[llm.stages]]
name = "summary"
model = "gpt-5.6-terra"                    # any stage can use its own model, endpoint, key or prompt
request = { reasoning_effort = "medium" }  # extra fields merged into the request
```

API keys can stay out of the file: `NOTEPIPE_ASR_API_KEY` and `NOTEPIPE_LLM_API_KEY` are used when the file has none.

### Speech recognition

| Service | `[asr]` settings |
|---|---|
| OpenAI | `base_url = "https://api.openai.com/v1"`, `model = "gpt-4o-mini-transcribe"` |
| Groq | `base_url = "https://api.groq.com/openai/v1"`, `model = "whisper-large-v3-turbo"` |
| Mistral | `base_url = "https://api.mistral.ai/v1"`, `model = "voxtral-mini-latest"` |
| Audio-capable chat models | `mode = "chat"` with the provider's `base_url` and `model` |
| Any OpenAI-compatible server you run | its `base_url` |
| xAI, Meta | the [xAI](adapters/xai) or [Meta](adapters/meta) adapter |
| llama.cpp, Intel NPU, Apple silicon (local) | the [llama](adapters/llama), [OpenVINO](adapters/openvino) or [Apple](adapters/apple) adapter |

### AI stages

A stage sends either the note's paragraphs or the whole transcript to a language model, with a prompt that says what to
do. `proofread` and `summary` are built in. Anything else is a few lines of configuration — for example, bilingual notes:

```toml
[[llm.stages]]
name = "translate"
input = "paragraphs"       # each paragraph …
output = "below"           # … gets the model's text underneath
prompt = "Translate each item into Korean. Keep names, code and formulas unchanged. Return an item unchanged if it is already in Korean."
```

Any OpenAI-compatible chat endpoint works, including OpenAI, Anthropic (`https://api.anthropic.com/v1`), Google Gemini
(`https://generativelanguage.googleapis.com/v1beta/openai`), OpenRouter, Groq, and local servers such as Ollama
(`http://localhost:11434/v1`) or LM Studio.

## Adapters

Adapters are separate packages with programs that notepipe starts when needed. The official ones are extras of
`notepipe`, for example `uv tool install "notepipe[llama]"`; any other adapter installs next to notepipe with
`uv tool install notepipe --with <package>`.

| Extra | Package | Kind | Purpose |
|---|---|---|---|
| `ytdlp` | [`notepipe-source-ytdlp`](adapters/ytdlp) | source | URLs from any site yt-dlp supports |
| `llama` | [`notepipe-asr-llama`](adapters/llama) | speech | llama.cpp on CPU or GPU |
| `openvino` | [`notepipe-asr-openvino`](adapters/openvino) | speech | Intel NPU (or CPU/GPU) with OpenVINO |
| `apple` | [`notepipe-asr-apple`](adapters/apple) | speech | Apple silicon, macOS 15 or later; built in CI, not yet verified on hardware |
| `xai` | [`notepipe-asr-xai`](adapters/xai) | speech | xAI speech-to-text |
| `meta` | [`notepipe-asr-meta`](adapters/meta) | speech | Meta speech-to-text |

Writing your own takes a few dozen lines; see [docs/adapters.md](docs/adapters.md).

## Using notepipe from other programs

Pass the configuration on standard input and read newline-delimited JSON events:

```sh
notepipe "$SOURCE" -o "$OUT" --config - --json < config.toml
```

[docs/cli.md](docs/cli.md) documents the events, exit codes, `note.json`, caching and the completion hook that runs after
each note is written.

## Status

notepipe is young (0.x). The configuration format, the CLI contract and `note.json` may still change; every change is
recorded in the [changelog](CHANGELOG.md).

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md), and report security problems as described in
[SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE). notepipe started as a fork of [course2md](https://github.com/mizorewww/course2md) by mizorewww.
