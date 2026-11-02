# Configuration reference

notepipe reads `config.toml` from `$XDG_CONFIG_HOME/notepipe/` when that variable is set, otherwise from
`%APPDATA%\notepipe\` on Windows and `~/.config/notepipe/` elsewhere. `notepipe config path` prints the location.
`--config FILE` uses another file, and `--config -` reads the configuration from standard input; either may be TOML or
JSON with the same structure.

```sh
notepipe config init    # create the file with the default settings
notepipe config path    # print where it is
notepipe config show    # show the configuration in effect
```

The first run creates the file when there is none and says where it is; `config init` writes the same file, and the
[README](../README.md#the-pipeline--default-configuration) shows it. It uses OpenAI with the key in `OPENAI_API_KEY`, so it works on local
files as soon as that variable is set; URLs also need a [download program](#download). Settings at their default are written as comments, so the file shows every value in effect:
remove the `#` to change one. Settings that start empty are left out of it and described below.

Values are taken from, in order: command-line options, the configuration, the defaults below. Unknown keys are errors,
so a typo is reported instead of being ignored.

## How a run works

Every stage reads files and writes one file. A stage names what it reads in `input` and what it writes in `output`;
apart from the source's title, author, duration and link, nothing else passes between stages. The files live in the source's work folder, so you can open any of them:

```
source ─► download ─► media ─┬─► transcript ─► transcript.json ─┐
                             └─► screenshots ─► images/ ────────┴─► draft ─► draft.json
                                                                                 │
                         note.md ◄─ markdown ◄─ summary.md ◄─ summary ◄─ proofread.json ◄─ proofread
```

A stage's `publish` copies its output into the note folder, `<out>/<title>/`, or to standard output. Nothing reads a
published copy back, so moving or renaming what you publish never changes what other stages read.

The work folder is `work/<source>/` in notepipe's cache folder: `$XDG_CACHE_HOME/notepipe` when that variable is set,
otherwise `~/Library/Caches/notepipe` on macOS, `%LOCALAPPDATA%\notepipe\cache` on Windows and `~/.cache/notepipe`
elsewhere. `-v` prints it, and the
`done` event of `--json` gives it as `work_dir`.

## Top level

| Key | Option | Default | Meaning |
|---|---|---|---|
| `pipeline` | `--skip` | required | The stages that run, in order. A stage whose name is not listed does not run, but its table stays and is still checked |
| `out` | `-o`, `--out` | `"."` | Folder that receives note folders; `~` is your home folder |

## `[providers.NAME]`

