<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/3ae3ae/notepipe/main/docs/wordmark-dark.svg">
    <img src="https://raw.githubusercontent.com/3ae3ae/notepipe/main/docs/wordmark-light.svg" alt="notepipe" width="360">
  </picture>
</h1>

<p align="center">
  <strong>A programmable video-and-audio-to-Markdown pipeline — and a standalone CLI that works out of the box.</strong>
</p>

<p align="center">
  <a href="https://github.com/3ae3ae/notepipe/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/3ae3ae/notepipe/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://pypi.org/project/notepipe/"><img alt="PyPI" src="https://img.shields.io/pypi/v/notepipe"></a>
  <a href="https://crates.io/crates/notepipe"><img alt="crates.io" src="https://img.shields.io/crates/v/notepipe"></a>
  <a href="https://github.com/3ae3ae/notepipe/blob/main/LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
</p>

Point `notepipe` at a lecture, talk, meeting or podcast and it writes a plain folder of Markdown, slide screenshots and
structured JSON: a summary at the top, followed by the spoken transcript grouped into paragraphs under the frame that
was on screen when each passage was spoken.

Look inside and almost nothing is fixed. Downloading, transcription, screenshot capture, timeline assembly, AI
rewriting, summarization and Markdown rendering are separate stages wired together in one TOML file. Every stage names
the file it reads and the file it writes, any command-line program can be a stage, any stage can stream its output to
`stdout`, and other programs can drive the binary directly over `stdin` and JSON lines. The core is a single Rust
binary whose only runtime dependency is `ffmpeg` — running on Linux, macOS, Windows, and Android in Termux.

```sh
export OPENAI_API_KEY=sk-…
notepipe lecture.mp4
```

```text
Introduction-to-Compilers/
├── note.md          # summary + screenshot-illustrated paragraphs
├── note.json        # structured document (metadata, sections, timed paragraphs, raw text)
└── images/          # 0001.jpg, 0002.jpg, … captured when the screen changed and settled
```

```markdown
## Overview
A compiler translates source code in stages, from characters to tokens, syntax trees and machine code.

![](<images/0001.jpg>)

We begin with lexical analysis. The scanner reads raw characters and groups them into tokens such as
identifiers, literals and operators.
```

---

## Why notepipe

### 1. A transparent pipeline of real files
A run is a dependency graph of stages listed in `pipeline`. Each `[stages.NAME]` table declares what it reads (`input`),
what it writes (`output`) and whether its output is copied into the final note (`publish`):

```text
source ─► download ─► media ─┬─► transcript ─► transcript.json ─┐
                             └─► screenshots ─► images/ ────────┴─► draft ─► draft.json
                                                                                 │
                         note.md ◄─ markdown ◄─ summary.md ◄─ summary ◄─ proofread.json ◄─ proofread
```

Every intermediate file lives in the recording's work directory (`notepipe -v` prints its path). You can open
`transcript.json`, inspect `draft.json` with `jq`, or compare `draft.json` against `proofread.json` to see every word a
model changed. Independent stages — such as `transcript` and `screenshots` — run at the same time, and the entire graph
is type-checked before any media or network work begins.

### 2. Almost nothing is hardcoded
Even `proofread` and `summary` are not built-in features: they are two `type = "llm"` stages in your `config.toml` with
plain-text prompts you can edit, duplicate or delete. You can rewire the graph to translate paragraphs, generate study
questions, run a Python or `jq` script between stages, replace the default Markdown assembly with your own layout, or
call `pandoc` to publish a `.docx` file alongside `note.md`.

### 3. Composes with standard Unix tools
Inside the pipeline, a `type = "run"` stage turns any command-line tool into a pipeline step, piping a file in on
`stdin` and capturing `stdout` (or passing paths via `{input}` and `{output}`) without invoking a shell. At the end of
the pipeline, setting `publish = "-"` on a stage streams its output to standard output so `notepipe` drops straight into
shell pipelines:

