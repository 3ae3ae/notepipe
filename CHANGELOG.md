# Changelog

All notable changes to notepipe are documented in this file. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Adapters under `adapters/` are versioned separately.

## [Unreleased]

## [0.4.4] - 2026-10-28

### Added

- `notepipe[sensevoice]` installs the SenseVoice speech adapter (`notepipe-asr-sensevoice` 0.1.0), which runs
  SenseVoice through FunASR on CPU, an NVIDIA GPU or Apple silicon. It recognizes Chinese, English, Japanese, Korean
  and Cantonese.

## [0.4.3] - 2026-10-26

### Fixed

- A speech program that stops at the model's output limit can say so with the error code `output_limit`; notepipe
  then reports "The model stopped at its output limit." instead of blaming the request parameters. The llama.cpp
  adapter (`notepipe-asr-llama` 0.1.3) sends it.
- On a re-run, a transcript stage that reads only subtitles no longer downloads the media again when the source has
  no subtitles.
- The yt-dlp adapter (`notepipe-source-ytdlp` 0.1.2) fails a download that is missing fragments, or that met errors
  `--ignore-errors` would pass over, instead of handing on the partial file as complete.

### Changed

- The speech-swift adapter's README (`notepipe-asr-speech-swift` 0.1.2) says when Whisper on Core ML ends a segment
  early without an error.

## [0.4.2] - 2026-10-26

### Fixed

- The llama.cpp speech adapter (`notepipe-asr-llama` 0.1.2) no longer returns a transcript that the model cut off at
  its output limit as finished; it fails the segment, and notepipe neither retries nor keeps it. The adapter contract
  asks the same of every speech program.

## [0.4.1] - 2026-10-23

### Changed

- The generated configuration takes local files: its download stage has no `command`, because the yt-dlp adapter is a
  separate install. To take URLs, install `notepipe[ytdlp]` and add `command = ["notepipe-source-ytdlp"]` to
  `[stages.download]`; a URL without a download command stops with a message that says so. A download stage's
  `command` is optional.

## [0.4.0] - 2026-10-23

Stages now read and write files, and say which: the configuration shows where every input comes from and where every
output goes. Configurations from 0.3.0 must be rewritten; `notepipe config init --force` writes the new default file.

### Added

- `input`, `output` and `publish` on every stage. A stage reads files that earlier stages wrote and writes one file in
  the source's work folder; `publish` copies it into the note folder, or to standard output with `"-"`. Names that
  differ only in case count as the same, because many file systems treat them so.
- `run` stages run any command-line program, such as pandoc or a script of your own, on a stage's files.
- `document` stages: grouping the transcript under its screenshots, which happened implicitly before, is a stage.
  Transcript lines with different extra fields, such as a speaker added by a `run` stage, start a new paragraph.
- `out`, the folder that receives note folders.
- The work folder is printed with `-v` and given in the `done` event as `work_dir`.

### Changed

- An `llm` stage's `output` decides what it does: `.json` rewrites each paragraph of a document, `.md` answers once for
  a transcript, document or Markdown file. The default summary reads the proofread document.
- The `markdown` stage writes the note's text from the files it is given, in order, and adds nothing else: no front
  matter, title or headings of its own.
- A failing `llm` stage stops the run, and nothing is published.
- The note folder holds what stages publish, the screenshots those files show, and `.notepipe.json`, which marks the
  source the folder belongs to; a folder without it is never replaced.
- The download stage runs first; after it, stages that do not depend on each other run at the same time, and a
  provider's `concurrency` is shared by all the stages that use it.
- A provider's `command` and a download stage's `command` are argument lists; `args` is gone.
- Subtitles next to a local file are found by the download stage.
- `note.json` is schema 3, without `top`. The `done` event has `files` and `work_dir` instead of `markdown` and `json`.
- Configuration errors name the key and line.
- `notepipe doctor` no longer fails when only the download program is missing, since only URLs need it, and reports a
  missing key once.
- Skipped stages print nothing and send no events.
- Ctrl-C stops a run at once, without waiting for requests in flight, and removes downloaded media and extracted audio.

### Removed

- The `note` and `filter` stage types (replaced by `publish` and `run`), `keep`, and `reads`.

## [0.3.0] - 2026-10-22

### Added

- `keep = true` on a stage, for debugging, writes its result into the note folder's `stages/`, named after the stage:
  the downloaded media and subtitles, the transcript, the screenshots' times, the text after an `llm` stage, the
  Markdown an `llm` stage wrote, or the note a filter returned.

### Changed

- The stage types `rewrite`, `add-below` and `add-top` are one type, `llm`, whose `reads` is `"paragraphs"` (the
  answer replaces each paragraph) or `"transcript"` (the answer is added at the top).
- `note.json` is schema 2: fragments no longer have `below`.
- Stage names may use only letters, digits, `-` and `_`, because they name files in `stages/`.
- The configuration file notepipe creates writes settings at their default as comments, so only required settings
  and chosen values are active.

### Removed

- Adding a model's answer below each paragraph (`add-below`).
- `hook` stages. To do something with each note, run it after notepipe; the `done` event and the last line of
  output give the note folder.

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
