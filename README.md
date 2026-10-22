<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/3ae3ae/notepipe/main/docs/wordmark-dark.svg">
    <img src="https://raw.githubusercontent.com/3ae3ae/notepipe/main/docs/wordmark-light.svg" alt="notepipe" width="360">
  </picture>
</h1>

<p align="center">
  <strong>Turn video and audio into Markdown notes.</strong><br>
  The transcript in readable paragraphs, arranged around screenshots of what was on screen,<br>
  optionally proofread, summarized or extended by the AI model of your choice.
</p>

<p align="center">
  <a href="https://github.com/3ae3ae/notepipe/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/3ae3ae/notepipe/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://pypi.org/project/notepipe/"><img alt="PyPI" src="https://img.shields.io/pypi/v/notepipe"></a>
  <a href="https://crates.io/crates/notepipe"><img alt="crates.io" src="https://img.shields.io/crates/v/notepipe"></a>
  <a href="https://github.com/3ae3ae/notepipe/blob/main/LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
</p>

```sh
notepipe lecture.mp4
```

```markdown
---
title: "Introduction to Compilers"
source: "https://www.youtube.com/watch?v=…"
author: "Example University"
duration: "1:12:40"
created: "2026-10-21T09:30:00Z"
---

# Introduction to Compilers

## Overview
A compiler translates a program in stages, from characters to tokens, trees and finally machine code.

## [03:12](https://www.youtube.com/watch?v=…&t=192s)

![03:12](images/0002.jpg)

The first stage is lexical analysis. The scanner reads characters and groups them into tokens such as identifiers,
numbers and operators.
```

notepipe works on lectures, talks, meetings and podcasts. It is a dependable command-line tool on its own, and a
component that other programs can build on.

## Why notepipe

- **A pipeline you can read.** Every step, from downloading to writing the note, is a stage in the configuration, in
  the order it runs. Take one out, change its settings, or add your own: what the AI does is a prompt you write, a
  filter stage runs your own program on the note, and a hook stage runs one after it.
- **Extensible without forking.** Websites, local speech engines and vendor APIs are *adapters*: small separate programs
  with a documented contract, written in any language, installed as extras and named in the configuration.
- **Portable.** A single binary for Linux and macOS on x86_64 and arm64 and for Windows on x86_64, and it builds on
  Android through Termux. Its only requirement is ffmpeg. Speech recognition and AI go through OpenAI-compatible APIs.
  The configuration it creates uses OpenAI, and any other provider, a server of your own or a local engine is a change
  to that file; the engine itself has no vendor built in.
- **Reliable.** A note is built completely before it replaces the previous one, so a failed or interrupted run never
  leaves a half-written or lost note. Finished work is cached: downloads, screenshots, transcripts and AI responses are
  reused, and an interrupted transcription resumes where it stopped. Ctrl-C stops every process notepipe started. Every
  code change is tested in CI on Linux, macOS and Windows.
- **Plain, durable output.** Markdown, JPEG and JSON in an ordinary folder, readable by any editor, notes app or script.
- **Private.** No telemetry. Audio and text go only to the providers the configuration names, or nowhere at all with
  local adapters.

