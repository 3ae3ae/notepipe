# Changelog

All notable changes to notepipe are documented in this file. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Adapters under `adapters/` are versioned separately.

## [Unreleased]

## [0.6.3] - 2026-11-14

### Changed

- `notepipe config show` starts with `# Configuration file: PATH`, a TOML comment, so its whole output is TOML and can
  be passed to `--config -`.
- `--help` has a "Programs" section: what `--json`, `--config -` and `publish = "-"` do with the standard streams, the
  exit statuses, and a link to docs/cli.md.

### Fixed

- With `--json`, a finished run's warnings, and a configuration file that cannot be created, are reported only as
  events; they were also printed to standard error.
- `--help` says that `-q` keeps warnings as well as errors, and no longer suggests that notepipe prompts without
  `--json`.
- README: the second "Why notepipe" heading no longer runs into the paragraph below it, and the pipeline diagram shows
  that the `markdown` stage also reads `proofread.json`.
- README: `notepipe talk.mp4 | glow` printed nothing with the default configuration. The README now shows a command
  that prints the note's Markdown for one run without editing `config.toml`, and says that `notepipe talk.mp4 | glow`
  needs `publish = "-"` on the `markdown` stage.
- docs/cli.md says what goes to standard output and standard error, that standard input is read only for
  `--config -`, that `--config` replaces the whole configuration, that `--json` reports a command line that cannot be
  parsed as an `error` event, and what `notepipe doctor` exits with.

## [0.6.2] - 2026-11-11

### Added

- Speech with `api = "chat"` takes a `request`, extra fields for the request body, like `rewrite` and `write` stages:
  `request = { temperature = 0 }` under `[stages.NAME.speech]` sends the sampling value that 0.6.1 stopped sending.
  Without it nothing changes, and answers already cached are still used.

### Fixed

- The pandoc examples in the documents pass `-f markdown+rebase_relative_paths`, so the Word copy holds the note's
  screenshots; with the earlier command pandoc did not find them.
- Documents: statements that the code did not bear out are corrected, among them what a failed run can leave behind,
  what `--no-resume` does to an unchanged note, and the model that three adapter examples send.

## [0.6.1] - 2026-11-11

### Changed

- Speech with `api = "chat"` sends the `prompt` as written and adds nothing of its own: the sentence that named the
  `language` is no longer appended, and `temperature` is no longer sent. Name the language in the prompt if the model
  needs it. Finished transcripts are reused as they are.
- A screenshots stage that fails stops the run, like every other stage. It used to publish a note without images and
  a warning. `--skip screenshots` makes the note without them.

### Fixed

- Notes are published on Android. The rename that puts a note folder in place without replacing another one was
  missing there, so every run that wrote a note folder ended with `Could not publish note folder`.

## [0.6.0] - 2026-11-04

Caches from earlier versions are not reused: the first rerun of a source repeats its speech recognition, screenshots
and model requests. Notes already written are not affected.

### Added

- A `run` stage can produce every kind, `media`, `images` and `file` as well as `transcript`, `document` and
  `markdown`, and can read `file` outputs, so a program can take the place of any built-in stage. For example, ffmpeg
  can pick a track or even out the loudness for speech recognition while the screenshots still come from the original
  video. A run stage's media must be a file that ffprobe reads; images must be a folder whose `images.json` lists
  images inside it in time order.

### Changed

- A media input reaches a `run` program as the recording file itself, not as the download stage's folder.
- Each speech segment is cached like a model request, by the provider, the speech settings and the segment's audio,
  so an interrupted transcription sends only the segments not yet answered. The start/end checkpoint is gone, and the
  answers live in `.cache/responses/` (formerly `.cache/llm/`).
- Transcripts and screenshots are reused only for the same media, recognised by the SHA-256 of its file. A download
  keeps it as `media_sha256` in `info.json`.
- Media is handled by kind: every media file in the work folder, downloaded, written by a `run` stage or extracted for
  speech recognition, is deleted when the run ends, and no media output can be published.
- A `run` stage whose output fails its kind's check names the stage: `Stage NAME: the program wrote …`.

### Fixed

- Changing a download's `command` so that it delivers different media no longer reuses the old media's transcript and
  screenshots.
- A transcription resumed after its media changed no longer mixes in lines from the old media.
- Audio extracted by an interrupted run is no longer reused for different media.

## [0.5.0] - 2026-11-03

Configurations, caches and note folders from earlier versions do not carry over: rewrite the configuration
(`notepipe config init --force` writes the new default), and expect the first run of each source to repeat its speech
recognition and model requests.

### Changed

- `type = "llm"` is replaced by two types. `rewrite` changes a document's paragraphs in batches (`batch_size`,
  `images`); `write` produces one Markdown answer (`chunk_size`). The output's name no longer decides what a stage does.