```sh
notepipe talk.mp4 | glow
```

### 4. Both a CLI tool and an embeddable engine
The CLI is also a strict programmatic interface. Applications and scripts can pass a complete TOML or JSON pipeline on
`stdin` (`--config -`), read real-time progress, token counts and logs as newline-delimited JSON (`--json`), and consume
the structured `note.json` output (`schema` 3) once the `done` event arrives.

### 5. One binary for Linux, macOS, Windows — and Termux on Android
The core binary compiles neural voice activity detection ([earshot](https://crates.io/crates/earshot)) and perceptual
frame comparison (SSIM) directly into one native executable. It bundles no speech-recognition or language models, no
Python runtime and no platform SDKs. Anything tied to a specific website, local inference runtime or non-OpenAI API lives outside the core in
a standalone **adapter** process.

---

## Install

`notepipe` requires [`ffmpeg`](https://ffmpeg.org/download.html) (including `ffprobe`) on your `PATH`:
`brew install ffmpeg` on macOS, `sudo apt install ffmpeg` on Debian/Ubuntu, `winget install ffmpeg` on Windows, or
`pkg install ffmpeg` in Termux.

| Way to install | Command |
|---|---|
| **uv** | `uv tool install notepipe` (or `uv tool install "notepipe[ytdlp]"` with the URL adapter) |
| **pipx / pip** | `pipx install notepipe` or `pip install notepipe` |
| **Cargo** (pure Rust, no Python) | `cargo install notepipe` |
| **Prebuilt binaries** | [GitHub Releases](https://github.com/3ae3ae/notepipe/releases) — Linux (`x86_64`, `aarch64`), macOS (`arm64`, `x86_64`), Windows (`x86_64`) |
| **Android ([Termux](https://termux.dev))** | `pkg install rust ffmpeg && cargo install notepipe` |

Because `uv tool install` replaces the tool's environment each time it runs, list all the Python adapter extras you
want in a single command — for example `uv tool install "notepipe[ytdlp,llama]"`.

---

## Getting started

### Local video and audio files

Out of the box, the default configuration processes local video and audio files using OpenAI (`whisper-1` for speech
recognition and `gpt-6-luna` for proofreading and summarization) with the API key in `OPENAI_API_KEY`:

```sh
export OPENAI_API_KEY=sk-…

notepipe lecture.mp4                              # writes ./<title>/note.md, note.json and images/
notepipe podcast.m4a -o ~/notes                   # audio-only input (no screenshots); write under ~/notes
notepipe seminar.mp4 --language ko                # prefer Korean captions or pass `ko` to speech recognition
notepipe talk.mp4 --subtitle talk.srt             # use an existing .srt or .vtt file as the transcript
notepipe lecture.mp4 --skip proofread,summary     # no proofreading or summary for this run
```

On the first run, `notepipe` creates `config.toml` and prints its path. When sidecar subtitles sit next to a local file
(`lecture.srt`, `lecture.vtt`, `lecture.en.srt`, …), the `transcript` stage uses them automatically instead of calling
speech recognition.

Every provider key used by an active stage is checked before any work starts. To build a note with no API key at all,
supply a subtitle file and skip the model stages:

```sh
notepipe talk.mp4 --subtitle talk.srt --skip proofread,summary
```

### Online URLs (`notepipe[ytdlp]`)

The core binary handles local files directly; fetching URLs is delegated to a download program configured on
`[stages.download]`. To run `notepipe` on YouTube or any other site supported by
[yt-dlp](https://github.com/yt-dlp/yt-dlp):

1. Install the `notepipe[ytdlp]` extra:
   ```sh
   uv tool install "notepipe[ytdlp]"
   ```
2. Add `command = ["notepipe-source-ytdlp"]` to `[stages.download]` in your `config.toml` (find the file with
   `notepipe config path`):
   ```toml
   [stages.download]
   type = "download"
   input = "source"
   output = "media"
   command = ["notepipe-source-ytdlp"]
   ```
3. Pass any URL on the command line:
   ```sh
   notepipe https://youtu.be/VIDEO_ID
   ```

If you pass a URL without configuring a download command, `notepipe` stops before starting any work with:
`A URL needs a download stage with a command, for example ["notepipe-source-ytdlp"] from notepipe[ytdlp]`.

Run `notepipe doctor` at any time to verify `ffmpeg`, `ffprobe`, your `config.toml`, configured adapter executables and
provider environment variables.

---

## The pipeline & default configuration

`notepipe` reads `config.toml` from `$XDG_CONFIG_HOME/notepipe/` when set, `%APPDATA%\notepipe\` on Windows, and
`~/.config/notepipe/` elsewhere:

```sh
notepipe config init    # write the default config.toml (--force to overwrite)
notepipe config path    # print the config file path
notepipe config show    # print the effective configuration as TOML
```

In the generated file, settings at their built-in defaults are shown as `#` comments so every knob is visible in place,
and the prompts for `proofread` and `summary` are written out in full. Here is the exact default configuration with the
two multi-line prompts shortened to `'''…'''`:

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

### Stage types and file kinds

Stage names (`draft`, `proofread`, `summary`, …) and file names (`transcript.json`, `draft.json`, …) are arbitrary. What
a stage does is determined solely by its `type`, and each stage produces a typed artifact in the work folder that
downstream stages can read:

| Stage `type` | Reads (`input`) | Writes (`output`) | What it does |
|---|---|---|---|
| `download` | `"source"` | `media` | Resolves a local file (plus adjacent `.srt`/`.vtt` files) in place, or runs `command` to fetch a URL's media, metadata and captions. |
| `transcript` | `media` | `transcript` (`.json`) | Tries `sources` in order (`subtitles`, `speech`). For speech, detects voice segments locally with `earshot`, splits long stretches at quiet points, and transcribes via `provider`. |
| `screenshots` | `media` | `images` (folder) | Samples video frames via `ffmpeg`, compares them with SSIM (`sensitivity`, optional `region`), waits for animations to settle (`mode = "stable"`), and saves full-res JPEGs. |
| `document` | `transcript` + optional `images` | `document` (`.json`) | Aligns transcript lines under the active screenshot (splitting only at natural punctuation/space breaks) and merges fragments into paragraphs with script-aware word spacing. |
| `llm` | `document`, `transcript` or `markdown` | `document` (`.json`) or `markdown` (`.md`) | Sends the input to `provider` with `prompt`. An `output` ending in `.json` rewrites a document's paragraphs in batches (keeping original text in `raw`); ending in `.md` produces one Markdown response (using chunked map-reduce for long recordings). |
| `markdown` | one or more `document` / `markdown` files | `markdown` (`.md`) | Joins its inputs in order, rendering documents as `![](<images/0001.jpg>)` followed by their paragraphs, without adding front matter or headings. |
| `run` | any stage outputs | `file` (or `produces` kind) | Runs an external command without a shell. Setting `produces = "transcript" \| "document" \| "markdown"` validates the output so later stages can read it. |

Every setting and CLI override is documented in
[docs/configuration.md](https://github.com/3ae3ae/notepipe/blob/main/docs/configuration.md).

---

## Rewiring and extending the pipeline

Because stages communicate only through named files in the work folder, you can insert, reorder or replace stages
without touching `notepipe`'s source code.

### Add custom AI stages, CLI filters and export formats

```toml
pipeline = [
  "download",
  "transcript",
  "screenshots",
  "draft",
  "glossary",
  "proofread",
  "translate",
  "actions",
  "summary",
  "markdown",
  "docx",
]

# 1. A non-LLM filter: reads draft.json on stdin, writes terms.json on stdout
[stages.glossary]
type = "run"
input = "draft.json"
output = "terms.json"
produces = "document"
command = ["python3", "fix-terms.py", "--glossary", "glossary.tsv"]

# 2. Proofread the glossary-fixed document, attaching each section's screenshot for visual spelling context
[stages.proofread]
type = "llm"
input = "terms.json"
output = "proofread.json"
provider = "openai"
model = "gpt-6-luna"
images = true
prompt = '''…'''

# 3. Rewrite each paragraph into Korean (.json output preserves timestamps, screenshots and `raw` text)
[stages.translate]
type = "llm"
input = "proofread.json"
output = "translated.json"
publish = "note.json"
provider = "openai"
model = "gpt-6-luna"
prompt = "Translate each item into Korean. Keep technical identifiers, code and LaTeX math unchanged."

# 4. Produce a standalone Markdown section (.md output answers once for the whole recording)
[stages.actions]
type = "llm"
input = "proofread.json"
output = "actions.md"
provider = "openai"
model = "gpt-6-luna"
prompt = "Extract decisions and action items as a Markdown checklist, noting owners and timestamps."

# 5. Assemble summary, action items and translated body in order
[stages.markdown]
type = "markdown"
input = ["summary.md", "actions.md", "translated.json"]
output = "note.md"
publish = "note.md"

# 6. Convert note.md to Word with pandoc and publish note.docx into the note folder
[stages.docx]
type = "run"
input = "note.md"
output = "note.docx"
publish = "note.docx"
command = ["pandoc", "{input}", "-o", "{output}"]
```

A `run` stage without `{input}` receives its single input file on `stdin`, and without `{output}` its `stdout` becomes
the stage's output file — so a complete document filter in Python is just:

```python
#!/usr/bin/env python3
import json, sys

doc = json.load(sys.stdin)
for section in doc["sections"]:
    for item in section["speech"]:
        item["text"] = item["text"].replace("Kubernetis", "Kubernetes")
json.dump(doc, sys.stdout)
```

If a `run` stage on `transcript.json` adds custom fields to transcript events (such as `"speaker": "Alice"`), the
`document` stage preserves those fields and automatically starts a new paragraph whenever they change.

### Switch cloud providers or local servers

Any OpenAI-compatible HTTP endpoint works as a provider. Define `[providers.NAME]` and reference `provider = "NAME"` on
the stages that should use it; API keys are read from the environment variable named in `api_key_env` and never stored
in `config.toml`.

```toml
[providers.groq]
base_url = "https://api.groq.com/openai/v1"
api_key_env = "GROQ_API_KEY"

[providers.ollama]
base_url = "http://localhost:11434/v1"
concurrency = 1
```

| Service | `base_url` | Notes |
|---|---|---|
| **OpenAI** | `https://api.openai.com/v1` | Default (`whisper-1`, `gpt-6-luna`, …) |
| **Anthropic** | `https://api.anthropic.com/v1` | Any Claude model for `llm` stages |
| **Google Gemini** | `https://generativelanguage.googleapis.com/v1beta/openai` | Any Gemini model |
| **Groq** | `https://api.groq.com/openai/v1` | e.g. `whisper-large-v3-turbo` for speech or chat models |
| **Mistral** | `https://api.mistral.ai/v1` | e.g. `voxtral-mini-latest` for speech or chat models |
| **OpenRouter** | `https://openrouter.ai/api/v1` | Any routed chat or multimodal model |
| **Ollama / LM Studio / vLLM** | `http://localhost:11434/v1` (or your server's URL) | Set `concurrency = 1` for single-GPU local servers |

For audio-capable chat models, set `api = "chat"` under `[stages.transcript.speech]` to send speech segments to
`/chat/completions` instead of `/audio/transcriptions`.

### Plug in local speech engines and other APIs (Adapters)

When a provider defines `command` instead of `base_url`, `notepipe` starts that program on demand with
`--host 127.0.0.1 --port <free port>`, waits for `GET /health` to return `200`, routes requests to it across all stages
sharing that provider, and stops its process tree when the last stage finishes:

```toml
[providers.llama]
command = ["notepipe-asr-llama", "-hf", "ggml-org/Qwen3-ASR-1.7B-GGUF:Q8_0"]
concurrency = 1

[stages.transcript.speech]
provider = "llama"
```

Official adapters are published as independent packages and install as extras of `notepipe`:

| Extra | Executable | Kind | What it does |
|---|---|---|---|
| [`ytdlp`](https://github.com/3ae3ae/notepipe/blob/main/adapters/ytdlp) | `notepipe-source-ytdlp` | download | Downloads media, metadata and captions from any site [yt-dlp](https://github.com/yt-dlp/yt-dlp) supports; honors your yt-dlp config and forwards args after `"--"`. |
| [`llama`](https://github.com/3ae3ae/notepipe/blob/main/adapters/llama) | `notepipe-asr-llama` | provider | Local speech recognition on CPU or GPU via [llama.cpp](https://github.com/ggml-org/llama.cpp)'s `llama-server`. |
| [`openvino`](https://github.com/3ae3ae/notepipe/blob/main/adapters/openvino) | `notepipe-asr-openvino` | provider | Local speech recognition on an Intel NPU (default), CPU or GPU via [OpenVINO GenAI](https://github.com/openvinotoolkit/openvino.genai). |
| [`speech-swift`](https://github.com/3ae3ae/notepipe/blob/main/adapters/speech-swift) | `notepipe-asr-speech-swift` | provider | Local speech recognition on Apple silicon (macOS 15+) using MLX or Core ML via [speech-swift](https://github.com/soniqo/speech-swift) *(built in CI; not yet verified on hardware)*. |
| [`xai`](https://github.com/3ae3ae/notepipe/blob/main/adapters/xai) | `notepipe-asr-xai` | provider | Bridges transcription requests to xAI's speech-to-text API (`XAI_API_KEY`). |
| [`meta`](https://github.com/3ae3ae/notepipe/blob/main/adapters/meta) | `notepipe-asr-meta` | provider | Bridges transcription requests to Meta's speech-to-text API (`META_API_KEY`). |

Writing your own provider, download or run program in any language takes only a few dozen lines of code. Full protocol
specifications and working Python stdlib templates are in
[docs/adapters.md](https://github.com/3ae3ae/notepipe/blob/main/docs/adapters.md). Third-party adapters can be installed
alongside `notepipe` with `uv tool install notepipe --with <package>` or placed anywhere on `PATH`.

---

## Using notepipe as an engine from other programs

Other tools, desktop UIs and automation pipelines integrate with `notepipe` by spawning the binary:

```sh
notepipe "$SOURCE" -o "$OUT" --config - --json < config.toml
```

- **Configuration on `stdin` (`--config -`):** Pass the full pipeline as TOML or JSON without touching the user's
  `~/.config/notepipe/config.toml`. CLI flags (`-o`, `--skip`, `--language`, `--subtitle`, …) still apply on top.
- **Structured NDJSON events (`--json`):** Standard output emits one JSON object per line:
  - `{"type": "stage", "stage": "transcript", "status": "start" | "done"}` (plus substeps like `transcript/speech` or `screenshots/scan`)
  - `{"type": "progress", "stage": "…", "current": 4, "total": 18}`
  - `{"type": "tokens", "stage": "proofread", "prompt": 1420, "completion": 980}`
  - `{"type": "log", "level": "info" | "warn" | "error" | "debug", "message": "…"}`
  - `{"type": "done", "note_dir": "…", "files": {"markdown": "…", "proofread": "…"}, "work_dir": "…", "warnings": []}`
  - `{"type": "error", "message": "…"}`
- **Structured document (`note.json`, schema 3):**
  ```json
  {
    "schema": 3,
    "meta": {
      "title": "Introduction to Compilers",
      "author": "…",
      "duration": 2740.5,
      "source": "/abs/path/to/lecture.mp4",
      "seek_url": "https://…"
    },
    "sections": [
      {
        "t": 0.0,
        "end": 42.0,
        "image": "images/0001.jpg",
        "speech": [
          {"start": 0.5, "end": 14.2, "text": "We begin with lexical analysis.", "raw": "uh we begin with lexical analysis"}
        ]
      }
    ]
  }
  ```
- **Operational guarantees:**
  - **Crash-safe publishing.** A note is assembled in a staging directory and swapped into `<out>/<title>/` whole.
    Failed or cancelled runs leave an existing note untouched; an interrupted swap is completed or rolled back on the
    next run. Folders not created by `notepipe` (missing `.notepipe.json`) are never overwritten; distinct recordings
    sharing a title receive `-2`, `-3`, … suffixes.
  - **Content-addressed caching and resume.** Downloads, screenshots, speech segments and validated LLM responses are
    cached in the source's work folder by the inputs that affect them. Editing the summary prompt sends only the summary's requests again;
    an interrupted transcription resumes from the first unfinished segment (`--no-resume` starts fresh).
  - **Clean cancellation.** `Ctrl-C` (or `SIGTERM` on Unix) exits with code `130`, terminates every child process tree
    `notepipe` spawned, and removes temporary downloaded media and extracted audio.

See [docs/cli.md](https://github.com/3ae3ae/notepipe/blob/main/docs/cli.md) for the complete CLI integration contract and
[docs/design.md](https://github.com/3ae3ae/notepipe/blob/main/docs/design.md) for architecture and design principles.

---

## Documentation & project status

- [Design & architecture](https://github.com/3ae3ae/notepipe/blob/main/docs/design.md)
- [Configuration reference](https://github.com/3ae3ae/notepipe/blob/main/docs/configuration.md)
- [CLI integration protocol & `note.json` schema](https://github.com/3ae3ae/notepipe/blob/main/docs/cli.md)
- [Adapter contracts (provider, download & run programs)](https://github.com/3ae3ae/notepipe/blob/main/docs/adapters.md)
- [Changelog](https://github.com/3ae3ae/notepipe/blob/main/CHANGELOG.md) ·
  [Contributing](https://github.com/3ae3ae/notepipe/blob/main/CONTRIBUTING.md) ·
  [Security policy](https://github.com/3ae3ae/notepipe/blob/main/SECURITY.md)

`notepipe` is in `0.x`. While the project is young, the configuration format, CLI options and `note.json` schema may
evolve between minor versions; all breaking and user-visible changes are recorded in
[CHANGELOG.md](https://github.com/3ae3ae/notepipe/blob/main/CHANGELOG.md).

## Acknowledgements

`notepipe` began as a fork of **[course2md](https://github.com/mizorewww/course2md)** by
[mizorewww](https://github.com/mizorewww), and its screenshot detection, subtitle handling and timeline alignment are
adapted from `course2md`'s foundations. Thank you to mizorewww and all
[course2md contributors](https://github.com/mizorewww/course2md/graphs/contributors) — if you want a full desktop
application with a built-in notes library, check out `course2md`.

`course2md`'s screenshot detection approach follows
[yt-slide-mark](https://github.com/exopoiesis/yt-slide-mark) by exopoiesis. `notepipe` also builds on
[FFmpeg](https://ffmpeg.org) and [earshot](https://crates.io/crates/earshot), and its official adapters wrap
[yt-dlp](https://github.com/yt-dlp/yt-dlp), [llama.cpp](https://github.com/ggml-org/llama.cpp),
[OpenVINO](https://github.com/openvinotoolkit/openvino.genai) and
[speech-swift](https://github.com/soniqo/speech-swift).

## License

Released under the [MIT License](https://github.com/3ae3ae/notepipe/blob/main/LICENSE), which retains `course2md`'s
copyright notice alongside `notepipe`'s own.
