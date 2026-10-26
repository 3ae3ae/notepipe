# Adapters

The notepipe core turns a local media file into a note and talks to the outside world only through standard interfaces.
Anything tied to a particular engine, vendor or website is an **adapter**: a separate program that notepipe runs. To
attach one, install it and name it in `config.toml`; to detach one, uninstall it or remove it from the configuration.

What a program must do depends on where it is named:

| Named in | Kind | It |
|---|---|---|
| `[providers.NAME]` `command` | [provider program](#provider-programs) | serves an OpenAI-compatible API on localhost while stages use it |
| a `download` stage | [download program](#download-programs) | turns a URL into a local media file plus metadata and subtitles |
| a `run` stage | [run program](#run-programs) | reads the stage's input files and writes its output: your own script or any command-line tool |

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
command = ["notepipe-asr-llama", "-hf", "ggml-org/Qwen3-ASR-1.7B-GGUF:Q8_0"]
concurrency = 1

[stages.transcript.speech]
provider = "llama"
```

When a stage uses a provider that has a `command`, notepipe:

1. starts `command` with `--host 127.0.0.1 --port <free port>` added at the end;
2. waits until `GET /health` answers `200` (up to 30 minutes, because a first run may download a model);
3. sends the stage's requests to `http://127.0.0.1:<port>/v1`;
4. stops the program and its child processes after the last stage that uses it, or when the run fails or is cancelled.

All stages that use the same provider share that one process and its `concurrency`. Responses are cached by
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

Arguments before them are the program's own; users write them in `command`.

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
markers or formatting are the program's job to remove. A transcript cut off at the model's output limit is not a result:
answer with a `4xx` status other than `429`, which notepipe does not retry.

### Chat API

`llm` stages send `POST <base_url>/chat/completions` with a JSON body in the OpenAI Chat Completions format:

| Field | Present | Content |
|---|---|---|
| `model` | when the stage has a model | the model name |
| `messages` | always | a system message with the stage's prompt and notepipe's format instructions, then a user message with the input (below) |
| `response_format` | an `output` ending in `.json` | `{"type": "json_object"}` |
| other fields | when configured | everything in the stage's `request` |

The input, and the answer notepipe expects in `content`:

- **`output` ending in `.json`:** the user content is an array of parts. The first is a `text` part holding a JSON array
  of up to `batch_size` paragraphs, `[{"id": 0, "text": "…"}, …]`; with `images = true` an `image_url` part with the
  screenshot follows, as a JPEG data URL. Answer with `{"segments": [{"id": 0, "text": "…"}, …]}`, one entry for every
  input `id`.
- **`output` ending in `.md`:** the user content is a string. For a transcript or a document: the title and author
  when they are known, then one line per fragment or paragraph starting with its start time, such as `[03:12]` or
  `[1:03:12]`. For Markdown: the Markdown as it is. Answer with Markdown. An input longer than `chunk_size` arrives in
  several requests, followed by one that asks to combine their results.

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
input = "source"
output = "media"
command = ["notepipe-source-ytdlp", "--", "--cookies-from-browser", "firefox"]
```

The program runs only for `http(s)` sources; a download stage without `command` takes local files only. For a local file, notepipe writes the stage's `info.json` itself, with
the file's absolute path and the subtitles found next to it, and never deletes the file. Results are cached per URL
and command. Downloaded media is deleted once the run ends (after a forced exit, by the next run), so a later run that only changes later stages reuses the
transcript and screenshots, while one that needs the media again downloads it again.

### Contract

notepipe runs:

```
<command…> <URL> --out <DIR>
```

`DIR` is an empty directory that notepipe owns, inside the work folder. The program downloads the media, and any
subtitles, into it.

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
open the item at a given time. Both URLs go into the document's `meta`; notepipe's own layout adds no links, but your
prompts and programs can.

notepipe saves the result line as `info.json` in the stage's output, next to the media and subtitles, where later
stages read it. Write diagnostics to standard error.
Exit with status 0 on success and anything else on failure; notepipe shows the last lines of standard error.

## Run programs

A `run` stage makes any command-line program a stage. Its `input` files go in and its `output` comes out:

```toml
[stages.terms]                       # a script that fixes glossary terms in the document
type = "run"
input = "draft.json"
output = "terms.json"
produces = "document"
command = ["python3", "fix-terms.py", "--glossary", "glossary.tsv"]

[stages.docx]                        # pandoc makes a Word copy of the note
type = "run"
input = "note.md"
output = "note.docx"
publish = "note.docx"
command = ["pandoc", "{input}", "-o", "{output}"]
```

### Contract

- **No shell.** `command` is the program and its arguments, run as written. Only two placeholders exist, and each must
  be a whole argument: `{input}` becomes the absolute paths of the inputs, in order, as separate arguments, and
  `{output}` the absolute path the program must write. Nothing else is ever substituted, so a downloaded title can
  never reach the command line.
- **Standard input and output.** Without `{input}`, the stage's one input file arrives on standard input, and a stage
  with no input gets an empty one; a stage with several inputs, or with a folder as input, must use `{input}`. Without `{output}`, standard output becomes the output
  file. Write diagnostics to standard error.
- **Where it runs.** In the folder notepipe was started in, so relative paths in `command`, such as `glossary.tsv`, are
  yours. There is no time limit; Ctrl-C stops the program.
- **Result.** Exit with status 0 on success. A non-zero exit, a missing output, or an output that is not a valid
  `produces` kind stops the run, and notepipe shows the last lines of standard error. A document must keep its
  screenshots' paths inside the work folder.
- **Kinds.** With `produces = "transcript"`, `"document"` or `"markdown"`, other stages can read the output; the formats
  are in [docs/configuration.md](configuration.md#what-stages-read-and-write). Without it, the output can only be
  published.
- **Every run.** `run` stages are not cached: they run each time, because notepipe cannot see what the program depends
  on.

A program in Python, using only the standard library, that reads the document on standard input and prints it changed:

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
