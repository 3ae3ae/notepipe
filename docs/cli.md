# Using notepipe from other programs

The command-line interface is notepipe's integration contract. A program runs the `notepipe` binary, passes the
configuration it wants, reads progress as JSON lines and then reads the note folder.

```sh
notepipe "$SOURCE" -o "$OUT" --config - --json < config.toml
```

## Input

- `SOURCE` is a local video or audio file, or an `http(s)` URL (fetched by the program in the `download` stage's
  `command`).
- `--config FILE` reads the configuration from `FILE` instead of the user's config file; `--config -` reads it from
  standard input. The content is TOML, or JSON when its first non-space character is `{`. It has the same structure as
  `config.toml` (see `notepipe config init`). Command-line options still override it.
- `--subtitle FILE` supplies an SRT or VTT transcript: it becomes the transcript stage's only source. An unreadable or
  empty file is an error.
- `--skip a,b` leaves those stages out of the pipeline for this run.
- notepipe never prompts.

API keys come from the environment variables that providers name in `api_key_env`; a program that starts notepipe
passes them in the child's environment. The configuration is checked, and the key of every provider that a running
stage may use is looked up, before any work starts.

## Events

With `--json`, standard output carries one JSON object per line. Every object has a `type`:

| `type` | Fields | Meaning |
|---|---|---|
| `stage` | `stage`, `status` (`start` or `done`) | a pipeline stage began or ended |
| `progress` | `stage`, `current`, `total` | progress within a stage |
| `tokens` | `stage`, `prompt`, `completion` | token usage so far in a stage |
| `log` | `level` (`error`, `warn`, `info`, `debug`), `message` | diagnostics |
| `done` | `note_dir`, `files`, `work_dir`, `warnings` | the run finished and its outputs were published |
| `error` | `message` | the run failed |

`stage` is the name of a stage in the pipeline, or `NAME/STEP` for a step inside it: `TRANSCRIPT/subtitles`,
`TRANSCRIPT/audio` and `TRANSCRIPT/speech` in the transcript stage, `SCREENSHOTS/scan` and `SCREENSHOTS/extract` in the
screenshots stage, and `NAME/provider` while a stage starts a provider program. A skipped stage sends no events.
Treat unknown names and extra fields as informational; new ones may be
added.

In the `done` event, `note_dir` is the note folder, or `null` when nothing was published to one; `files` maps each
publishing stage to the path it published, or to `"-"`; `work_dir` is the source's work folder, where every stage's
output is; `warnings` is the final list of warnings.

A stage that publishes to `-` prints its output on standard output after the note folder is written, so a failure to
print leaves the new note in place. It cannot be combined with `--json`.

## Exit status

| Code | Meaning |
|---|---|
| 0 | the note was written (it may still carry warnings) |
| 1 | the run failed, including invalid settings or option values; the previous note for this source, if any, is unchanged (see below for the rare exception) |
| 2 | the command line could not be parsed, such as an unknown option |
| 130 | cancelled by Ctrl-C, or by SIGTERM on Unix |

On the first signal notepipe stops its stages and the programs it started, abandons requests in flight (their answers
are never cached), removes downloaded media and extracted audio, and exits. A second signal exits immediately after
killing those programs, and the next run does the cleanup.

## Output

The note folder, `<out>/<title>/`, holds what the stages publish and the screenshots that a published document or
its Markdown shows. With the default configuration:

```
<out>/<title>/note.md
<out>/<title>/note.json
<out>/<title>/images/0001.jpg …
<out>/<title>/.notepipe.json      the source this folder belongs to
```

The folder name comes from the title. A run on the same source replaces its folder, which it recognises by
`.notepipe.json`; a different source with the same title gets a `-2`, `-3`, … suffix, and a folder without
`.notepipe.json` is never replaced. After a source's title changes, its note goes to a folder for the new title. The
folder is built in a staging folder and moved into place whole, so a reader never sees a half-written note and a failed
run leaves the previous note as it was. If moving the new note into place fails and the previous note cannot be moved
back either, the error message gives the path where the previous note was kept; the next run restores it.

A published document, such as the default `note.json`, has this format (`schema` 3), which is also what `run`
stages read and write with `produces = "document"`:

```json
{
  "schema": 3,
  "meta": {"title": "…", "author": "…", "duration": 123.4, "source": "…", "seek_url": "…"},
  "sections": [
    {"t": 0.0, "end": 42.0, "image": "images/0001.jpg",
     "speech": [{"start": 0.5, "end": 7.0, "text": "…", "raw": "…"}]}
  ]
}
```

`source` is the URL as given, or the absolute path of the local file. `image` is empty for sections without a
screenshot (for example audio-only input). `raw` is present when a stage replaced the text, and `seek_url` when the
download program provided a link template. Fields a `run` stage adds are kept.

## Work folders and caches

Each source has a work folder under `work/` in notepipe's cache folder: `$XDG_CACHE_HOME/notepipe` when that variable
is set, otherwise `~/Library/Caches/notepipe` on macOS, `%LOCALAPPDATA%\notepipe\cache` on Windows and
`~/.cache/notepipe` elsewhere. It holds every
stage's output under its `output` name, and in `.cache/` what lets a later run reuse work: the download, speech
segments, screenshots and model answers. `-v` prints the folder, and the `done` event gives it as `work_dir`.

Downloaded media and extracted audio are deleted once the run ends (after a forced exit, by the next run); a later run that needs them again, because
screenshot or speech settings changed, downloads the media again, and one that changes only later stages does not.
Work folders can be deleted whenever notepipe is not running; `--no-resume` clears one source's folder.
