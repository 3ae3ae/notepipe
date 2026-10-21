# Configuration reference

notepipe reads `config.toml` from `$XDG_CONFIG_HOME/notepipe/` when that variable is set, otherwise from
`%APPDATA%\notepipe\` on Windows and `~/.config/notepipe/` elsewhere. `notepipe config path` prints the location.
`--config FILE` uses another file, and `--config -` reads the configuration from standard input; either may be TOML or
JSON with the same structure.

```sh
notepipe config init    # create the file, with every setting explained in comments
notepipe config path    # print where it is
notepipe config show    # show the settings in effect
```

Values are taken from, in order: command-line options, the configuration, built-in defaults. Unknown keys are errors, so
a typo is reported instead of being ignored. The sections follow the order in which a note is made.

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

When an interactive `auto` run finds no subtitles and `[speech]` is empty, it asks for the speech settings and saves
them; with `source = "speech"`, empty settings are an error.

## `[speech]` — speech recognition

| Key | Option | Default | Meaning |
|---|---|---|---|
| `base_url` | `--speech-base-url` | `""` | Service root (for example `https://api.groq.com/openai/v1`) or the full endpoint; ignored when `adapter` is set |
| `api_key` | `--speech-api-key` | `""` | Sent as `Authorization: Bearer`; when empty, `NOTEPIPE_SPEECH_API_KEY` is used |
| `model` | `--speech-model` | `""` | Sent only when set |
| `api` | `--speech-api` | `"transcriptions"` | `transcriptions`: OpenAI-compatible `/audio/transcriptions`; `chat`: audio input to `/chat/completions` |
| `adapter` | — | `[]` | Speech adapter to start for the run, as an argument list; see [docs/adapters.md](adapters.md#speech-adapters) |
| `concurrency` | — | `4` | Parallel requests, 1 to 16 |

### `[speech.detection]`

Speech is located with a voice activity detector and cut into segments before it is sent. The defaults suit most
recordings.

| Key | Option | Default | Meaning |
|---|---|---|---|
| `threshold` | — | `0.5` | Speech confidence from 0 to 1; lower keeps quieter speech |
| `min_silence` | — | `0.4` | Pauses shorter than this many seconds do not split segments |
| `max_segment` | `--max-segment` | `20.0` | Longest segment sent in one request, 1 to 600 seconds |

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

## `[ai]` — AI stages

| Key | Option | Default | Meaning |
|---|---|---|---|
| `base_url` | `--ai-base-url` | `""` | OpenAI-compatible chat service used by every stage that does not set its own |
| `api_key` | `--ai-api-key` | `""` | Sent as `Authorization: Bearer`; when empty, `NOTEPIPE_AI_API_KEY` is used |
| `model` | `--ai-model` | `""` | Model for every stage that does not set its own |
| `concurrency` | — | `8` | Parallel requests per stage, 1 to 16 |
| `request` | — | `{}` | Extra fields merged into every request body, for example `{ reasoning_effort = "low" }` |

Nothing is sent by default for sampling or length limits; add fields such as `temperature` or `max_tokens` through
`request` if your model needs them. `model`, `messages`, `stream` and `response_format` are set by notepipe and cannot be
set through `request`.

### `[[ai.stages]]`

Each block adds a stage; stages run in the order of the blocks. No stage runs unless one is configured or selected with
`--stages`.

| Key | Default | Meaning |
|---|---|---|
| `name` | required | `proofread` and `summary` are built in; any other name defines a stage of your own. Names must be unique. |
| `input` | built-in or required | `paragraphs`: the note's paragraphs, in batches; `transcript`: the whole timestamped transcript |
| `output` | built-in or required | For `paragraphs`: `replace` or `below`. For `transcript`: `top` |
| `prompt` | built-in or required | Instructions for the model. For a built-in stage, replaces its prompt. |
| `images` | `false` | `paragraphs` only: also send the screenshot shown during each paragraph |
| `base_url`, `api_key`, `model` | inherited | Override the `[ai]` connection for this stage |
| `request` | `{}` | Merged after `[ai].request` |

| Stage | Input → output | Default prompt |
|---|---|---|
| `proofread` | `paragraphs` → `replace` | Fixes recognition errors, filler and punctuation without changing meaning or language; writes formulas as LaTeX |
| `summary` | `transcript` → `top` | A one-sentence overview, 3–6 key points and a timestamped outline, in the transcript's language |

With `replace`, an empty result removes the paragraph; if a stage would empty every paragraph, the text is kept and a
warning is added. With `below`, an empty result or one identical to the paragraph adds nothing. `--stages a,b` runs the
named built-in or configured stages for one run, `--no-stages` runs none, and `--proofread-prompt` replaces the
`proofread` prompt for one run.

## `[download]`

| Key | Default | Meaning |
|---|---|---|
| `adapter` | `[]` | Download adapter for URLs, as an argument list. When empty, the one installed program named `notepipe-source-*` is used. See [docs/adapters.md](adapters.md#download-adapters). |

## Environment variables

| Variable | Meaning |
|---|---|
| `NOTEPIPE_SPEECH_API_KEY` | Speech API key when `[speech].api_key` is empty |
| `NOTEPIPE_AI_API_KEY` | AI API key when `[ai].api_key` is empty |
| `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` | Configuration and cache locations |
| `RUST_LOG` | Log filter for `-v` diagnostics |

## Example

```toml
[output]
dir = "~/notes"

[transcript]
language = "en"

[speech]
base_url = "https://api.groq.com/openai/v1"
model = "whisper-large-v3-turbo"

[ai]
base_url = "http://localhost:11434/v1"   # Ollama
model = "qwen3:14b"

[[ai.stages]]
name = "proofread"
images = true

[[ai.stages]]
name = "glossary"
input = "transcript"
output = "top"
prompt = "List the technical terms used in this recording with a one-line explanation of each, as a Markdown list under a level-2 heading named Glossary."
```
