# Adapters

The notepipe core turns a local media file into a note and talks to the outside world only through standard interfaces.
Anything tied to a particular engine, vendor or website is an **adapter**: a separate program that notepipe runs. To
attach one, install it and name it in `config.toml`; to detach one, uninstall it or remove its line.

There are two kinds:

- **Service adapters** serve an OpenAI-compatible API on localhost while a step needs them: the transcription API for
  speech recognition, the chat API for stages, or both. A [service](configuration.md#servicesname) names one in
  `adapter`.
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


## Service adapters

### Using one

```toml
[services.llama]
adapter = ["notepipe-asr-llama", "-hf", "ggml-org/Qwen3-ASR-1.7B-GGUF:Q8_0"]
concurrency = 1

[speech]
service = "llama"
```

When a step uses a service that has an `adapter`, notepipe:

1. starts the program with `--host 127.0.0.1 --port <free port>` appended to the configured arguments;
2. waits until `GET /health` answers `200` (up to 30 minutes, because a first run may download a model);
3. sends the step's requests to `http://127.0.0.1:<port>/v1`;
4. stops the program and its child processes when the step ends, fails or is cancelled.

Speech recognition is one step and the stages together are another, so stages that use the same service share one
process. The service's key is sent as for any endpoint. Responses are cached by the adapter command and the request, so
a later run reuses them even though the port changes. Local engines usually handle one request at a time, so give their
service `concurrency = 1`.

A server you run yourself needs no adapter: give the service its `base_url`.

An adapter is the place for anything that is not a plain OpenAI-compatible endpoint: a local engine, a different API
such as the OpenAI Responses API, a provider with its own authentication, or access through a subscription you already
have. A chat adapter need not use a language model at all. A stage can be handled by a program that applies fixed
rules, replaces terms from a glossary or calls any other tool, as long as it answers in the format below:

```toml
[services.terms]
adapter = ["my-term-fixer"]

[[stages]]
name = "fix-terms"
service = "terms"
input = "paragraphs"
output = "replace"
```

### Contract

An adapter is any executable that accepts these arguments and serves the API its step uses.

| Argument | Meaning |
|---|---|
| `--host HOST` | Address to listen on; bind only to it (notepipe passes `127.0.0.1`). |
| `--port PORT` | Port to listen on. |

Other arguments are the adapter's own; users write them in `adapter`.

`GET /health` returns `200` once the adapter can answer; before that, return `503` or refuse connections. If the
process exits, notepipe reports its last output lines.

On failure respond with a non-2xx status and `{"error": {"message": "…"}}`. When the service has a key, requests carry
`Authorization: Bearer <key>`; an adapter may check it or forward it.

### Transcription API

Speech recognition sends each speech segment to `POST /v1/audio/transcriptions` as `multipart/form-data`, compatible
with OpenAI's endpoint (or, with `[speech].api = "chat"`, to the chat API as audio input):

| Field | Required | Content |
|---|---|---|
| `model` | no | `[speech].model`, when set |
| `language` | no | `[transcript].language`, when set, as written in the configuration (for example `ko`) |
| `file` | yes | one speech segment: WAV, 16 kHz, mono, 16-bit PCM, about `[speech.detection].max_segment` seconds or less (20 by default) plus up to half a second of padding; always the last part |

Respond `200` with `{"text": "…"}`, using an empty string when there is no speech. Return clean text: model-specific
markers or formatting are the adapter's job to remove.

### Chat API

Stages send `POST /v1/chat/completions` with a JSON body in the OpenAI Chat Completions format:

| Field | Present | Content |
|---|---|---|
| `model` | when the stage has a model | the model name |
| `messages` | always | a system message with the stage's prompt and notepipe's output instructions, then a user message with the input (below) |
| `response_format` | paragraph stages | `{"type": "json_object"}` |
| other fields | when configured | everything in the stage's `request` |

The input, and the answer notepipe expects in `content`:

- **Paragraph stages:** the user content is an array of parts. The first is a `text` part holding a JSON array of up to
  20 paragraphs, `[{"id": 0, "text": "…"}, …]`; with `images = true` an `image_url` part with the screenshot follows, as
  a JPEG data URL. Answer with `{"segments": [{"id": 0, "text": "…"}, …]}`, one entry for every input `id`.
- **Transcript stages:** the user content is a string: the title and author when they are known, then the transcript,
  one line per fragment starting with its start time, such as `[03:12]` or `[1:03:12]`. Answer with Markdown. A long
  transcript arrives in several requests, followed by one that asks to combine their results.

`stream` is never requested. Respond `200` with:

```json
{"choices": [{"message": {"content": "…"}}], "usage": {"prompt_tokens": 120, "completion_tokens": 40}}
```

`usage` is optional; when present, notepipe reports the counts. `429` and `5xx` responses are retried with backoff.

An adapter may ignore `response_format`, because the system message asks for JSON as well. If it rejects the field
instead, it should answer `400` with a message that mentions `response_format`; notepipe then retries without it.

### Examples

A transcription adapter in Python, using only the standard library:

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

A chat adapter in Python, using only the standard library:

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

Name the executable `notepipe-asr-<name>` when it serves the transcription API, `notepipe-ai-<name>` when it serves the
chat API, or `notepipe-source-<name>` for a download adapter, and publish it as a package that installs that executable
(a Python console script or a binary wheel works well), so that `uv tool install notepipe --with <package>` places it
next to notepipe.
