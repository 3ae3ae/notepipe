# Adapters

The notepipe core turns a local media file into a note and talks to the outside world only through standard interfaces.
Anything tied to a particular engine, vendor or website is an **adapter**: a separate program that notepipe runs. To
attach one, install it; a speech or AI adapter is also named in `config.toml`. To detach one, uninstall it or remove its
line.

There are three kinds:

- **Speech adapters** serve an OpenAI-compatible transcription API on localhost for the duration of a run.
- **AI adapters** serve an OpenAI-compatible chat API on localhost for the stages of a run.
- **Download adapters** turn a URL into a local media file plus metadata and subtitles.

Official adapters are extras of the `notepipe` package, for example `uv tool install "notepipe[llama]"`. Other adapters
install next to notepipe with `uv tool install notepipe --with <package>`.
notepipe looks for adapter programs in its own directory first, then on `PATH`.

## Official adapters

| Extra | Package | Kind | Notes |
|---|---|---|---|
| `ytdlp` | [`notepipe-source-ytdlp`](../adapters/ytdlp) | source | Any site yt-dlp supports; honors your yt-dlp configuration files |
| `llama` | [`notepipe-asr-llama`](../adapters/llama) | speech | Runs `llama-server`; strips Qwen3-ASR output markers |
| `openvino` | [`notepipe-asr-openvino`](../adapters/openvino) | speech | Intel NPU by default; `--device CPU` or `GPU` also work |
| `speech-swift` | [`notepipe-asr-speech-swift`](../adapters/speech-swift) | speech | Apple silicon (macOS 15 or later) with MLX and Core ML through speech-swift; built in CI, not yet verified on hardware |
| `xai` | [`notepipe-asr-xai`](../adapters/xai) | speech | Forwards to xAI's speech-to-text API |
| `meta` | [`notepipe-asr-meta`](../adapters/meta) | speech | Forwards to Meta's speech-to-text API |

## Speech adapters

### Using one

```toml
[speech]
adapter = ["notepipe-asr-llama", "-hf", "ggml-org/Qwen3-ASR-1.7B-GGUF:Q8_0"]
concurrency = 1
```

When `adapter` is set, notepipe:

1. starts the program with `--host 127.0.0.1 --port <free port>` appended to the configured arguments;
2. waits until `GET /health` answers `200` (up to 30 minutes, because a first run may download a model);
3. sends every speech segment to `http://127.0.0.1:<port>/v1` using `[speech].api` (`transcriptions` by default);
4. stops the program and its child processes when transcription ends, fails or is cancelled.

`base_url` is ignored while `adapter` is set; `model`, `language` and `api_key` are sent as for any endpoint. Local
engines usually handle one request at a time, so set `concurrency = 1`.

A server you run yourself needs no adapter: point `base_url` at it.

### Contract

An adapter is any executable that accepts these arguments and serves this API.

| Argument | Meaning |
|---|---|
| `--host HOST` | Address to listen on; bind only to it (notepipe passes `127.0.0.1`). |
| `--port PORT` | Port to listen on. |

Other arguments are the adapter's own; users write them in `adapter`.

`GET /health` returns `200` once the adapter can transcribe; before that, return `503` or refuse connections. If the
process exits, notepipe reports its last output lines.

`POST /v1/audio/transcriptions` takes `multipart/form-data`, compatible with OpenAI's endpoint:

| Field | Required | Content |
|---|---|---|
| `model` | no | `[speech].model`, when set |
| `language` | no | `[transcript].language`, when set, as written in the configuration (for example `ko`) |
| `file` | yes | one speech segment: WAV, 16 kHz, mono, 16-bit PCM, about `[speech.detection].max_segment` seconds or less (20 by default) plus up to half a second of padding; always the last part |

Respond `200` with `{"text": "…"}`, using an empty string when there is no speech. On failure respond with a non-2xx
status and `{"error": {"message": "…"}}`. If `[speech].api_key` is set, requests carry `Authorization: Bearer <key>`; an
adapter may check it or forward it.

