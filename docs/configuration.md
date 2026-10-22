# Configuration reference

notepipe reads `config.toml` from `$XDG_CONFIG_HOME/notepipe/` when that variable is set, otherwise from
`%APPDATA%\notepipe\` on Windows and `~/.config/notepipe/` elsewhere. `notepipe config path` prints the location.
`--config FILE` uses another file, and `--config -` reads the configuration from standard input; either may be TOML or
JSON with the same structure.

```sh
notepipe config init    # create the file with the default settings
notepipe config path    # print where it is
notepipe config show    # show the settings in effect
```

The first run creates the file when there is none and says where it is; `config init` writes the same file. It holds
every setting that has a value by default, with that value; the services `speech` and `ai`, waiting for an address; and
the stages `proofread` and `summary` with their prompts. What the file says is what runs: change a value, delete a stage
or add your own. Settings that start empty are left out of it and described below.

Values are taken from, in order: command-line options, the configuration, the defaults below. Unknown keys are errors,
so a typo is reported instead of being ignored.

## How the parts fit

A connection to something outside notepipe is written once, under `[services]`, and every step that needs one names it:
`[speech]` for speech recognition, and each of the `[[stages]]`. The service says where requests go and how they are
authenticated; the step says what it asks for, such as the API, the model and the prompt.

```toml
[services.openai]
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"

[speech]
service = "openai"
model = "gpt-4o-mini-transcribe"

