# notepipe

Turn video and audio into Markdown notes: the transcript arranged around screenshots of what was on screen, optionally
proofread, summarized or extended by a language model.

notepipe is a small engine with a command-line interface. It is useful on its own, and other programs can drive it.
Speech recognition and language models are reached over standard HTTP APIs; local engines and online sources plug in as
separately installed [adapters](docs/adapters.md).

## Features

- **Local files and URLs** — video or audio files on your computer; web pages through a source adapter such as the
  official [yt-dlp adapter](adapters/ytdlp).
- **Subtitles first** — existing subtitles are used when available; otherwise speech is transcribed.
- **Any speech service** — OpenAI-compatible transcription APIs, audio-capable chat models, or a local engine through an
  adapter.
- **Screenshots** — a screenshot is taken whenever the picture changes and placed next to what was said.
- **Prompt-driven AI stages** — proofreading, summaries, translations or anything else you can describe in a prompt, with
  any OpenAI-compatible model.
- **Plain output** — a folder per note with `note.md` (YAML front matter and Markdown), `images/` and `note.json`.

## Install

notepipe needs [ffmpeg](https://ffmpeg.org/download.html) on your `PATH`.

With [uv](https://docs.astral.sh/uv/):

```sh
uv tool install notepipe                                  # local files only
uv tool install notepipe --with notepipe-source-ytdlp     # also URLs (via yt-dlp)
```

Add other [adapters](#adapters) the same way with more `--with` options.

With Cargo: `cargo install notepipe`. Prebuilt binaries are attached to each
[GitHub release](https://github.com/3ae3ae/notepipe/releases).

On Android, install [Termux](https://termux.dev), then `pkg install rust ffmpeg uv` and use either method above.

## Quick start

```sh
notepipe lecture.mp4
notepipe interview.m4a -o ~/notes
notepipe https://www.youtube.com/watch?v=VIDEO_ID      # needs a source adapter
```

If a recording has no usable subtitles and no speech service is configured, the first interactive run asks for one and
saves it. `notepipe doctor` checks your setup.

## Configuration

Settings live in `~/.config/notepipe/config.toml` (`%APPDATA%\notepipe\config.toml` on Windows; `$XDG_CONFIG_HOME` is
honored). `notepipe config init` writes a commented template and `notepipe config show` prints the effective settings.
Command-line options override the file.

```toml
[asr]                                      # speech recognition
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini-transcribe"
language = "ko"                            # optional; detected when empty

[llm]                                      # connection shared by all AI stages
base_url = "https://api.openai.com/v1"
model = "gpt-5.6-luna"

[[llm.stages]]                             # stages run in this order
name = "proofread"

[[llm.stages]]
name = "summary"
model = "gpt-5.6-terra"                    # per-stage overrides: base_url, api_key, model, prompt, request
request = { reasoning_effort = "medium" }  # extra fields merged into the request body
```

API keys can stay out of the file: `NOTEPIPE_ASR_API_KEY` and `NOTEPIPE_LLM_API_KEY` are used when the file has none.

### Speech recognition

| Service | `[asr]` settings |
|---|---|
| OpenAI | `base_url = "https://api.openai.com/v1"`, `model = "gpt-4o-mini-transcribe"` |
| Groq | `base_url = "https://api.groq.com/openai/v1"`, `model = "whisper-large-v3-turbo"` |
| Mistral | `base_url = "https://api.mistral.ai/v1"`, `model = "voxtral-mini-latest"` |
| Audio-capable chat models | `mode = "chat"` with the provider's `base_url` and `model` |
| xAI | `command = ["notepipe-asr-xai"]` — [adapter](adapters/xai) |
| Meta | `command = ["notepipe-asr-meta"]` — [adapter](adapters/meta) |
| llama.cpp (local, any platform) | `command = ["notepipe-asr-llama", "-hf", "ggml-org/Qwen3-ASR-1.7B-GGUF:Q8_0"]`, `concurrency = 1` — [adapter](adapters/llama) |
| Intel NPU (local) | `command = ["notepipe-asr-openvino"]`, `concurrency = 1` — [adapter](adapters/openvino) |
| Apple silicon (local) | `command = ["notepipe-asr-apple"]`, `concurrency = 1` — [adapter](adapters/apple) |

When `command` is set, notepipe starts the adapter for the run and stops it afterwards. Other tuning: `max_speech`
(longest segment, seconds), `vad_threshold` and `min_silence` (speech detection).

### AI stages

A stage applies a prompt either to the paragraphs of the note or to the whole transcript:

| `input` | `output` | Effect |
|---|---|---|
| `paragraphs` | `replace` | each paragraph is replaced by the model's text (an empty result removes it) |
| `paragraphs` | `below` | the model's text is added under each paragraph |
| `transcript` | `top` | the model's Markdown is added at the top of the note |

`proofread` (paragraphs → replace) and `summary` (transcript → top) are built in with default prompts; set `prompt` to
change what they do. Any other name defines your own stage, for example bilingual notes:

```toml
[[llm.stages]]
name = "translate"
input = "paragraphs"
output = "below"
prompt = "Translate each item into Korean. Keep the meaning, terminology and tone; do not summarize, add or omit content. Keep names, code and formulas unchanged. Return an item unchanged if it is already in Korean."
```

Set `images = true` on a paragraphs stage to send each paragraph's screenshot along. `--stages proofread,summary` picks
stages for one run and `--no-stages` runs none. Any OpenAI-compatible chat endpoint works:

| Provider | `base_url` |
|---|---|
| OpenAI | `https://api.openai.com/v1` |
| Anthropic | `https://api.anthropic.com/v1` |
| Google Gemini | `https://generativelanguage.googleapis.com/v1beta/openai` |
| OpenRouter | `https://openrouter.ai/api/v1` |
| Groq | `https://api.groq.com/openai/v1` |
| Ollama (local) | `http://localhost:11434/v1` |
| LM Studio (local) | `http://localhost:1234/v1` |

## Output

```
<out>/<title>/
├── note.md        YAML front matter, stage output at the top, then the notes
├── images/        screenshots referenced by note.md
└── note.json      the same note as structured data
```

Running again on the same source replaces its note. Intermediate results are cached per source, so changing only AI
settings reuses the download, screenshots and transcript; `--no-resume` starts over. `[defaults].on_complete` runs a
command after each note is written, for example to convert or sync it.

## Using notepipe from other programs

Pass the configuration on standard input and read newline-delimited JSON events:

```sh
notepipe "$SOURCE" -o "$OUT" --config - --json < config.toml
```

See [docs/cli.md](docs/cli.md) for the events, exit codes, `note.json` and the completion hook.

## Adapters

| Package | Kind | Purpose |
|---|---|---|
| [`notepipe-source-ytdlp`](adapters/ytdlp) | source | URLs from any site yt-dlp supports |
| [`notepipe-asr-llama`](adapters/llama) | speech | llama.cpp on CPU or GPU |
| [`notepipe-asr-openvino`](adapters/openvino) | speech | Intel NPU (or CPU/GPU) with OpenVINO |
| [`notepipe-asr-apple`](adapters/apple) | speech | Apple silicon (not yet verified on hardware) |
| [`notepipe-asr-xai`](adapters/xai) | speech | xAI speech-to-text |
| [`notepipe-asr-meta`](adapters/meta) | speech | Meta speech-to-text |

[docs/adapters.md](docs/adapters.md) describes the contracts and how to write your own.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE). notepipe started as a fork of [course2md](https://github.com/mizorewww/course2md) by mizorewww.
