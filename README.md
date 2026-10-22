<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/3ae3ae/notepipe/main/docs/wordmark-dark.svg">
    <img src="https://raw.githubusercontent.com/3ae3ae/notepipe/main/docs/wordmark-light.svg" alt="notepipe" width="360">
  </picture>
</h1>

<p align="center">
  <strong>Turn video and audio into Markdown notes.</strong><br>
  The transcript in readable paragraphs, arranged around screenshots of what was on screen,<br>
  with an optional AI summary on top — driven by a single TOML pipeline you can rewire from end to end.
</p>

<p align="center">
  <a href="https://github.com/3ae3ae/notepipe/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/3ae3ae/notepipe/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://pypi.org/project/notepipe/"><img alt="PyPI" src="https://img.shields.io/pypi/v/notepipe"></a>
  <a href="https://crates.io/crates/notepipe"><img alt="crates.io" src="https://img.shields.io/crates/v/notepipe"></a>
  <a href="https://github.com/3ae3ae/notepipe/blob/main/LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
</p>

notepipe is a command-line tool and extensible engine that turns lectures, talks, meetings and podcasts — from local
files or URLs — into plain Markdown notes you can read, search and keep. It ships as one small Rust binary that works
out of the box with OpenAI and lets you swap every stage of the run, from the speech engine to the prompts, without
touching its code.

```sh
export OPENAI_API_KEY=sk-…
notepipe lecture.mp4
```

```text
Introduction-to-Compilers/
├── note.md        # the note: summary, then screenshots and paragraphs
├── note.json      # the proofread transcript and screenshots as structured data
└── images/        # 0001.jpg, 0002.jpg, …
```

```markdown
## Overview
A compiler translates a program in stages, from characters to tokens, trees and finally machine code.

![](<images/0001.jpg>)

The first stage is lexical analysis. The scanner reads characters and groups them into tokens such as identifiers,
numbers and operators.
```

## Highlights

- 🧩 **A pipeline you can see.** Downloading, transcription, screenshots, every AI step and the note's layout are
  stages in one TOML file. Each stage names the files it reads and the file it writes, so the whole flow is on the page,
  and every intermediate file is there to open. Rename, reorder, remove, skip or add stages at will.
- 🔌 **Any model, cloud or local.** Works out of the box with OpenAI (`whisper-1` and `gpt-6-luna`). Point
  `[providers]` at Anthropic, Gemini, Groq, Mistral, OpenRouter, Ollama, LM Studio or your own server — or run speech
  recognition locally with llama.cpp, OpenVINO (Intel NPU, CPU or GPU) or Apple silicon.
- 🛠️ **Extensible in any language, without forking.** Any command-line tool can be a stage, reading a stage's files and
  writing its output: pandoc, `jq` or a script of your own. Local engines and other APIs plug in as programs with small,
  documented contracts.
- 🧱 **A CLI tool and an engine at once.** Run it in a terminal, send a note to standard output for the next command,
  or drive it from your own application with `--config -`, JSON progress lines (`--json`), documented exit codes and
  `note.json`.
- 📦 **Runs anywhere.** One small Rust binary prebuilt for Linux and macOS (`x86_64`, `arm64`) and Windows (`x86_64`),
  tested on all three in CI, and buildable on Android in Termux. Its only runtime dependency is `ffmpeg`.
- 🛡️ **Safe, cached and resumable.** A note is built in full before it replaces the previous one, and a replacement
  cut short is completed or undone on the next run, so a note is never half-written or lost. Downloads, screenshots, transcripts and AI responses are cached, interrupted transcriptions resume where
  they stopped, and `Ctrl-C` stops every process notepipe started.
- 🔒 **Private.** No telemetry. Audio and text go only to the providers you configure, or nowhere at all with local
  engines.
- 📝 **Plain, durable output.** An ordinary folder of Markdown, JPEG screenshots and JSON that any editor, notes app or
  script can read, laid out the way your stages write it.

## Install

notepipe requires [ffmpeg](https://ffmpeg.org/download.html) on your `PATH` (`brew install ffmpeg`,
`sudo apt install ffmpeg`, `winget install ffmpeg`, or `pkg install ffmpeg` in Termux).

```sh
uv tool install "notepipe[ytdlp]"   # includes URL support via yt-dlp; use `notepipe` for local files only
```

