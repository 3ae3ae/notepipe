# Configuration reference

notepipe reads `config.toml` from `$XDG_CONFIG_HOME/notepipe/` when that variable is set, otherwise from
`%APPDATA%\notepipe\` on Windows and `~/.config/notepipe/` elsewhere. `--config FILE` uses another file, and `--config -`
reads the configuration from standard input; either may be TOML or JSON with the same structure.

Values are taken from, in order: command-line options, the configuration, built-in defaults. Unknown keys are errors, so
a typo is reported instead of being ignored. `notepipe config init` writes a commented template; `notepipe config show`
prints the effective values.

## `[defaults]`

| Key | Option | Default | Meaning |
|---|---|---|---|
| `out` | `-o`, `--out` | `"."` | Directory that receives the note folders |
| `transcript_source` | `--transcript-source` | `"auto"` | `auto`: subtitles if available, otherwise speech recognition; `subtitle`: subtitles only; `asr`: speech recognition only |
| `similarity` | `--similarity` | `0.85` | Screenshot sensitivity, greater than 0 and at most 1; higher takes more screenshots |
| `sample_interval` | `--sample-interval` | `1.0` | Seconds between sampled frames, at least 0.2 |
| `cooldown` | `--cooldown` | `10.0` | Minimum seconds between screenshots; a final screenshot at the end of the recording may come sooner |
| `screenshot_mode` | `--screenshot-mode` | `"stable"` | `stable` waits for the picture to settle; `first` captures at the first change |
| `stable_secs` | `--stable-secs` | `0.8` | How long the picture must stay unchanged in `stable` mode |
| `roi` | `--roi` | none | Only this region is compared, as `left%,top%-right%,bottom%` (for example `"40%,0%-100%,100%"`) |
| `on_complete` | — | `[]` | Command run after each note is written; see [docs/cli.md](cli.md#completion-hook) |

## `[source]`

| Key | Default | Meaning |
|---|---|---|
| `command` | `[]` | Source adapter for URLs, as an argument list. When empty, a single executable named `notepipe-source-*` is used if one is found. See [docs/adapters.md](adapters.md#source-adapters). |

## `[asr]` — speech recognition

| Key | Option | Default | Meaning |
|---|---|---|---|
| `mode` | `--asr-mode` | `"transcriptions"` | `transcriptions`: OpenAI-compatible `/audio/transcriptions`; `chat`: audio input to `/chat/completions` |
| `base_url` | `--asr-base-url` | `""` | Service root (for example `https://api.groq.com/openai/v1`) or the full endpoint; ignored when `command` is set |
| `api_key` | `--asr-api-key` | `""` | Sent as `Authorization: Bearer`; when empty, the CLI uses `NOTEPIPE_ASR_API_KEY` |
| `model` | `--asr-model` | `""` | Sent only when set |
| `language` | `--asr-language` | `""` | Spoken language as a code such as `en` or `ko`; detected when empty. Also the preferred subtitle language |
| `concurrency` | — | `4` | Parallel requests, 1 to 16 |
| `command` | — | `[]` | Speech adapter to start for the run; see [docs/adapters.md](adapters.md#speech-adapters) |
| `max_speech` | `--max-speech` | `20.0` | Longest speech segment sent in one request, 1 to 600 seconds |
| `vad_threshold` | — | `0.5` | Speech detection threshold, 0 to 1; lower keeps quieter speech |
| `min_silence` | — | `0.4` | Pauses shorter than this many seconds do not split segments |

Speech recognition runs when no subtitles are found, or with `--transcript-source asr`; subtitles that are found but
cannot be read are an error. When an interactive run finds no subtitles and `[asr]` is empty, it asks for these settings
and saves them. With `--transcript-source asr`, empty settings are an error instead.

## `[llm]` — AI stages

| Key | Option | Default | Meaning |
|---|---|---|---|
| `base_url` | `--llm-base-url` | `""` | OpenAI-compatible endpoint root used by every stage unless the stage overrides it |
| `api_key` | `--llm-api-key` | `""` | Sent as `Authorization: Bearer`; when empty, the CLI uses `NOTEPIPE_LLM_API_KEY` |
| `model` | `--llm-model` | `""` | Model used by every stage unless the stage overrides it |
| `concurrency` | — | `8` | Parallel requests per stage, 1 to 16 |
| `request` | — | `{}` | Extra fields merged into every request body, for example `{ reasoning_effort = "low" }` |
| `stages` | `--stages`, `--no-stages` | `[]` | The stages to run, in order (see below) |

Nothing is sent by default for sampling or length limits; add fields such as `temperature` or `max_tokens` through
`request` if your model needs them. `model`, `messages`, `stream` and `response_format` are reserved and cannot be set
through `request`.

### `[[llm.stages]]`

| Key | Default | Meaning |
|---|---|---|
| `name` | required | `proofread` and `summary` are built in; any other name defines a custom stage. Names must be unique. |
| `input` | built-in or required | `paragraphs`: the note's paragraphs, in batches; `transcript`: the whole timestamped transcript |
| `output` | built-in or required | For `paragraphs`: `replace` or `below`. For `transcript`: `top` |
| `prompt` | built-in or required | Instructions for the model. For built-in stages, replaces the default prompt. |
| `images` | `false` | `paragraphs` only: also send the screenshot shown during each paragraph |
| `base_url`, `api_key`, `model` | inherited | Override the `[llm]` connection for this stage |
| `request` | `{}` | Merged after `[llm].request` |

| Stage | Input → output | Default prompt |
|---|---|---|
| `proofread` | `paragraphs` → `replace` | Fixes recognition errors, filler and punctuation without changing meaning or language; writes formulas as LaTeX |
| `summary` | `transcript` → `top` | A one-sentence overview, 3–6 key points and a timestamped outline, in the transcript's language |

With `replace`, an empty result removes the paragraph; if a stage would empty every paragraph, the text is kept and a
warning is added. With `below`, an empty result or one identical to the paragraph adds nothing. `--stages a,b` runs the
named built-in or configured stages for one run; `--llm-prompt` replaces the `proofread` prompt for one run.

## Environment variables

| Variable | Meaning |
|---|---|
| `NOTEPIPE_ASR_API_KEY` | Speech API key when `[asr].api_key` is empty (CLI only) |
| `NOTEPIPE_LLM_API_KEY` | AI API key when `[llm].api_key` is empty (CLI only) |
| `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` | Configuration and cache locations |
| `RUST_LOG` | Log filter for `-v` diagnostics |

## Example

```toml
[defaults]
out = "~/notes"

[asr]
base_url = "https://api.groq.com/openai/v1"
model = "whisper-large-v3-turbo"

[llm]
base_url = "http://localhost:11434/v1"   # Ollama
model = "qwen3:14b"

[[llm.stages]]
name = "proofread"
images = true

[[llm.stages]]
name = "glossary"
input = "transcript"
output = "top"
prompt = "List the technical terms used in this recording with a one-line explanation of each, as a Markdown list under a level-2 heading named Glossary."
```