Return clean text: model-specific markers or formatting are the adapter's job to remove.

### Example

A complete adapter in Python, using only the standard library:

```python
#!/usr/bin/env python3
import argparse
import email
import json
from http.server import BaseHTTPRequestHandler, HTTPServer


def transcribe(wav_bytes, language):
    return ""  # call your engine here


class Handler(BaseHTTPRequestHandler):
    def reply(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == "/health":
            self.reply(200, {"status": "ok"})
        else:
            self.reply(404, {"error": {"message": "not found"}})

    def do_POST(self):
        if self.path != "/v1/audio/transcriptions":
            return self.reply(404, {"error": {"message": "not found"}})
        try:
            length = int(self.headers["Content-Length"])
            raw = b"Content-Type: " + self.headers["Content-Type"].encode() + b"\r\n\r\n" + self.rfile.read(length)
            parts = {
                part.get_param("name", header="content-disposition"): part.get_payload(decode=True)
                for part in email.message_from_bytes(raw).get_payload()
            }
            wav = parts["file"]
            language = (parts.get("language") or b"").decode() or None
        except Exception:
            return self.reply(400, {"error": {"message": "expected a multipart upload with a file field"}})
        try:
            self.reply(200, {"text": transcribe(wav, language)})
        except Exception as error:
            self.reply(500, {"error": {"message": str(error)}})


parser = argparse.ArgumentParser()
parser.add_argument("--host", required=True)
parser.add_argument("--port", type=int, required=True)
args = parser.parse_args()
HTTPServer((args.host, args.port), Handler).serve_forever()
```

## AI adapters

### Using one

```toml
[ai]
adapter = ["my-ai-adapter", "--option"]   # every stage that sets neither base_url nor adapter

[[stages]]
name = "glossary-terms"
input = "paragraphs"
output = "replace"
adapter = ["my-term-fixer"]               # this stage only; it needs no prompt
```

For each adapter command that a stage uses, notepipe:

1. starts the program with `--host 127.0.0.1 --port <free port>` appended to the configured arguments, before the first
   stage that uses it;
2. waits until `GET /health` answers `200` (up to 30 minutes);
3. sends the requests of the stages that use it to `http://127.0.0.1:<port>/v1/chat/completions`;
4. stops the program and its child processes when the stages end, fail or are cancelled.

`[ai].base_url` is ignored while `[ai].adapter` is set, and `model` becomes optional for stages that use an adapter. Responses
are cached by the adapter command and the request, so a later run with the same adapter reuses them even though the port
changes.

An AI adapter is the place for anything that is not a plain Chat Completions endpoint: a different API, such as the
OpenAI Responses API; a provider with its own authentication; or access through a subscription you already have. It
need not use a language model at all. A stage can be handled by a program that applies fixed rules, replaces terms from
a glossary or calls any other tool, as long as it answers in the format below.

### Contract