| Method | Command / Source |
|---|---|
| **uv** (recommended) | `uv tool install "notepipe[ytdlp]"` |
| **pipx / pip** | `pipx install "notepipe[ytdlp]"` or `pip install "notepipe[ytdlp]"` |
| **Cargo** (no Python) | `cargo install notepipe` |
| **Prebuilt binaries** | [GitHub Releases](https://github.com/3ae3ae/notepipe/releases) — Linux and macOS (`x86_64`, `arm64`), Windows (`x86_64`) |
| **Android ([Termux](https://termux.dev))** | `pkg install rust ffmpeg python && cargo install notepipe` |

To include speech adapters when installing with `uv`, `pipx` or `pip`, list every extra you want in one command
(because `uv tool install` replaces the environment each time), for example
`uv tool install "notepipe[ytdlp,llama]"`. See
[Adapters](https://github.com/3ae3ae/notepipe/blob/main/README.md#adapters) for all extras.

## Quick start

```sh
export OPENAI_API_KEY=sk-…                        # default configuration uses OpenAI
notepipe lecture.mp4                              # local video -> ./<title>/note.md
notepipe https://youtu.be/VIDEO_ID                # URL, with notepipe[ytdlp]
notepipe interview.m4a -o ~/notes                 # audio-only input; write under ~/notes
notepipe talk.mp4 --subtitle talk.srt             # use an existing .srt or .vtt transcript
notepipe lecture.mp4 --skip proofread,summary     # no proofreading or summary for this run
```

The first run creates `config.toml` and prints its path. When subtitles exist — `lecture.srt` or `lecture.vtt` next to
a local file, or captions from the URL — notepipe uses them automatically. Otherwise speech is transcribed with
OpenAI's `whisper-1`, and the note is proofread and summarized with `gpt-6-luna`. The key is checked before any work
starts for every stage that might use it, including speech recognition as the transcript's fallback; to make a note
without any key, give the transcript yourself and skip the model stages:
`notepipe talk.mp4 --subtitle talk.srt --skip proofread,summary`.

Each note is written to a folder named after its title, with what the stages publish: by default `note.md`,
`note.json` and `images/`. Re-running on the same recording replaces its folder whole; a different recording with
the same title gets a `-2`, `-3`, … suffix. Run `notepipe doctor` to check your setup and `notepipe --help` to see all
options.

## How it works

Every stage reads files and writes one, and its table in `config.toml` says which. With the default configuration:

```text
source ─► download ─► media ─┬─► transcript ─► transcript.json ─┐
                             └─► screenshots ─► images/ ────────┴─► draft ─► draft.json
                                                                                 │
                         note.md ◄─ markdown ◄─ summary.md ◄─ summary ◄─ proofread.json ◄─ proofread
```

The files live in the recording's work folder, where you can open each one (`-v` prints where). A stage's `publish`
copies its output into the note folder, or to standard output. What a stage does comes from its `type`:

| Type | Reads → writes | What it does |
|---|---|---|
| `download` | the file or URL → media | Fetches a URL's media, subtitles and details with a download program (`notepipe-source-ytdlp` by default); a local file is used where it is |
| `transcript` | media → transcript | Uses subtitles when available, or recognizes speech through a provider |
| `screenshots` | media → images | Captures a frame each time the picture changes and settles |
| `document` | transcript + images → document | Groups the transcript under its screenshots, in paragraphs |
| `llm` | a file → `.json` or `.md` | Sends a file to a model with your prompt: writing `.json` rewrites each paragraph of a document (proofread, translate, …); writing `.md` answers once (summary, action items, a new layout, …) |
| `markdown` | documents and Markdown → Markdown | Joins them, in order, into the note's text |
| `run` | any files → a file | Runs any command-line program, such as pandoc or a script of your own |

Stages that do not depend on each other, such as the transcript and the screenshots, run at the same time. See
[docs/design.md](https://github.com/3ae3ae/notepipe/blob/main/docs/design.md) for the architecture and design principles.

## Configuration

```sh
notepipe config init    # create the configuration file with the default settings
notepipe config path    # print where it is
notepipe config show    # show the configuration in effect
```

notepipe reads `~/.config/notepipe/config.toml`, or `%APPDATA%\notepipe\config.toml` on Windows
(`$XDG_CONFIG_HOME/notepipe/config.toml` when set). `pipeline` lists the stages that run, in order; each stage has its
own `[stages.NAME]` table with what it reads (`input`), writes (`output`) and publishes (`publish`), and
`[providers.NAME]` defines the API endpoints or local adapter commands they share.

Settings at their default are written as `#` comments so you can see every value in effect at a glance. The default
prompts for `proofread` and `summary` are written out in full in the file and are yours to edit. Here is the generated
file with the two prompts shortened:

```toml
# Lines starting with # show the defaults.
# Every setting is described in https://github.com/3ae3ae/notepipe/blob/main/docs/configuration.md

pipeline = ["download", "transcript", "screenshots", "draft", "proofread", "summary", "markdown"]
# out = "."

[providers.openai]
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
# concurrency = 4

[stages.download]
type = "download"
input = "source"
output = "media"
command = ["notepipe-source-ytdlp"]

[stages.transcript]
type = "transcript"
input = "media"
output = "transcript.json"
# language = "auto"
# sources = ["subtitles", "speech"]

[stages.transcript.subtitles]
# automatic = true

[stages.transcript.speech]
provider = "openai"
model = "whisper-1"
# api = "transcriptions"
# threshold = 0.5
# min_silence = 0.4
# max_segment = 20.0

[stages.screenshots]
type = "screenshots"
input = "media"
output = "images"
# mode = "stable"
# sensitivity = 0.85
# frame_interval = 1.0
# min_gap = 10.0
# settle = 0.8

[stages.draft]
type = "document"
input = ["transcript.json", "images"]
output = "draft.json"

[stages.proofread]
type = "llm"
input = "draft.json"
output = "proofread.json"
publish = "note.json"
provider = "openai"
model = "gpt-6-luna"
# images = false
# batch_size = 20
prompt = '''…'''

[stages.summary]
type = "llm"
input = "proofread.json"
output = "summary.md"
provider = "openai"
model = "gpt-6-luna"
# chunk_size = 25000
prompt = '''…'''

[stages.markdown]
type = "markdown"
input = ["summary.md", "proofread.json"]
output = "note.md"
publish = "note.md"
```

Stage names and file names are yours to choose; behavior comes from `type`, and the kind of each file from the stage
that writes it. Remove a stage from `pipeline` to disable it while keeping its table, or pass `--skip NAMES` to leave
stages out for a single run: a skipped `proofread` hands `draft.json` on unchanged, and a skipped `summary` leaves the
summary out. Every setting is documented in [docs/configuration.md](https://github.com/3ae3ae/notepipe/blob/main/docs/configuration.md).

### Adding your own stages

Add a `[stages.NAME]` table, point its `input` at an earlier stage's `output`, and list its name in `pipeline`:

```toml
[stages.translate]                  # rewrites each paragraph; `raw` keeps the original text
type = "llm"
input = "proofread.json"
output = "translated.json"
provider = "openai"
model = "gpt-6-luna"
prompt = "Translate each item into Korean. Keep names, code and formulas unchanged."

[stages.action-items]               # one Markdown answer for the whole recording
type = "llm"
input = "proofread.json"
output = "actions.md"
provider = "openai"
model = "gpt-6-luna"
prompt = "List the decisions and action items in this meeting as a Markdown checklist, with owners where named."

[stages.docx]                       # a Word copy of the note, made by pandoc
type = "run"
input = "note.md"
output = "note.docx"
publish = "note.docx"
command = ["pandoc", "{input}", "-o", "{output}"]
```

Then put `actions.md` into the markdown stage's `input`, in the place where it should appear. An `llm` stage that
writes `.json` rewrites each paragraph and keeps times and screenshots; one that writes `.md` answers once. To lay the
note out your own way, let an `llm` or `run` stage write the Markdown instead of the `markdown` stage. A `run` stage
can also change a document without a model, such as replacing terms from a glossary
([docs/adapters.md](https://github.com/3ae3ae/notepipe/blob/main/docs/adapters.md#run-programs)).

### Switching models and providers

Any OpenAI-compatible API works: define a `[providers.NAME]` table and reference `provider = "NAME"` in your stages.
API keys are read only from the environment variable named in `api_key_env`.

| Service | `[providers.NAME]` | `model` |
|---|---|---|
| OpenAI (default) | `base_url = "https://api.openai.com/v1"`, `api_key_env = "OPENAI_API_KEY"` | `whisper-1`, `gpt-6-luna`, … |
| Anthropic | `base_url = "https://api.anthropic.com/v1"` | any Claude model |
| Google Gemini | `base_url = "https://generativelanguage.googleapis.com/v1beta/openai"` | any Gemini model |
| Groq | `base_url = "https://api.groq.com/openai/v1"` | `whisper-large-v3-turbo` (speech) or a chat model |
| Mistral | `base_url = "https://api.mistral.ai/v1"` | `voxtral-mini-latest` (speech) or a chat model |
| OpenRouter | `base_url = "https://openrouter.ai/api/v1"` | any model on OpenRouter |
| Ollama, LM Studio, custom server | its `base_url`, e.g. `http://localhost:11434/v1` | its served models |

Audio-capable chat models can transcribe speech when `api = "chat"` is set in `[stages.transcript.speech]`. Services
that do not speak the OpenAI API directly connect through a
[provider program](https://github.com/3ae3ae/notepipe/blob/main/docs/adapters.md#provider-programs).

## Adapters

Anything tied to a specific website, local runtime or non-OpenAI vendor lives outside the core in an **adapter**: a
standalone program that notepipe runs. Three contracts cover every extension point:

1. **Provider programs** start on demand and serve an OpenAI-compatible HTTP API on `localhost` (for local speech
   engines or custom API translators).
2. **Download programs** turn a URL into a local media file, metadata and subtitles.
3. **Run programs** read a stage's input files and write its output, through arguments or standard input and output.

Official adapters are published as Python extras of `notepipe`:

| Extra | Install | What it adds | Notes |
|---|---|---|---|
| [`ytdlp`](https://github.com/3ae3ae/notepipe/blob/main/adapters/ytdlp) | `uv tool install "notepipe[ytdlp]"` | URLs and subtitles from YouTube and every other site [yt-dlp](https://github.com/yt-dlp/yt-dlp) supports | used by the default `download` stage |
| [`llama`](https://github.com/3ae3ae/notepipe/blob/main/adapters/llama) | `uv tool install "notepipe[llama]"` | Local speech recognition via [llama.cpp](https://github.com/ggml-org/llama.cpp) on CPU or GPU | requires `llama-server` |
| [`openvino`](https://github.com/3ae3ae/notepipe/blob/main/adapters/openvino) | `uv tool install "notepipe[openvino]"` | Local speech recognition on an Intel NPU, CPU or GPU via [OpenVINO](https://github.com/openvinotoolkit/openvino.genai) | |
| [`speech-swift`](https://github.com/3ae3ae/notepipe/blob/main/adapters/speech-swift) | `uv tool install "notepipe[speech-swift]"` | Local speech recognition on Apple silicon with MLX or Core ML via [speech-swift](https://github.com/soniqo/speech-swift) | macOS 15+; built in CI, not yet verified on hardware |
| [`xai`](https://github.com/3ae3ae/notepipe/blob/main/adapters/xai) | `uv tool install "notepipe[xai]"` | xAI's speech-to-text API | |
| [`meta`](https://github.com/3ae3ae/notepipe/blob/main/adapters/meta) | `uv tool install "notepipe[meta]"` | Meta's speech-to-text API | |

To use a speech adapter, set `command` on a provider instead of `base_url`:

```toml
[providers.llama]
command = ["notepipe-asr-llama", "-hf", "ggml-org/Qwen3-ASR-1.7B-GGUF:Q8_0"]
concurrency = 1

[stages.transcript.speech]
provider = "llama"
```

Writing your own adapter in any language takes only a few dozen lines;
[docs/adapters.md](https://github.com/3ae3ae/notepipe/blob/main/docs/adapters.md) specifies all three contracts with
complete Python examples. Install third-party adapters alongside notepipe with
`uv tool install notepipe --with <package>`, or place the executable on your `PATH`.

## Using notepipe from other programs

The CLI is notepipe's programmatic interface. Pass the configuration on `stdin` (TOML or JSON) and stream progress as
JSON lines on `stdout`:

```sh
notepipe "$SOURCE" -o "$OUT" --config - --json < config.toml
```

The `done` event gives the note folder, each published file and the work folder. A stage with `publish = "-"` writes
its output to standard output instead, after the note is saved, so `notepipe talk.mp4 | glow` can show it in the
terminal. Event schemas, exit codes, the `note.json` format and caching rules are documented in
[docs/cli.md](https://github.com/3ae3ae/notepipe/blob/main/docs/cli.md).

## Project status

notepipe is young (`0.x`). The configuration format, CLI flags and `note.json` schema may still change between minor
releases; all breaking and user-visible changes are tracked in
[CHANGELOG.md](https://github.com/3ae3ae/notepipe/blob/main/CHANGELOG.md).

## Contributing

Bug reports, ideas and pull requests are welcome. See
[CONTRIBUTING.md](https://github.com/3ae3ae/notepipe/blob/main/CONTRIBUTING.md) for how to build, test and propose
changes, and [SECURITY.md](https://github.com/3ae3ae/notepipe/blob/main/SECURITY.md) for reporting security issues
privately.

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

notepipe is released under the [MIT License](https://github.com/3ae3ae/notepipe/blob/main/LICENSE), which keeps
course2md's copyright notice alongside notepipe's own.
