# Changelog

All notable changes to notepipe are documented in this file. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Adapters under `adapters/` are versioned separately.

## [Unreleased]

### Added

- `adapter` on a stage: a program that handles that stage instead of the AI service. It need not use a language model,
  and such a stage needs no `prompt`.

### Changed

- Stages are top-level `[[stages]]` blocks instead of `[[ai.stages]]`; `[ai]` is the service they use by default.
  Configurations from 0.1.0 must move their stages.
- `proofread` and `summary` are no longer built in. They are ordinary stages in the configuration file, with their
  prompts written out, and every stage sets `input`, `output` and `prompt`.
- A run without a configuration file creates one and says where it is. It and `config init` write every default value
  and the two stages, and leave out settings that start empty.
- A stage with no AI service or adapter is skipped with a warning instead of stopping the run.

### Removed

- `--proofread-prompt`; edit the stage's prompt instead.

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