Arguments, `GET /health` and process handling are the same as for [speech adapters](#contract).

`POST /v1/chat/completions` takes a JSON body in the OpenAI Chat Completions format:

| Field | Present | Content |
|---|---|---|
| `model` | when the stage has a model | the model name |
| `messages` | always | a system message with the stage's prompt and notepipe's output instructions, then a user message with the input (below) |
| `response_format` | paragraph stages | `{"type": "json_object"}` |
| other fields | when configured | everything in `[ai].request` and the stage's `request` |

The input, and the answer notepipe expects in `content`:

- **Paragraph stages:** the user content is an array of parts. The first is a `text` part holding a JSON array of up to
  20 paragraphs, `[{"id": 0, "text": "…"}, …]`; with `images = true` an `image_url` part with the screenshot follows, as
  a JPEG data URL. Answer with `{"segments": [{"id": 0, "text": "…"}, …]}`, one entry for every input `id`.
- **Transcript stages:** the user content is a string: the title and author when they are known, then the transcript,
  one line per fragment starting with its start time, such as `[03:12]` or `[1:03:12]`. Answer with Markdown. A long transcript arrives in several
  requests, followed by one that asks to combine their results.

`stream` is never requested. Respond `200` with:

```json
{"choices": [{"message": {"content": "…"}}], "usage": {"prompt_tokens": 120, "completion_tokens": 40}}
```

`usage` is optional; when present, notepipe reports the counts. On failure respond with a non-2xx status and
`{"error": {"message": "…"}}`; `429` and `5xx` responses are retried with backoff.

An adapter may ignore `response_format`, because the prompt asks for JSON as well. If it rejects the field instead, it
should answer `400` with a message that mentions `response_format`; notepipe then retries without it. When the stage has
an API key (`api_key`, or `NOTEPIPE_AI_API_KEY`), requests carry `Authorization: Bearer <key>`; an adapter may check it
or forward it.

### Example

A complete AI adapter in Python, using only the standard library:

```python
#!/usr/bin/env python3
import argparse
import json
from http.server import BaseHTTPRequestHandler, HTTPServer


def complete(request):
    # request is the Chat Completions body; return the model's answer as text.
    return "Call your model here."


class Handler(BaseHTTPRequestHandler):
    def reply(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == "/health":
            self.reply(200, {"status": "ok"})
        else:
            self.reply(404, {"error": {"message": "not found"}})

    def do_POST(self):
        if self.path != "/v1/chat/completions":
            return self.reply(404, {"error": {"message": "not found"}})
        try:
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            request["messages"]
        except Exception:
            return self.reply(400, {"error": {"message": "expected a chat completions request"}})
        try:
            text = complete(request)
        except Exception as error:
            return self.reply(500, {"error": {"message": str(error)}})
        self.reply(200, {"choices": [{"message": {"role": "assistant", "content": text}}]})


parser = argparse.ArgumentParser()
parser.add_argument("--host", required=True)
parser.add_argument("--port", type=int, required=True)
args = parser.parse_args()
HTTPServer((args.host, args.port), Handler).serve_forever()
```

## Download adapters

### Using one

For an `http(s)` source, notepipe runs `[download].adapter`. When it is not set, notepipe looks for executables whose
names start with `notepipe-source-`: if it finds exactly one it uses it, and if it finds several it asks you to choose
with `[download].adapter`.

```toml
[download]
adapter = ["notepipe-source-ytdlp", "--", "--cookies-from-browser", "firefox"]
```

Results are cached per URL and adapter. Media is deleted just before the note is written, so a later run that only
changes stages reuses the cached screenshots and transcript, while one that needs the media again downloads it again.

### Contract

notepipe runs:

```
<command…> <URL> --out <DIR>
```

`DIR` is an empty directory owned by notepipe. The adapter downloads the media, and any subtitles, into it.

Standard output carries JSON objects, one per line, each at most 1 MiB. Optional progress lines look like
`{"progress": 0.42}` (0 to 1). The last line is the result:

```json
{
  "media": "media.mp4",
  "title": "…",
  "author": "…",
  "duration": 123.4,
  "language": "en",
  "url": "https://…",
  "seek_url": "https://…?t={t}",
  "subtitles": [{"path": "media.en.vtt", "language": "en", "automatic": false}]
}
```

Only `media` is required. Paths are relative to `DIR` and must stay inside it. Subtitles must be SRT or VTT; `automatic`
marks machine-generated captions. `seek_url`, if present, is a URL template in which `{t}` is replaced by whole seconds to
open the item at a given time; without it, notes link to the source URL.

Write diagnostics to standard error. Exit with status 0 on success and anything else on failure; notepipe shows the last
lines of standard error.

## Publishing an adapter

Name the executable `notepipe-asr-<name>`, `notepipe-ai-<name>` or `notepipe-source-<name>`, and publish it as a package
that installs that executable (a Python console script or a binary wheel works well), so that `uv tool install notepipe
--with <package>` places it next to notepipe.
