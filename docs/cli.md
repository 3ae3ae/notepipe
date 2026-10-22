# Using notepipe from other programs

The command-line interface is notepipe's integration contract. A program runs the `notepipe` binary, passes the
configuration it wants, reads progress as JSON lines and then reads the note folder.

```sh
notepipe "$SOURCE" -o "$OUT" --config - --json < config.toml
```

## Input

- `SOURCE` is a local video or audio file, or an `http(s)` URL (fetched by the pipeline's `download` stage).
- `--config FILE` reads the configuration from `FILE` instead of the user's config file; `--config -` reads it from
  standard input. The content is TOML, or JSON when its first non-space character is `{`. It has the same structure as
  `config.toml` (see `notepipe config init`). Command-line options still override it.
- `--subtitle FILE` supplies an SRT or VTT transcript; the transcript stage is skipped. An unreadable or empty file is an
  error.
- `--skip a,b` leaves those stages out of the pipeline for this run.
- notepipe never prompts.

API keys come from the environment variables that providers name in `api_key_env`; a program that starts notepipe
passes them in the child's environment. The configuration is checked, and every key is looked up, before any work
starts.

## Events

With `--json`, standard output carries one JSON object per line. Every object has a `type`:

| `type` | Fields | Meaning |
|---|---|---|
| `stage` | `stage`, `status` (`start` or `done`) | a pipeline stage began or ended |
| `progress` | `stage`, `current`, `total` | progress within a stage |
| `tokens` | `stage`, `prompt`, `completion` | token usage so far in a stage |
| `log` | `level` (`error`, `warn`, `info`, `debug`), `message` | diagnostics |
| `done` | `note_dir`, `markdown`, `json`, `warnings` | the note was written |
| `error` | `message` | the run failed |

`stage` is the name of a stage in the pipeline, or `NAME/STEP` for a step inside it: `TRANSCRIPT/subtitles`,
`TRANSCRIPT/audio` and `TRANSCRIPT/speech` in the transcript stage, `SCREENSHOTS/scan` and `SCREENSHOTS/extract` in the
screenshots stage, and `NAME/provider` while a stage starts a provider program. `media` is reading the source's media
information, before the pipeline's stages. Treat unknown names and extra fields as informational; new ones may be
added.

## Exit status

| Code | Meaning |
|---|---|
| 0 | the note was written (it may still carry warnings) |
| 1 | the run failed; the previous note for this source, if any, is unchanged (see below for the rare exception) |
| 2 | invalid command-line arguments |
| 130 | cancelled by Ctrl-C, or by SIGTERM on Unix |

On the first signal notepipe stops its stages and any adapters it started; a second signal exits immediately after
killing adapter processes.

## Output

```
<out>/<title>/note.md
<out>/<title>/images/0001.jpg …
<out>/<title>/note.json
<out>/<title>/stages/…        only for stages with keep = true (docs/configuration.md#keeping-stage-results)
```

The folder name comes from the title. A run on the same source replaces that folder; a different source with the same
title gets a `-2`, `-3`, … suffix. Folders are found by name, so after a source's title changes, its note goes to the
folder named after the new title. The note is built in a staging folder and moved into place, so a reader never sees a
half-written note. If moving the new note into place fails and the previous note cannot be moved back either, the error
message gives the path where the previous note was kept; the next run restores it.

`note.md` starts with YAML front matter:

```yaml
---
title: "…"
source: "…"            # the URL as given, or the absolute path of the local file
author: "…"            # when known
duration: "h:mm:ss"    # when known
created: "2026-10-21T12:00:00Z"
generator: "notepipe …"   # notepipe and its version
---
```

`note.json` (schema 2) holds the same content:

```json
{
  "schema": 2,
  "meta": {"title": "…", "author": "…", "duration": 123.4, "source": "…", "seek_url": "…"},
  "top": [{"stage": "summary", "text": "Markdown"}],
  "sections": [
    {"t": 0.0, "end": 42.0, "image": "images/0001.jpg",
     "speech": [{"start": 0.5, "end": 7.0, "text": "…", "raw": "…"}]}
  ],
  "warnings": ["…"]
}
```

`image` is empty for sections without a screenshot (for example audio-only input). `raw` is present when a stage replaced
the text, and `seek_url` when the download adapter provided a link template.

`note.json` lists the warnings known when the note was written. Warnings raised afterwards — cleaning up the previous
note — appear only in the `done` event, whose `warnings` list is final.

## Caches

Downloads, screenshots, transcripts and AI responses are cached per source in `$XDG_CACHE_HOME/notepipe` when that is
set, otherwise in `~/Library/Caches/notepipe` on macOS, `%LOCALAPPDATA%\notepipe\cache` on Windows and
`~/.cache/notepipe` elsewhere. Media and extracted audio are deleted once the stages have run, just before the note
is written. A later run that needs them again, because screenshot settings changed or, when speech is recognized, speech
settings changed, downloads the media again; one that changes only stages does not. The cache can be deleted whenever
notepipe is not running; `--no-resume` clears one source's entry.