[[stages]]
name = "summary"
service = "openai"
model = "gpt-5.6-luna"
input = "transcript"
output = "top"
prompt = "Summarize the recording in five bullet points."
```

A service can also be an [adapter](adapters.md#service-adapters): a program that notepipe starts and that answers in the
same format. That is how local engines are attached, and how a stage can be handled by something that is not a language
model at all.

Command-line options change what a run does with the note: its input, transcript, screenshots, stages and output. Where
requests go is set only in the configuration; `--config -` passes one for a single run.

## `[services.NAME]`

| Key | Default | Meaning |
|---|---|---|
| `base_url` | `""` | Service root (for example `https://api.groq.com/openai/v1`) or the full endpoint of an OpenAI-compatible API |
| `adapter` | `[]` | Program to start instead, as an argument list; see [docs/adapters.md](adapters.md#service-adapters) |
| `api_key` | `""` | Sent as `Authorization: Bearer <key>` |
| `api_key_env` | `""` | Name of an environment variable that holds the key, such as `OPENAI_API_KEY`; read when a step uses the service |
| `concurrency` | `4` | Requests sent to it at the same time, 1 to 16. Local engines usually want `1` |

A service is connected when it has a `base_url` or an `adapter`, but not both. One with neither stays in the file for you
to fill in: a stage that uses it is skipped with a warning, and speech recognition asks for it in a terminal and fails
elsewhere. Naming a service that does not exist is an error, and so is setting both `api_key` and `api_key_env`, or
naming a variable that is not set.

## `[output]`

| Key | Option | Default | Meaning |
|---|---|---|---|
| `dir` | `-o`, `--out` | `"."` | Folder that receives the note folders; `~` is your home folder |
| `on_complete` | — | `[]` | Program run after each note is written, as an argument list; see [docs/cli.md](cli.md#completion-hook) |

## `[transcript]`

| Key | Option | Default | Meaning |
|---|---|---|---|
| `source` | `--transcript` | `"auto"` | `auto`: subtitles when there are any, otherwise speech recognition; `subtitles`: subtitles only; `speech`: speech recognition only |
| `language` | `--language` | `""` | Spoken language as a code such as `en` or `ko`; sent to speech recognition and used to choose among subtitles. Empty: detect |

Subtitles are `NAME.srt` or `NAME.vtt` next to a local `NAME.mp4`, or those a download adapter delivers. `--subtitle FILE`
takes precedence over all three sources. Subtitles that are found but cannot be read are an error.

## `[speech]` — speech recognition

| Key | Option | Default | Meaning |
|---|---|---|---|
| `service` | — | `""` | The service that recognizes speech |
| `api` | — | `"transcriptions"` | `transcriptions`: OpenAI-compatible `/audio/transcriptions`; `chat`: audio input to `/chat/completions` |
| `model` | — | `""` | Sent only when set |

When a run in a terminal needs speech recognition and the service is not connected, notepipe asks for the connection and
saves it to that service, or to a new service named `speech` when `[speech].service` is empty.

### `[speech.detection]`

Speech is located with a voice activity detector and cut into segments before it is sent. The defaults suit most
recordings.

| Key | Option | Default | Meaning |
|---|---|---|---|
| `threshold` | — | `0.5` | Speech confidence from 0 to 1; lower keeps quieter speech |
| `min_silence` | — | `0.4` | Pauses shorter than this many seconds do not split segments |
| `max_segment` | `--max-segment` | `20.0` | Longest stretch of speech in one request, 1 to 600 seconds; up to 0.25 s of silence is added on each side |

## `[screenshots]`

| Key | Option | Default | Meaning |
|---|---|---|---|
| `mode` | `--screenshot-mode` | `"stable"` | `stable` waits for the picture to stop changing; `first` takes the screenshot at the first change |
| `sensitivity` | `--sensitivity` | `0.85` | How small a change triggers a screenshot, greater than 0 and at most 1; higher takes more |
| `frame_interval` | `--frame-interval` | `1.0` | Seconds between compared frames, at least 0.2 |
| `min_gap` | `--min-gap` | `10.0` | Least seconds between screenshots; a final screenshot at the end of the recording may come sooner |
| `settle` | `--settle` | `0.8` | In `stable` mode, seconds the picture must stay unchanged |
| `region` | `--region` | `""` | Only this part of the frame is compared, as `left%,top%-right%,bottom%` (for example `"40%,0%-100%,100%"`) |

Audio files have no screenshots.

## `[[stages]]`

Each block adds a stage; stages run in the order of the blocks once the transcript is ready. A stage sends each
paragraph, or the whole transcript, to its service and puts the answer into the note. notepipe has no stages of its own:
the two in the file it creates are ordinary stages like any you add.

| Key | Default | Meaning |
|---|---|---|
| `name` | required | Names the stage in progress output, warnings and `--stages`; names must be unique |
| `service` | required | The service the stage sends its input to |
| `model` | `""` | Sent only when set |
| `input` | required | `paragraphs`: the note's paragraphs, in batches; `transcript`: the whole timestamped transcript |
| `output` | required | For `paragraphs`: `replace` or `below`. For `transcript`: `top` |
| `prompt` | `""` | Instructions sent with the input |
| `images` | `false` | `paragraphs` only: also send the screenshot shown during each paragraph |
| `request` | `{}` | Extra fields merged into each request body, for example `{ reasoning_effort = "low" }` |

Nothing is sent by default for sampling or length limits; add fields such as `temperature` or `max_tokens` through
`request` if your model needs them. `model`, `messages`, `stream` and `response_format` are set by notepipe and cannot be
set through `request`.

The file notepipe creates holds these two, both using the service `ai`:

| Stage | Input → output | What its prompt asks for |
|---|---|---|
| `proofread` | `paragraphs` → `replace` | Fixes recognition errors, filler and punctuation without changing meaning or language; writes formulas as LaTeX |
| `summary` | `transcript` → `top` | A one-sentence overview, 3–6 key points and a timestamped outline, in the transcript's language |

A stage whose service is not connected is skipped with a warning, so the created file works before you set up an AI
service; delete the stages you do not want.

With `replace`, an empty result removes the paragraph; if a stage would empty every paragraph, the text is kept and a
warning is added. With `below`, an empty result or one identical to the paragraph adds nothing. `--stages a,b` runs the
named stages of the configuration for one run, and `--no-stages` runs none.

## `[download]`

| Key | Default | Meaning |
|---|---|---|
| `adapter` | `[]` | Download adapter for URLs, as an argument list. When empty, the one installed program named `notepipe-source-*` is used. See [docs/adapters.md](adapters.md#download-adapters). |

## Environment variables

| Variable | Meaning |
|---|---|
| the names in `api_key_env` | API keys of services |
| `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` | Configuration and cache locations |
| `RUST_LOG` | Log filter for `-v` diagnostics |

## Example

Speech through Groq, stages through a local Ollama:

```toml
[services.groq]
base_url = "https://api.groq.com/openai/v1"
api_key_env = "GROQ_API_KEY"

[services.ollama]
base_url = "http://localhost:11434/v1"
concurrency = 1

[output]
dir = "~/notes"

[transcript]
language = "en"

[speech]
service = "groq"
model = "whisper-large-v3-turbo"

[[stages]]
name = "proofread"
service = "ollama"
model = "qwen3:14b"
input = "paragraphs"
output = "replace"
images = true
prompt = "Fix speech-recognition errors and filler in each item. Keep its meaning and language."

[[stages]]
name = "glossary"
service = "ollama"
model = "qwen3:14b"
input = "transcript"
output = "top"
prompt = "List the technical terms used in this recording with a one-line explanation of each, as a Markdown list under a level-2 heading named Glossary."
```