- A `rewrite` stage sends and receives paragraphs as plain text between numbered marker lines instead of JSON, so LaTeX
  backslashes and every other character pass through unchanged. `response_format` is no longer sent; an answer whose
  blocks do not match the paragraphs is asked again.
- A local source is identified by the SHA-256 of its contents, so a moved or renamed recording reuses all earlier work.
  `.notepipe.json` holds that identity instead of a path; the document's `meta.source` still holds the location.
- A note folder is never modified. A rerun with unchanged results publishes nothing and says `Note unchanged`, leaving
  edits as they are (`--json`: `"unchanged": true` in `done`); a changed result goes into a new folder next to the old
  one. Replacing a note, its transaction record and recovery, and the `.notepipe.lock` in the output folder are gone.
- Speech with `api = "chat"` takes its instruction from a `prompt` in the speech table, which it requires; the fixed
  instruction that asked for natural punctuation is gone.

### Fixed

- A file whose modification time is before 1970 can be processed.

## [0.4.7] - 2026-11-03

### Changed

- A request is tried at most three times, by one rule for speech and chat: failed connections, `429` and `5xx`
  responses, and answers that are not in the expected format are tried again; other client errors, the output limit and
  a content filter stop at once. Every request has 600 seconds; speech used to have 120 and chat 300.
- A failure names the part of the recording, such as `Speech recognition failed at 12:03–12:21` or `paragraphs at
  04:10–07:55`, and adds the service's own error message on one line, with the provider's key replaced. The message is
  shown and never stored. Retries are reported as warnings.
- Empty speech is silence: the transcript stage no longer stops after ten empty answers, and reports how many segments
  returned no text. A stage that removes every paragraph warns and goes on instead of failing, which also ends reruns
  that failed the same way from the cache.
- A provider program starts only when a request is not in the cache, so a rerun answered from the cache starts no
  engine.
- A transcript line is no longer split where the screenshot changes; it goes whole to the screenshot shown at its
  midpoint. Repeated subtitle cues are kept, and the lines of a cue are joined with the same spacing as paragraphs.
- The `--json` `error` event carries the warnings collected before the failure, and human output prints them too.

### Fixed

- Subtitle cleanup removed any text in angle brackets, so `vector<int>` or `x<y and z>w` lost text; it now removes only
  cue markup. Numeric character references, `&apos;`, `&lrm;` and `&rlm;` are decoded, `&nbsp;` becomes a no-break
  space, files with CR line endings are read, cues with unreadable times are reported, and subtitles are read afresh on
  every run.
- Merged paragraphs keep the latest end time, the original text of every line and the spaces between fragments, and
  Markdown keeps the indentation of its first line.
- A screenshot shows the frame that triggered it rather than the one before; earlier screenshots are taken again.
- A resumed transcription sent some finished segments again, because their times changed in the checkpoint's JSON.
- Tokens of answers that were rejected and asked again are counted.
- A provider program that exits during the run stops it with its exit status and last output; a program writing
  output that is not UTF-8 no longer fails; and notepipe no longer waits on pipes held by a descendant after a program
  ends or the run is cancelled.
- A duration that cannot be read is unknown (`null`) with a warning, instead of 0.
- Command-line options replace a configured value that fails validation; publishing a folder to standard output is
  rejected before any work; and names the file system treats as one file are an error instead of overwriting each
  other.
- The download program's canonical `url` is kept in the document's `meta`; a file it returns named `info.json`, and
  transcript fields named `schema`, `meta` or `sections`, are errors instead of breaking the run.
- `notepipe doctor` reports a program path that cannot be executed.
- The llama.cpp adapter (`notepipe-asr-llama` 0.1.5) fits all its decodes of a segment into 570 seconds, answers `504`
  when they run out, and says in a `warning` when a fallback decoding produced the text. The speech-swift adapter
  (`notepipe-asr-speech-swift` 0.1.3) answers `output_limit` when decoding stops early, instead of returning the
  shortened text as complete.

## [0.4.6] - 2026-10-28

### Fixed

- The llama.cpp adapter (`notepipe-asr-llama` 0.1.4) no longer lets Qwen3-ASR repeat one word until the request times
  out. Each segment's output is capped by its length, and a segment that reaches the cap is decoded again with a
  repetition penalty and then with sampling. Before, such a segment ran for two minutes on every attempt and stopped
  the run.

## [0.4.5] - 2026-10-28

### Fixed

- A paragraph stage no longer writes invisible control characters into the note when the model leaves a backslash
  unescaped, as in `\hat{y}` or `\frac`. JSON mode turns such a backslash into a control character, so notepipe now
  asks for that batch again.

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
