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
[README](../README.md#configuration) shows it. It uses OpenAI with the key in `OPENAI_API_KEY`, so it works as soon as
that variable is set. Settings at their default are written as comments, so the file shows every value in effect:
remove the `#` to change one, take a stage out of the pipeline, or add your own. Settings that start empty are left out
of it and described below.

Values are taken from, in order: command-line options, the configuration, the defaults below. Unknown keys are errors,
so a typo is reported instead of being ignored.

## Structure

```toml
pipeline = ["download", "transcript", "screenshots", "proofread", "summary", "note"]   # what runs, in order

[providers.openai]        # connections that stages share
# …

[stages.proofread]        # one table per stage; the table's key is the stage's name
type = "llm"              # what the stage does
# …
```

The name is yours to choose: letters, digits, `-` and `_`. What a stage does depends only on its `type`, and the keys it
accepts depend on the type. A stage's settings live in its own table, and the only thing stages share is a provider. A stage whose name is not in
`pipeline` does not run, but its table stays and is still checked; `--skip NAMES` leaves stages out for one run.

## `pipeline`

The names of the stages to run, in order. Before any work starts, notepipe checks that:

- every name is a table in `[stages]`, listed once;
- the stages come in this order: `download`; then `transcript` and `screenshots`, in either order; then `llm` and
  `filter` stages, in any order; then `note`, last;
- at most one `download`, `transcript` and `screenshots` stage runs, and exactly one `note` stage;
- a `transcript` stage runs, unless `--subtitle FILE` supplies the transcript;
- a URL source has a `download` stage.

The `transcript` and `screenshots` stages work at the same time, because neither needs the other's result. The note's
sections and paragraphs are built from their results before the next stage starts.

## `[providers.NAME]`

A provider is a service that stages send requests to: an OpenAI-compatible API, or a program that notepipe starts and
that serves one (see [docs/adapters.md](adapters.md#provider-programs)).

| Key | Default | Meaning |
|---|---|---|
| `base_url` | — | API root, such as `https://api.openai.com/v1`; stages add the path they call |
| `command` | — | A program to start instead of `base_url`; a provider has one of the two |
| `args` | `[]` | Arguments for `command` |
| `api_key_env` | `""` | Environment variable that holds the key, sent as `Authorization: Bearer <key>`; none when empty |
| `concurrency` | `4` | Requests sent to the provider at the same time, 1 to 16. Local engines usually want `1` |

When a provider that a running stage uses names a variable that is not set, the run stops before any work starts.

## Keeping stage results

For debugging, a stage with `keep = true` writes its result into the note folder, under `stages/` and named after the
stage:

| Type | Kept as |
|---|---|
| `download` | `stages/NAME/`: the media file, its subtitles and `info.json`, the download program's result |
| `transcript` | `stages/NAME.json`: the transcript the run used, as `{"events": [{"start", "end", "text"}, …]}` |
| `screenshots` | `stages/NAME.json`: each screenshot's time and image, as `[{"t", "image"}, …]` |
| `llm` | `stages/NAME.json`: the note's sections right after the stage (`reads = "paragraphs"`), or `stages/NAME.md`: the Markdown it wrote (`reads = "transcript"`) |
| `filter` | `stages/NAME.json`: the note the program returned |

By default (`keep = false`) nothing is kept, and the downloaded media file is deleted once the note is written. The
`note` stage, which writes the note itself, has no `keep`; neither does a stage that does not run.

## `[stages.NAME]`

Every stage has a `type`, and each type has its own keys. `download`, `transcript`, `screenshots`, `llm` and `filter`
stages also take `keep` (default `false`; see [Keeping stage results](#keeping-stage-results)).

### `download`

Fetches the media of a URL with a [download program](adapters.md#download-programs). Local files skip this stage.

| Key | Default | Meaning |
|---|---|---|
| `command` | required | The program, such as `notepipe-source-ytdlp` (installed with `uv tool install "notepipe[ytdlp]"`) |
| `args` | `[]` | Its arguments; for yt-dlp, arguments after `"--"` go to yt-dlp itself |

### `transcript`

Makes the transcript. It tries its sources in order and uses the first that gives one.

| Key | Option | Default | Meaning |
|---|---|---|---|
| `language` | `--language` | `"auto"` | Spoken language as a code such as `en` or `ko`; it picks the subtitle language and is sent to speech recognition. `auto`: no preference, and the provider detects it |
| `sources` | — | `["subtitles", "speech"]` | `subtitles`, `speech` or both, in the order to try |

`[stages.NAME.subtitles]`: which subtitles count. Among them, those in `language` come first, then those in the
recording's original language, and human captions before machine-generated ones.

| Key | Default | Meaning |
|---|---|---|
| `sidecar` | `true` | `NAME.srt` or `NAME.vtt` next to a local `NAME.mp4`, including language variants such as `NAME.en.srt` |
| `downloaded` | `true` | Subtitles that the download stage delivered |
| `automatic` | `true` | Machine-generated captions may be used |

`[stages.NAME.speech]`: speech recognition. Speech is located on your machine with a voice activity detector, cut into
segments and sent to the provider.

| Key | Default | Meaning |
|---|---|---|
| `provider` | required | The provider to send speech to |
| `api` | `"transcriptions"` | `transcriptions`: `/audio/transcriptions`; `chat`: audio input to `/chat/completions` |
| `model` | `""` | Sent only when set |
| `threshold` | `0.5` | Speech confidence from 0 to 1; lower keeps quieter speech |
| `min_silence` | `0.4` | Pauses shorter than this many seconds do not split segments |
| `max_segment` | `20.0` | Longest stretch of speech in one request, 1 to 600 seconds; up to 0.25 s of silence is added on each side |

`--subtitle FILE` supplies the transcript directly; the transcript stage is skipped. Subtitles that are found but cannot
be read are an error.

### `screenshots`

Takes a screenshot when the picture changes. Without this stage, notes have text only; audio files never have
screenshots.

| Key | Option | Default | Meaning |
|---|---|---|---|
| `mode` | `--screenshot-mode` | `"stable"` | `stable` waits for the picture to stop changing; `first` takes the screenshot at the first change |
| `sensitivity` | `--sensitivity` | `0.85` | How small a change triggers a screenshot, greater than 0 and at most 1; higher takes more |
| `frame_interval` | `--frame-interval` | `1.0` | Seconds between compared frames, at least 0.2 |
| `min_gap` | `--min-gap` | `10.0` | Least seconds between screenshots; a final screenshot at the end of the recording may come sooner |
| `settle` | `--settle` | `0.8` | In `stable` mode, seconds the picture must stay unchanged |
| `region` | `--region` | the whole frame | Only this part of the frame is compared, as `left%,top%-right%,bottom%` (for example `"40%,0%-100%,100%"`) |

### `llm`

Sends the note to a model with a prompt and puts the answer into the note. `reads` decides what the model reads and
where its answer goes:

| `reads` | The model reads | Its answer |
|---|---|---|
| `"paragraphs"` | each paragraph | replaces the paragraph; the original stays in `note.json` as `raw` |
| `"transcript"` | the whole transcript, with times | is added at the top of the note, as Markdown |

| Key | Default | Meaning |
|---|---|---|
| `reads` | required | `"paragraphs"` or `"transcript"` |
| `provider` | required | The provider to send requests to |
| `model` | `""` | Sent only when set |
| `prompt` | `""` | Instructions sent with the input |
| `images` | `false` | `paragraphs` only: also send the screenshot shown during each paragraph |
| `batch_size` | `20` | `paragraphs` only: paragraphs in one request, 1 to 100 |
| `chunk_size` | `25000` | `transcript` only: characters of transcript in one request, 1000 to 1000000. A longer transcript is sent in parts, and one more request combines their answers |
| `request` | `{}` | Extra fields for the request body, for example `{ reasoning_effort = "low" }` |

notepipe adds instructions for the answer's format to the prompt, and sets `model`, `messages`, `stream` and
`response_format` itself; `request` cannot change those. Nothing is sent by default for sampling or length limits. A
failed request does not stop the run: the paragraphs it covered keep their text, a `transcript` stage adds nothing, and
the note gets a warning. When a `paragraphs` stage answers with empty text, the paragraph is removed; if a stage would
empty every paragraph, the text is kept and a warning is added.

The default file holds two `llm` stages:

| Stage | `reads` | What its prompt asks for |
|---|---|---|
| `proofread` | `paragraphs` | Fixes recognition errors, filler and punctuation without changing meaning or language; writes formulas as LaTeX |
| `summary` | `transcript` | A one-sentence overview, 3–6 key points and a timestamped outline, in the transcript's language |

### `filter`

Runs a program that changes the note, for processing that needs no language model. See
[docs/adapters.md](adapters.md#filter-programs).

| Key | Default | Meaning |
|---|---|---|
| `command` | required | The program |
| `args` | `[]` | Its arguments |

### `note`

Writes the note, the output of the run: the note folder with `note.md`, `note.json` and `images/` (see
[docs/cli.md](cli.md#output)).

| Key | Option | Default | Meaning |
|---|---|---|---|
| `dir` | `-o`, `--out` | `"."` | Folder that receives the note folders; `~` is your home folder |

## Environment variables

| Variable | Meaning |
|---|---|
| the names in `api_key_env` | API keys of providers |
| `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` | Configuration and cache locations |
| `RUST_LOG` | Log filter for `-v` diagnostics |

## Example

Speech on a local engine, AI stages on a local Ollama, and glossary terms fixed by a program of your own:

```toml
pipeline = ["download", "transcript", "screenshots", "fix-terms", "proofread", "note"]

[providers.llama]
command = "notepipe-asr-llama"
args = ["-hf", "ggml-org/Qwen3-ASR-1.7B-GGUF:Q8_0"]
concurrency = 1

[providers.ollama]
base_url = "http://localhost:11434/v1"
concurrency = 1

[stages.download]
type = "download"
command = "notepipe-source-ytdlp"

[stages.transcript]
type = "transcript"
language = "en"
sources = ["subtitles", "speech"]

[stages.transcript.speech]
provider = "llama"

[stages.screenshots]
type = "screenshots"

[stages.fix-terms]
type = "filter"
command = "fix-terms"
args = ["--glossary", "glossary.tsv"]

[stages.proofread]
type = "llm"
reads = "paragraphs"
provider = "ollama"
model = "qwen3:14b"
prompt = "Fix speech-recognition errors and filler in each item. Keep its meaning and language."

[stages.note]
type = "note"
dir = "~/notes"
```
