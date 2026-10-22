# Changelog

All notable changes to notepipe are documented in this file. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Adapters under `adapters/` are versioned separately.

## [Unreleased]

## [0.2.0] - 2026-10-22

The configuration is rebuilt: the whole run is a pipeline of named stages. Configurations from 0.1.0 must be rewritten;
`notepipe config init --force` writes the new default file.

### Added

- `pipeline`, the stages that run, in order, and one `[stages.NAME]` table per stage, with a `type`: `download`,
  `transcript`, `screenshots`, `rewrite`, `add-below`, `add-top`, `filter`, `note` or `hook`. The order is checked
  before any work starts.
- `[providers.NAME]`: a connection that stages share, either an API's `base_url` or a `command` that notepipe starts,
  with the variable that holds its key (`api_key_env`) and its `concurrency`.
- `filter` stages: a program reads the note as JSON on standard input and prints it changed.
- Notes without screenshots for videos, by leaving the screenshots stage out.
- The transcript stage's `sources` order and subtitle choices (`sidecar`, `downloaded`, `automatic`), and the
  `batch_size` and `chunk_size` of AI stages, which were fixed before.
- `--skip NAMES` leaves stages out for one run.

### Changed

- The default configuration works out of the box with OpenAI: `whisper-1` for speech recognition and `gpt-6-luna` for
  the `proofread` and `summary` stages, with the key in `OPENAI_API_KEY`. A run without a configuration file creates it
  and says so; the file lists every value in effect, with the two prompts written out.
- `proofread` and `summary` are no longer built in; they are ordinary stages in the file.
- AI stages say what they do to the note (`rewrite`, `add-below`, `add-top`) instead of `input` and `output`.
- `on_complete` is a `hook` stage, and the output folder is the `note` stage's `dir`.
- `--json` stage events use the names of the pipeline's stages, such as `transcript` and `note`, with steps such as
  `screenshots/scan`; `media` replaces `fetch`.
- `notepipe config show` prints the configuration in effect as TOML.

### Removed

- The sections `[output]`, `[transcript]`, `[speech]`, `[screenshots]`, `[ai]` and `[download]`, and
  `[[ai.stages]]`.
- The first-run setup prompt; notepipe never prompts.
- API keys in the configuration (`api_key`) and `NOTEPIPE_SPEECH_API_KEY` / `NOTEPIPE_AI_API_KEY`; use `api_key_env`.
- Finding a download adapter by its name; the download stage names its program.
- `--stages`, `--no-stages`, `--transcript`, `--max-segment`, `--proofread-prompt`, and the `--speech-*` and `--ai-*`
  connection options.

## [0.1.0] - 2026-10-22

The first release as notepipe. It started from course2md 2.0.0-rc.16 by mizorewww; this entry lists the differences.

### Added

- Audio files and audio-only online items as input.
- Speech adapters: notepipe starts a local engine that serves an OpenAI-compatible transcription API for the run.
  Official adapters for llama.cpp, OpenVINO, speech-swift (Apple silicon), xAI and Meta.
- AI adapters: `[ai].adapter` starts a program that serves the AI stages through an OpenAI-compatible chat API, so other
  APIs or subscription access can be connected.
- Download adapters for URLs, with an official yt-dlp adapter; the core itself reads local files.
- A configuration file whose sections follow the pipeline (`[output]`, `[transcript]`, `[speech]`, `[screenshots]`,
  `[ai]`, `[download]`), with matching option names; `notepipe config path`; API keys are optional.
- Prompt-driven AI stages (`[[ai.stages]]`): built-in `proofread` and `summary`, plus custom stages that replace
  paragraphs, add text below them or add Markdown at the top of the note; per-stage model, endpoint, key and extra request
  fields.
- `--config FILE|-`, `--subtitle FILE`, `--stages`, `--no-stages`, `NOTEPIPE_SPEECH_API_KEY`, `NOTEPIPE_AI_API_KEY`.
- `[output].on_complete`, a command run after each note is written.
- Packages for `uv tool install`, with the official adapters as extras (`notepipe[ytdlp]`), and a release workflow that
  publishes to PyPI and crates.io.

### Changed

- Speech detection uses earshot, a small built-in neural detector, instead of a fixed loudness threshold.
- Notes are written as `<out>/<title>/note.md`, `images/` and `note.json`; `note.md` starts with YAML front matter.
  Running again on the same source replaces its note; intermediate results are cached per source.
- Default prompts are English and replaceable; summaries are Markdown written by the model in the transcript's language.
- The CLI and `--json` output are English; NDJSON events are limited to `stage`, `progress`, `tokens`, `log`, `done` and
  `error`.
- Subtitles are chosen by the configured language, then the media's original language, instead of preferring Chinese.
- Korean and other space-separated scripts keep their word spacing when transcript fragments are joined.
- Ctrl-C (and SIGTERM on Unix) stops the run and any adapters it started.

### Removed

- The desktop application.
- Built-in local speech backends (now adapters) and model management commands.
- Bilibili login and site-specific handling, cookie options and other website-specific code (now in download adapters).
- ChatGPT-subscription and Ollama login; every language model is reached as an OpenAI-compatible endpoint.
- Interface translations, `--lang` and `NOTEPIPE_LANG`.
- HTML and ZIP exports, versioned note storage, `run-task` and the `summarize` subcommand.
- `--translate` (translation is a custom AI stage), `--formats`, `--no-download`, `--resume`, `--keep-video`.
