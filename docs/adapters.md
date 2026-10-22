# Adapters

The notepipe core turns a local media file into a note and talks to the outside world only through standard interfaces.
Anything tied to a particular engine, vendor or website is an **adapter**: a separate program that notepipe runs. To
attach one, install it and name it in `config.toml`; to detach one, uninstall it or remove it from the configuration.

What a program must do depends on where it is named:

| Named in | Kind | It |
|---|---|---|
| `[providers.NAME]` `command` | [provider program](#provider-programs) | serves an OpenAI-compatible API on localhost while stages use it |
| a `download` stage | [download program](#download-programs) | turns a URL into a local media file plus metadata and subtitles |
| a `filter` stage | [filter program](#filter-programs) | reads the note as JSON and prints it changed |

Official adapters are extras of the `notepipe` package, for example `uv tool install "notepipe[llama]"`. Other adapters
install next to notepipe with `uv tool install notepipe --with <package>`. notepipe looks for programs in its own
directory first, then on `PATH`; a relative path is resolved from the directory notepipe was started in. Programs run
without a shell.

## Official adapters

| Extra | Package | Kind | Notes |
|---|---|---|---|
| `ytdlp` | [`notepipe-source-ytdlp`](../adapters/ytdlp) | source | Any site yt-dlp supports; honors your yt-dlp configuration files |
| `llama` | [`notepipe-asr-llama`](../adapters/llama) | speech | Runs `llama-server`; strips Qwen3-ASR output markers |
| `openvino` | [`notepipe-asr-openvino`](../adapters/openvino) | speech | Intel NPU by default; `--device CPU` or `GPU` also work |
| `speech-swift` | [`notepipe-asr-speech-swift`](../adapters/speech-swift) | speech | Apple silicon (macOS 15 or later) with MLX and Core ML through speech-swift; built in CI, not yet verified on hardware |
| `xai` | [`notepipe-asr-xai`](../adapters/xai) | speech | Forwards to xAI's speech-to-text API |
| `meta` | [`notepipe-asr-meta`](../adapters/meta) | speech | Forwards to Meta's speech-to-text API |


## Provider programs

### Using one

```toml
[providers.llama]
command = "notepipe-asr-llama"
args = ["-hf", "ggml-org/Qwen3-ASR-1.7B-GGUF:Q8_0"]
concurrency = 1

[stages.transcript.speech]
provider = "llama"
```

When a stage uses a provider that has a `command`, notepipe:

1. starts the program with `args` and then `--host 127.0.0.1 --port <free port>`;
2. waits until `GET /health` answers `200` (up to 30 minutes, because a first run may download a model);
3. sends the stage's requests to `http://127.0.0.1:<port>/v1`;
4. stops the program and its child processes after the last stage that uses it, or when the run fails or is cancelled.

All stages that use the same provider share that one process, including the transcript stage. Responses are cached by
the command and the request, so a later run reuses them even though the port changes. Local engines usually
handle one request at a time, so give their provider `concurrency = 1`. A server you run yourself needs no program:
give the provider its `base_url`.

A provider program is the place for anything that is not a plain OpenAI-compatible endpoint: a local engine, a
different API such as the OpenAI Responses API, a provider with its own authentication, or access through a
subscription you already have.

### Contract

A provider program accepts these arguments and serves the API that its stages call.

| Argument | Meaning |
|---|---|
| `--host HOST` | Address to listen on; bind only to it (notepipe passes `127.0.0.1`). |
| `--port PORT` | Port to listen on. |

Arguments before them are the program's own; users write them in `args`.

`GET /health` returns `200` once the program can answer; before that, return `503` or refuse connections. If the
process exits, notepipe reports its last output lines.

On failure respond with a non-2xx status and `{"error": {"message": "…"}}`. When the provider has a key, requests carry
`Authorization: Bearer <key>`; a program may check it or forward it.

## Requests that stages send

These requests go to every provider, whether it is a remote API or a program.

### Transcription API

The transcript stage sends each speech segment to `POST <base_url>/audio/transcriptions` as `multipart/form-data`,
compatible with OpenAI's endpoint (or, with `api = "chat"`, to the chat API as audio input):

| Field | Required | Content |
|---|---|---|
| `model` | no | the speech `model`, when set |
| `language` | no | the transcript stage's `language` as written in the configuration (for example `ko`); not sent for `auto` |
| `file` | yes | one speech segment: WAV, 16 kHz, mono, 16-bit PCM, about `max_segment` seconds or less (20 by default) plus up to half a second of padding; always the last part |

Respond `200` with `{"text": "…"}`, using an empty string when there is no speech. Return clean text: model-specific
markers or formatting are the program's job to remove.

### Chat API

`llm` stages send `POST <base_url>/chat/completions` with a JSON body in the OpenAI Chat Completions format:

| Field | Present | Content |
|---|---|---|
| `model` | when the stage has a model | the model name |
| `messages` | always | a system message with the stage's prompt and notepipe's format instructions, then a user message with the input (below) |
| `response_format` | `reads = "paragraphs"` | `{"type": "json_object"}` |
| other fields | when configured | everything in the stage's `request` |

The input, and the answer notepipe expects in `content`:

- **`reads = "paragraphs"`:** the user content is an array of parts. The first is a `text` part holding a JSON array
  of up to `batch_size` paragraphs, `[{"id": 0, "text": "…"}, …]`; with `images = true` an `image_url` part with the
  screenshot follows, as a JPEG data URL. Answer with `{"segments": [{"id": 0, "text": "…"}, …]}`, one entry for every
  input `id`.
- **`reads = "transcript"`:** the user content is a string: the title and author when they are known, then the
  transcript, one line per fragment starting with its start time, such as `[03:12]` or `[1:03:12]`. Answer with
  Markdown. A transcript longer than `chunk_size` arrives in several requests, followed by one that asks to combine
  their results.

`stream` is never requested. Respond `200` with:

```json
{"choices": [{"message": {"content": "…"}}], "usage": {"prompt_tokens": 120, "completion_tokens": 40}}
```

`usage` is optional; when present, notepipe reports the counts. `429` and `5xx` responses are retried with backoff.

A program may ignore `response_format`, because the system message asks for JSON as well. If it rejects the field
instead, it should answer `400` with a message that mentions `response_format`; notepipe then retries without it.

### Examples

A transcription program in Python, using only the standard library:

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

A chat program in Python, using only the standard library:

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

## Download programs

### Using one

```toml
[stages.download]
type = "download"
command = "notepipe-source-ytdlp"
args = ["--", "--cookies-from-browser", "firefox"]
```

A `download` stage runs only for `http(s)` sources. Results are cached per URL and command. Media is deleted just before
the note is written, so a later run that only changes later stages reuses the cached screenshots and transcript, while
one that needs the media again downloads it again.

### Contract

notepipe runs:

```
<command> <args…> <URL> --out <DIR>
```

`DIR` is an empty directory owned by notepipe. The program downloads the media, and any subtitles, into it.

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


## Filter programs

A `filter` stage changes the note without a language model, for example to replace terms from a glossary:

```toml
[stages.fix-terms]
type = "filter"
command = "fix-terms"
args = ["--glossary", "glossary.tsv"]
```

### Contract

notepipe writes the note as it stands to the program's standard input, as JSON in the
[`note.json` format](cli.md#output), and reads the changed note from its standard output. The program runs in the
directory notepipe was started in, with a 10-minute limit.

notepipe takes from the result the `top` list and, for every fragment in `sections[].speech`, its `text`.
The result must have the same sections, with the same number of fragments each; everything else is ignored. When a
fragment's `text` changes, the old text is kept in `raw`.

Write diagnostics to standard error. Exit with status 0 on success. A failure, invalid JSON or a result with different
sections stops the run, and notepipe shows the last lines of standard error.

```python
#!/usr/bin/env python3
import json
import sys

note = json.load(sys.stdin)
for section in note["sections"]:
    for fragment in section["speech"]:
        fragment["text"] = fragment["text"].replace("Kubernetis", "Kubernetes")
json.dump(note, sys.stdout)
```

## Publishing an adapter

Name the executable `notepipe-asr-<name>` when it serves the transcription API, `notepipe-ai-<name>` when it serves the
chat API, or `notepipe-source-<name>` for a download program, and publish it as a package that installs that executable
(a Python console script or a binary wheel works well), so that `uv tool install notepipe --with <package>` places it
next to notepipe. The names are a convention; notepipe runs only the programs the configuration names.