A provider is a service that stages send requests to: an OpenAI-compatible API, or a program that notepipe starts and
that serves one (see [docs/adapters.md](adapters.md#provider-programs)).

| Key | Default | Meaning |
|---|---|---|
| `base_url` | — | API root, such as `https://api.openai.com/v1`; stages add the path they call |
| `command` | — | A program to start instead of `base_url`, as an argument list; a provider has one of the two |
| `api_key_env` | `""` | Environment variable that holds the key, sent as `Authorization: Bearer <key>`; none when empty |
| `concurrency` | `4` | Requests sent to the provider at the same time, 1 to 16, shared by every stage that uses it. Local engines usually want `1` |

When a provider that a running stage uses names a variable that is not set, the run stops before any work starts.

## `[stages.NAME]`

The name is yours to choose: letters, digits, `-` and `_`. What a stage does depends only on its `type`. Every stage has
these keys:

| Key | Meaning |
|---|---|
| `type` | What the stage does: `download`, `transcript`, `screenshots`, `document`, `rewrite`, `write`, `markdown` or `run` |
| `input` | What it reads: `"source"` (the file or URL on the command line; `download` only) or the `output` of a stage listed earlier in `pipeline`. A string, or a list |
| `output` | What it writes: a plain name in the work folder, such as `transcript.json`. No `/` or `\`, not starting with `.`, not `source`; each name is written by one stage; names that differ only in case count as the same, and so do names the file system treats as one file |
| `publish` | Optional: a name to copy the output to in the note folder, such as `note.md`, with the same rules as `output`, or `-` for standard output. Each name, ignoring case and anything else the file system ignores, is published by one stage, and only one stage, writing a file rather than a folder, can publish to `-` |

### What stages read and write

Each output has a kind, fixed by the stage that writes it, and each stage type reads only some kinds:

| Kind | Written by | Is |
|---|---|---|
| `media` | `download` | a folder with the media, its subtitles and `info.json` |
| `transcript` | `transcript` | JSON: the language and timed lines, `{"language": "en", "events": [{"start", "end", "text"}, …]}` |
| `images` | `screenshots` | a folder of `0001.jpg`, `0002.jpg`, … with `images.json`, `[{"t", "image"}, …]` |
| `document` | `document`, `rewrite` | JSON in the [note.json format](cli.md#output): sections with their screenshot and paragraphs |
| `markdown` | `markdown`, `write` | Markdown text |
| `file` | `run` | whatever the program wrote; it can only be published |

A `run` stage with `produces` writes a `transcript`, `document` or `markdown` instead. Every stage also knows the
source's title, author, duration and link, from the download stage.

### `download`

Reads `source` and writes `media`. For a URL it runs the [download program](adapters.md#download-programs) in
`command`; a local file
is used where it is, with the subtitles found next to it (`NAME.srt`, `NAME.vtt` and language variants such as
`NAME.en.srt`). A URL needs this stage with a `command`, and a run has at most one. Downloaded media is deleted once the run ends; a local file never is. The output
of a download stage cannot be published.

| Key | Default | Meaning |
|---|---|---|
| `command` | none | The download program for URLs, as an argument list, such as `["notepipe-source-ytdlp"]` (installed with `uv tool install "notepipe[ytdlp]"`); for yt-dlp, arguments after `"--"` go to yt-dlp itself. Without it, the stage takes local files only |

### `transcript`

Reads `media` and writes `transcript`. It tries its sources in order and uses the first that gives a transcript.

| Key | Option | Default | Meaning |
|---|---|---|---|
| `language` | `--language` | `"auto"` | Spoken language as a code such as `en` or `ko`; it picks the subtitle language and is sent to speech recognition. `auto`: no preference, and the provider detects it |
| `sources` | — | `["subtitles", "speech"]` | `subtitles`, `speech` or both, in the order to try |

`[stages.NAME.subtitles]`: the subtitles in `media` are ranked with those in `language` first, then those in the
recording's original language, and human captions before machine-generated ones.

| Key | Default | Meaning |
|---|---|---|
| `automatic` | `true` | Machine-generated captions may be used |

`[stages.NAME.speech]`: speech recognition. Speech is located on your machine with a voice activity detector, cut into
segments and sent to the provider.

| Key | Default | Meaning |
|---|---|---|
| `provider` | required | The provider to send speech to |
| `api` | `"transcriptions"` | `transcriptions`: `/audio/transcriptions`; `chat`: audio input to `/chat/completions` |
| `model` | `""` | Sent only when set |
| `prompt` | none | With `api = "chat"`, required: the instructions sent with each segment, followed by the language when it is set. Not accepted with `transcriptions` |
| `threshold` | `0.5` | Speech confidence from 0 to 1; lower keeps quieter speech |
| `min_silence` | `0.4` | Pauses shorter than this many seconds do not split segments |
| `max_segment` | `20.0` | Longest stretch of speech in one request, 1 to 600 seconds; up to 0.25 s of silence is added on each side |

`--subtitle FILE` makes FILE the stage's only source. Subtitles that are found but cannot be read are an error; cues
with times that cannot be read are skipped with a warning. Subtitles are read afresh on every run.

### `screenshots`

Reads `media` and writes `images`: a screenshot each time the picture changes. Audio files give none.

| Key | Option | Default | Meaning |
|---|---|---|---|
| `mode` | `--screenshot-mode` | `"stable"` | `stable` waits for the picture to stop changing; `first` takes the screenshot at the first change |
| `sensitivity` | `--sensitivity` | `0.85` | How small a change triggers a screenshot, greater than 0 and at most 1; higher takes more |
| `frame_interval` | `--frame-interval` | `1.0` | Seconds between compared frames, at least 0.2 |
| `min_gap` | `--min-gap` | `10.0` | Least seconds between screenshots; a final screenshot at the end of the recording may come sooner |
| `settle` | `--settle` | `0.8` | In `stable` mode, seconds the picture must stay unchanged |
| `region` | `--region` | the whole frame | Only this part of the frame is compared, as `left%,top%-right%,bottom%` (for example `"40%,0%-100%,100%"`) |

### `document`

Reads a `transcript` and, optionally, `images`, and writes a `document`: each transcript line, whole, under the
screenshot that was on screen at its midpoint, and the lines joined into paragraphs, with word spacing that follows each
writing system. Lines that carry different extra fields, such as a speaker added by a `run` stage, start a new
paragraph. It has no settings.

### `rewrite`

Reads a `document` and writes a `document`: its paragraphs go to a model in batches, with the prompt, and each is
replaced by the answer. Times and screenshots stay, and a changed paragraph keeps its first original text as `raw`.

| Key | Default | Meaning |
|---|---|---|
| `provider` | required | The provider to send requests to |
| `model` | `""` | Sent only when set |
| `prompt` | `""` | Instructions sent with the paragraphs |
| `images` | `false` | Also send the screenshot shown during each paragraph |
| `batch_size` | `20` | Paragraphs in one request, 1 to 100. Small local models follow the answer's format more reliably with fewer |
| `request` | `{}` | Extra fields for the request body, for example `{ reasoning_effort = "low" }` |

Paragraphs travel as plain text between numbered marker lines, in the request and in the answer, so nothing in them is
escaped ([docs/adapters.md](adapters.md#chat-api)). An empty answer for one paragraph removes that paragraph; a stage
that removes every paragraph warns and goes on.

### `write`

Reads a `transcript`, `document` or `markdown` and writes `markdown`: the model's answer. Transcripts and documents are
sent as timed lines with the title and author, and Markdown as it is.

| Key | Default | Meaning |
|---|---|---|
| `provider` | required | The provider to send requests to |
| `model` | `""` | Sent only when set |
| `prompt` | `""` | Instructions sent with the input |
| `chunk_size` | `25000` | About how many characters of timed lines go in one request, 1000 to 1000000. A longer transcript or document is sent in parts, and one more request combines their answers; Markdown is always sent whole |
| `request` | `{}` | Extra fields for the request body, for example `{ reasoning_effort = "low" }` |

### Requests to models

`rewrite` and `write` stages add instructions for the answer's format to the prompt and set `model`, `messages` and
`stream` themselves; `request` cannot change those or set `response_format`. Nothing is sent by default for sampling or
length limits.

A request is tried at most three times. A failed connection, a `429` or `5xx` answer and an answer that is not in the
expected format are tried again. Other client errors, and an answer cut off at the model's output limit or stopped by
its content filter, fail at once. A stage that fails stops the run, nothing is published, and the error names the stage,
the part of the recording and what the service said.

The default file holds a `rewrite` and a `write` stage:

| Stage | Type | Reads → writes | What its prompt asks for |
|---|---|---|---|
| `proofread` | `rewrite` | `draft.json` → `proofread.json` | Fixes recognition errors, filler and punctuation without changing meaning or language; writes formulas as LaTeX |
| `summary` | `write` | `proofread.json` → `summary.md` | A one-sentence overview, 3–6 key points and a timestamped outline, in the transcript's language |

### `markdown`

Reads one or more `document` and `markdown` files and writes `markdown`: each input in the order of `input`, separated by a blank
line. A document becomes its screenshots, as `![](<images/0001.jpg>)`, each followed by the paragraphs placed under
it. Nothing else is added: no front matter, title or headings. To lay the note out differently, write the
Markdown with a `write` stage or a `run` stage instead.

### `run`

Runs a program of your own, or any command-line tool, as a stage. See
[docs/adapters.md](adapters.md#run-programs).

| Key | Default | Meaning |
|---|---|---|
| `command` | required | The program and its arguments, as a list. `{input}` and `{output}`, as whole arguments, become the input paths and the output path |
| `produces` | none | `"transcript"`, `"document"` or `"markdown"`, when another stage reads the output; it is then checked like any other output of that kind |

## Publishing and the note folder

When every stage has finished, notepipe writes the note folder, `<out>/<title>/`, with every file a stage publishes.
A published document, or Markdown made from one, brings along the screenshots it shows, at the same path as in the
work folder: `images/` with the default names. Markdown brings along the images it refers to by the path the document
uses. The folder is built under a hidden name and renamed into place, so a reader never sees a half-written note and a
failed run writes nothing.

A note folder is never changed after it is written. notepipe recognises a source's notes by `.notepipe.json` inside,
which names the source by its contents. A rerun whose files are the same as the last ones it published leaves the note
as it is, even if you edited, renamed or added to it, and says `Note unchanged`; if no folder in the output folder
carries the source's `.notepipe.json`, the note is written again. A rerun with different results, for example after a
prompt changed, writes a new folder next to the old one with a `-2`, `-3`, … suffix, as does a source whose title
another folder already uses. A recording that was moved reuses all earlier work, but its new location changes its
note, so it gets a new folder too. Delete folders you no longer want yourself.

`publish = "-"` prints the output to standard output after the note folder is written; it cannot be combined with
`--json`, which uses standard output for events. A run that publishes only to standard output writes no folder.

## Order, speed and repeated runs

Before any work starts, notepipe checks the whole configuration: every input is written by an earlier stage, each
stage reads kinds it understands, names are unique, the keys of the providers in use are set, and something is
published. The download stage runs first, since every stage can read the source's details from it; the others then
start as soon as their inputs are ready, so the transcript and the screenshots are made at the same time.

Every stage writes its output again on every run, from what it has done before: a download, speech segments, and model
answers are reused as long as the settings that shaped them are unchanged. A local source is recognised by its
contents, so a moved or renamed file reuses this work. Changing a prompt therefore repeats only that
stage's requests, and an interrupted transcription resumes where it stopped. `document`, `markdown` and `run` stages
always run. `--no-resume` starts the source over.

`--skip NAMES` leaves stages out for one run. A skipped `rewrite` stage passes its input on unchanged, so
skipping `proofread` publishes `draft.json` as `note.json`. Any other skipped stage writes nothing: a `markdown` stage
leaves it out (skipping `summary` gives a note without a summary), and a `document` stage works without screenshots.
Skipping a stage whose output another stage needs is an error before any work starts.

## Environment variables

| Variable | Meaning |
|---|---|
| the names in `api_key_env` | API keys of providers |
| `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` | Configuration and cache locations |
| `RUST_LOG` | Log filter for `-v` diagnostics |

## Example

Speech on a local engine, the AI stages on a local Ollama, glossary terms fixed by a script, a Word copy made with
pandoc, and the Markdown also printed for the next command in a shell pipeline:

```toml
pipeline = ["download", "transcript", "screenshots", "draft", "terms", "proofread", "markdown", "docx"]
out = "~/notes"

[providers.llama]
command = ["notepipe-asr-llama", "-hf", "ggml-org/Qwen3-ASR-1.7B-GGUF:Q8_0"]
concurrency = 1

[providers.ollama]
base_url = "http://localhost:11434/v1"
concurrency = 1

[stages.download]
type = "download"
input = "source"
output = "media"
command = ["notepipe-source-ytdlp"]

[stages.transcript]
type = "transcript"
input = "media"
output = "transcript.json"
language = "en"

[stages.transcript.speech]
provider = "llama"

[stages.screenshots]
type = "screenshots"
input = "media"
output = "images"

[stages.draft]
type = "document"
input = ["transcript.json", "images"]
output = "draft.json"

[stages.terms]
type = "run"
input = "draft.json"
output = "terms.json"
produces = "document"
command = ["python3", "fix-terms.py", "--glossary", "glossary.tsv"]

[stages.proofread]
type = "rewrite"
input = "terms.json"
output = "proofread.json"
publish = "note.json"
provider = "ollama"
model = "qwen3:14b"
prompt = "Fix speech-recognition errors and filler in each item. Keep its meaning and language."

[stages.markdown]
type = "markdown"
input = "proofread.json"
output = "note.md"
publish = "-"

[stages.docx]
type = "run"
input = "note.md"
output = "note.docx"
publish = "note.docx"
command = ["pandoc", "{input}", "-o", "{output}"]
```

`fix-terms.py` reads the document on standard input and prints the changed document, and runs in the folder notepipe
was started in. With `publish = "-"` on the markdown stage, `notepipe talk.mp4 | glow` shows the note in the terminal
while `note.json` and `note.docx` go into the note folder.
