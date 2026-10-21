# Using notepipe from other programs

The command-line interface is notepipe's integration contract. A program runs the `notepipe` binary, passes the
configuration it wants, reads progress as JSON lines and then reads the note folder.

```sh
notepipe "$SOURCE" -o "$OUT" --config - --json < config.toml
```

## Input

- `SOURCE` is a local video or audio file, or an `http(s)` URL (resolved by a [source adapter](adapters.md#source-adapters)).
- `--config FILE` reads the configuration from `FILE` instead of the user's config file; `--config -` reads it from
  standard input. The content is TOML, or JSON when its first non-space character is `{`. It has the same structure as
  `config.toml` (see `notepipe config init`). Command-line options still override it.
- `--subtitle FILE` supplies an SRT or VTT transcript; subtitle discovery and speech recognition are skipped.
- `--stages a,b` / `--no-stages` choose the AI stages for this run.
- With `--json` notepipe never prompts. Missing settings are errors.

API keys can be part of the configuration. When the CLI reads its own config file, `NOTEPIPE_ASR_API_KEY` and
`NOTEPIPE_LLM_API_KEY` fill empty keys.

## Events

With `--json`, standard output carries one JSON object per line. Every object has a `type`:

| `type` | Fields | Meaning |
|---|---|---|
| `stage` | `stage`, `status` (`start` or `done`) | a pipeline stage began or ended |
| `progress` | `stage`, `current`, `total` | progress within a stage |
| `tokens` | `stage`, `prompt`, `completion` | token usage so far in an AI stage |
| `log` | `level` (`error`, `warn`, `info`, `debug`), `message` | diagnostics |
| `done` | `note_dir`, `markdown`, `json`, `warnings` | the note was written |
| `error` | `message` | the run failed |

Stage names include `adapter/start`, `download`, `subtitle`, `scenes/scan`, `scenes/extract`, `audio`, `transcribe`,
`summary` and `render`, plus the names of configured AI stages. Treat unknown stage names and extra fields as
informational; new ones may be added.

## Exit status

| Code | Meaning |
|---|---|
| 0 | the note was written (it may still carry warnings) |
| 1 | the run failed; the previous note for this source, if any, is unchanged |
| 2 | invalid command-line arguments |
| 130 | cancelled by SIGINT or SIGTERM |

On the first signal notepipe stops its stages and any adapters it started; a second signal exits immediately after
killing adapter processes.

## Output

```
<out>/<title>/note.md
<out>/<title>/images/0001.jpg …
<out>/<title>/note.json
```

The folder name comes from the title. A run on the same source replaces that folder; a different source with the same
title gets a `-2`, `-3`, … suffix. The note is built in a staging folder and moved into place, so a reader never sees a
half-written note.

`note.md` starts with YAML front matter:

```yaml
---
title: "…"
source: "…"            # the URL as given, or the absolute path of the local file
author: "…"            # when known
duration: "h:mm:ss"    # when known
created: "2026-10-21T12:00:00Z"
generator: "notepipe 0.1.0"
---
```

`note.json` (schema 1) holds the same content:

```json
{
  "schema": 1,
  "meta": {"title": "…", "author": "…", "duration": 123.4, "source": "…", "seek_url": "…"},
  "top": [{"stage": "summary", "text": "Markdown"}],
  "sections": [
    {"t": 0.0, "end": 42.0, "image": "images/0001.jpg",
     "speech": [{"start": 0.5, "end": 7.0, "text": "…", "raw": "…",
                 "below": [{"stage": "translate", "text": "…"}]}]}
  ],
  "warnings": ["…"]
}
```

`image` is empty for sections without a screenshot (for example audio-only input). `raw` is present when a stage replaced
the text, `below` when stages added text under it, and `seek_url` when the source adapter provided a link template.

## Completion hook

`[defaults].on_complete = ["program", "arg", …]` runs once after a note is written, without a shell, with the note folder
as its working directory and a 10-minute limit. It receives:

| Variable | Value |
|---|---|
| `NOTEPIPE_NOTE_DIR` | the note folder |
| `NOTEPIPE_MARKDOWN` | path of `note.md` |
| `NOTEPIPE_JSON` | path of `note.json` |
| `NOTEPIPE_WARNINGS` | number of warnings |

A failing hook produces a warning; the note and the exit status are unaffected.

## Caches

Downloads, screenshots, transcripts and AI responses are cached per source in `$XDG_CACHE_HOME/notepipe` when that is
set, otherwise in `~/Library/Caches/notepipe` on macOS, `%LOCALAPPDATA%\notepipe\cache` on Windows and
`~/.cache/notepipe` elsewhere. Media and extracted audio are deleted after a successful run. The cache can be deleted at any time; `--no-resume` clears one source's entry.