[docs/design.md](https://github.com/3ae3ae/notepipe/blob/main/docs/design.md) describes the design and the principles behind it.

## Contents

[Installation](#installation) · [Quick start](#quick-start) · [Configuration](#configuration) ·
[Speech recognition](#speech-recognition) · [Stages and prompts](#stages-and-prompts) · [Adapters](#adapters) ·
[The note](#the-note) · [Using notepipe from other programs](#using-notepipe-from-other-programs) ·
[Acknowledgements](#acknowledgements)

## Installation

notepipe needs [ffmpeg](https://ffmpeg.org/download.html) on your `PATH` (`brew install ffmpeg`, `sudo apt install
ffmpeg`, `winget install ffmpeg`, or `pkg install ffmpeg` in Termux).

Install it with [uv](https://docs.astral.sh/uv/), choosing the extras for what you need:

| You want notes from | Install |
|---|---|
| local video and audio files | `uv tool install notepipe` |
| … and URLs (YouTube and the other sites yt-dlp supports) | `uv tool install "notepipe[ytdlp]"` |
| … with local speech recognition through llama.cpp | `uv tool install "notepipe[llama]"` |
| … with local speech recognition on an Intel NPU, CPU or GPU | `uv tool install "notepipe[openvino]"` |
| … with local speech recognition on Apple silicon | `uv tool install "notepipe[speech-swift]"` |
| … with xAI speech-to-text | `uv tool install "notepipe[xai]"` |
| … with Meta speech-to-text | `uv tool install "notepipe[meta]"` |
| any combination | `uv tool install "notepipe[ytdlp,llama]"` |

Running `uv tool install` again replaces the installation, including its extras and `--with` packages, so list
everything you want each time; `uv tool list --show-extras --show-with` shows the current set. pip and pipx accept the
same extras.

Other ways to install:

- **Cargo:** `cargo install notepipe` installs the CLI; add adapters with pip or uv.
- **Prebuilt binaries:** every [GitHub release](https://github.com/3ae3ae/notepipe/releases) has binaries and
  checksums for Linux, macOS and Windows.
- **Android:** in [Termux](https://termux.dev), run `pkg install rust ffmpeg python`, then `cargo install notepipe`;
  add adapters with `pip install notepipe-source-ytdlp` and the like.

| Platform | Architectures | Install with | Tested |
|---|---|---|---|
| Linux | x86_64, arm64 | uv, pip, pipx, Cargo, binary | in CI (x86_64) and containers (arm64) |
| macOS | arm64, x86_64 | uv, pip, pipx, Cargo, binary | in CI (arm64) |
| Windows | x86_64 | uv, pip, pipx, Cargo, binary | in CI |
| Android (Termux) | arm64 | Cargo, pip | in a Termux container |

## Quick start

```sh
export OPENAI_API_KEY=sk-…                  # the default configuration uses OpenAI
notepipe lecture.mp4                        # writes ./<title>/note.md
notepipe interview.m4a -o ~/notes           # audio works too; notes go under ~/notes
notepipe talk.mp4 --subtitle talk.srt       # use a transcript you already have
notepipe https://youtu.be/VIDEO_ID           # a URL, with notepipe[ytdlp]
notepipe lecture.mp4 --skip proofread,summary   # without the AI stages
```

The first run creates the configuration file and says where it is. Subtitles are used when they exist: `lecture.srt` or
`lecture.vtt` next to the file, or those of the video at the URL. Otherwise speech is transcribed with OpenAI's
`whisper-1`, and the note is proofread and summarized with `gpt-6-luna`. `notepipe doctor` checks your setup, and
`notepipe --help` explains every option.

## Configuration

```sh
notepipe config init    # create the configuration file with the default settings
notepipe config path    # print where it is
notepipe config show    # show the configuration in effect
```

The file is `~/.config/notepipe/config.toml`, or `%APPDATA%\notepipe\config.toml` on Windows (`$XDG_CONFIG_HOME/notepipe/`
when that is set). `pipeline` lists the stages that run, in order; each stage's settings are in its own
`[stages.NAME]` table, and `[providers.NAME]` holds the connections they share. Every default value is written out, so
what the file says is what runs. This is the file notepipe creates, with the two prompts shortened:

```toml
# Every setting is described in https://github.com/3ae3ae/notepipe/blob/main/docs/configuration.md

pipeline = ["download", "transcript", "screenshots", "proofread", "summary", "note"]

[providers.openai]
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
concurrency = 4

[stages.download]
type = "download"
command = "notepipe-source-ytdlp"

[stages.transcript]
type = "transcript"
language = "auto"
sources = ["subtitles", "speech"]

[stages.transcript.subtitles]
sidecar = true
downloaded = true
automatic = true

[stages.transcript.speech]
provider = "openai"
api = "transcriptions"
model = "whisper-1"
threshold = 0.5
min_silence = 0.4
max_segment = 20.0

[stages.screenshots]
type = "screenshots"
mode = "stable"
sensitivity = 0.85
frame_interval = 1.0
min_gap = 10.0
settle = 0.8

[stages.proofread]
type = "rewrite"
provider = "openai"
model = "gpt-6-luna"
batch_size = 20
prompt = '''…'''

[stages.summary]
type = "add-top"
provider = "openai"
model = "gpt-6-luna"
chunk_size = 25000
prompt = '''…'''

[stages.note]
type = "note"
dir = "."
```

Take a stage out of `pipeline` to turn it off without losing its settings, or leave it out for one run with
`--skip NAME`. [docs/configuration.md](https://github.com/3ae3ae/notepipe/blob/main/docs/configuration.md) lists every setting.

## Speech recognition

Speech is located on your machine, cut into short segments and sent to the provider of the transcript stage's
`[stages.transcript.speech]`.

| Service | Provider | `[stages.transcript.speech]` |
|---|---|---|
| OpenAI (the default) | `base_url = "https://api.openai.com/v1"` | `model = "whisper-1"` |
| Groq | `base_url = "https://api.groq.com/openai/v1"`, `api_key_env = "GROQ_API_KEY"` | `model = "whisper-large-v3-turbo"` |
| Mistral | `base_url = "https://api.mistral.ai/v1"`, `api_key_env = "MISTRAL_API_KEY"` | `model = "voxtral-mini-latest"` |
| Audio-capable chat models | the provider's `base_url` | `api = "chat"` and the `model` |
| Any OpenAI-compatible server you run | its `base_url` | its `model`, if it needs one |
| xAI, Meta | the [`xai`](#xai--xai-speech-to-text) or [`meta`](#meta--meta-speech-to-text) adapter | |
| llama.cpp, Intel NPU, Apple silicon (on your machine) | the [`llama`](#llama--llamacpp), [`openvino`](#openvino--intel-npu-cpu-or-gpu) or [`speech-swift`](#speech-swift--apple-silicon) adapter | |

`sources = ["speech"]` in the transcript stage always uses speech recognition; `sources = ["subtitles"]` uses subtitles
only.

## Stages and prompts

A stage of type `rewrite`, `add-below` or `add-top` sends the note to an OpenAI-compatible chat model with a prompt and
puts the answer back into the note:

| Type | The model reads | Its answer |
|---|---|---|
| `rewrite` | each paragraph | replaces the paragraph |
| `add-below` | each paragraph | is added below the paragraph |
| `add-top` | the whole transcript | is added at the top of the note |

The configuration starts with two: `proofread` corrects recognition errors and removes filler, and `summary` adds an
overview, key points and a timed outline. They are ordinary stages, with their prompts written out in the file.
**Every stage is yours to change:** rewrite a prompt, reorder them in `pipeline`, take out the ones you do not want, or
add your own in a few lines and list its name in `pipeline`:

```toml
[stages.translate]
type = "add-below"
provider = "openai"
model = "gpt-6-luna"
prompt = "Translate each item into Korean. Keep names, code and formulas unchanged."

[stages.action-items]
type = "add-top"
provider = "openai"
model = "gpt-6-luna"
prompt = "List the decisions and action items in this meeting as a Markdown checklist, with owners where named."
```

Any OpenAI-compatible chat service works, including OpenAI, Anthropic (`https://api.anthropic.com/v1`), Google Gemini
(`https://generativelanguage.googleapis.com/v1beta/openai`), OpenRouter, Groq, and local servers such as Ollama
(`http://localhost:11434/v1`) or LM Studio; give it a `[providers.NAME]` table. Anything else, such as the OpenAI
Responses API or access through a subscription, connects through a [provider program](https://github.com/3ae3ae/notepipe/blob/main/docs/adapters.md#provider-programs).
Processing that needs no model, such as fixed replacements, is a `filter` stage that runs a program of your own on the
note ([docs/adapters.md](https://github.com/3ae3ae/notepipe/blob/main/docs/adapters.md#filter-programs)).

## Adapters

Adapters connect notepipe to things it does not do itself: downloading from websites, and speech engines or AI services
that are not OpenAI-compatible. Each is a separate package with a small program that notepipe starts when needed. The
official adapters are extras of the `notepipe` package; install several at once with `uv tool install
"notepipe[ytdlp,llama]"`. A speech adapter is a provider whose `command` is the adapter; point the transcript stage's
speech at it.

### `ytdlp` — URLs

Downloads from YouTube and the other sites [yt-dlp](https://github.com/yt-dlp/yt-dlp) supports, with the video's
subtitles and details. It honors your yt-dlp configuration, and the default configuration's download stage uses it.

```sh
uv tool install "notepipe[ytdlp]"
```

### `llama` — llama.cpp

Runs a speech model such as Qwen3-ASR with [llama.cpp](https://github.com/ggml-org/llama.cpp)'s `llama-server`, on a
CPU or GPU. Requires `llama-server`.

```sh
uv tool install "notepipe[llama]"
```

```toml
[providers.llama]
command = "notepipe-asr-llama"
args = ["-hf", "ggml-org/Qwen3-ASR-1.7B-GGUF:Q8_0"]
concurrency = 1

[stages.transcript.speech]
provider = "llama"
```

### `openvino` — Intel NPU, CPU or GPU

Runs Qwen3-ASR or Whisper with [OpenVINO](https://github.com/openvinotoolkit/openvino.genai), on an Intel NPU by
default.

```sh
uv tool install "notepipe[openvino]"
```

```toml
[providers.openvino]
command = "notepipe-asr-openvino"  # add args = ["--device", "CPU"] (or "GPU") to choose the device
concurrency = 1

[stages.transcript.speech]
provider = "openvino"
```

### `speech-swift` — Apple silicon

Runs Qwen3-ASR on MLX, or Qwen3-ASR 0.6B and Whisper on Core ML, with
[speech-swift](https://github.com/soniqo/speech-swift). macOS 15 or later on Apple silicon. Built in CI; not yet verified
on hardware.

```sh
uv tool install "notepipe[speech-swift]"
```

```toml
[providers.speech-swift]
command = "notepipe-asr-speech-swift"
concurrency = 1

[stages.transcript.speech]
provider = "speech-swift"
```

### `xai` — xAI speech-to-text

Connects notepipe to [xAI's speech-to-text API](https://docs.x.ai/developers/model-capabilities/audio/speech-to-text).

```sh
uv tool install "notepipe[xai]"
```

```toml
[providers.xai]
command = "notepipe-asr-xai"
api_key_env = "XAI_API_KEY"

[stages.transcript.speech]
provider = "xai"
model = "grok-voice-transcribe-2.0"
```

### `meta` — Meta speech-to-text

Connects notepipe to [Meta's speech-to-text API](https://dev.meta.ai/docs/speech-to-text).

```sh
uv tool install "notepipe[meta]"
```

```toml
[providers.meta]
command = "notepipe-asr-meta"
api_key_env = "META_API_KEY"

[stages.transcript.speech]
provider = "meta"
model = "muse-voice-transcribe-1.0"
```

### Your own

An adapter is any program that follows a short contract, so it can be written in any language; a complete example in
Python takes a few dozen lines. [docs/adapters.md](https://github.com/3ae3ae/notepipe/blob/main/docs/adapters.md) describes every kind: provider, download, filter
and hook programs.
Install one next to notepipe with `uv tool install notepipe --with <package>`, or put it on your `PATH`.

## The note

Each note is a folder named after the title:

```
Introduction to Compilers/
├── note.md        Markdown with YAML front matter
├── note.json      the same content as data
└── images/        0001.jpg, 0002.jpg, …
```

A later run on the same recording replaces its folder; a different recording with the same title gets a `-2`, `-3`, …
suffix, and a recording whose title has changed gets a folder for its new title. Each section's time links to the recording, at that moment where the source allows it.

## Using notepipe from other programs

The command line is notepipe's integration interface. Pass settings on standard input and read progress as JSON lines:

```sh
notepipe "$SOURCE" -o "$OUT" --config - --json < config.toml
```

[docs/cli.md](https://github.com/3ae3ae/notepipe/blob/main/docs/cli.md) documents the events, exit codes, `note.json` and caching.
A [hook stage](https://github.com/3ae3ae/notepipe/blob/main/docs/adapters.md#hook-programs) runs your own program after each note is written.

## Project status

notepipe is young (0.x). The configuration format, the command-line interface and `note.json` may still change; every
change is recorded in the [changelog](https://github.com/3ae3ae/notepipe/blob/main/CHANGELOG.md).

## Contributing

Issues and pull requests are welcome; [CONTRIBUTING.md](https://github.com/3ae3ae/notepipe/blob/main/CONTRIBUTING.md) explains how to build, test and propose a
change. Please report security problems privately, as described in [SECURITY.md](https://github.com/3ae3ae/notepipe/blob/main/SECURITY.md).

## Acknowledgements

notepipe began as a fork of **[course2md](https://github.com/mizorewww/course2md)** by
[mizorewww](https://github.com/mizorewww). course2md showed how useful slide-illustrated notes made from recordings can
be, and notepipe still stands on its foundations: the screenshot detection, subtitle handling and timeline alignment at
the heart of notepipe were adapted from it. Thank you to mizorewww and to everyone who
[contributed to course2md](https://github.com/mizorewww/course2md/graphs/contributors). If you are looking for a desktop
app with a notes library, course2md is the place to go.

course2md's screenshot detection follows [yt-slide-mark](https://github.com/exopoiesis/yt-slide-mark) by exopoiesis.
notepipe is also built on [FFmpeg](https://ffmpeg.org), [earshot](https://crates.io/crates/earshot) for speech
detection, and, through its adapters, [yt-dlp](https://github.com/yt-dlp/yt-dlp),
[llama.cpp](https://github.com/ggml-org/llama.cpp), [OpenVINO](https://github.com/openvinotoolkit/openvino.genai) and
[speech-swift](https://github.com/soniqo/speech-swift).

## License

notepipe is released under the [MIT License](https://github.com/3ae3ae/notepipe/blob/main/LICENSE), which keeps course2md's copyright notice alongside notepipe's
own.
