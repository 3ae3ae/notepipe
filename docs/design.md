# Design

This document explains what notepipe is for, the principles behind it, and how the pieces fit together. It is the
reference for deciding where a change belongs.

## Principles

Small, transparent, composable, and under your control.

**Do one thing.** notepipe turns a recording into a note. Everything in the core serves that path from a local media
file to a note folder.

**Keep it small and explicit.** Every option has a current use, and every operation has a name of its own. Each change
is the smallest one that fully solves its problem, judged by the system it leaves behind: a special case for one
symptom solves nothing.

**Hide nothing.** Every stage leaves a file you can open. When a run stops, it says where and why. Problems are
reported as warnings or errors.

**Keep the core light.** The core holds only what every run needs, and no speech recognition or language models or
platform SDKs. Engines, models and websites come as adapters that you install, swap or remove on their own.

**Speak standard interfaces.** Stages pass files to each other. The core uses ffmpeg for media, OpenAI-compatible HTTP
APIs for speech recognition and language models, and a few small, documented [program contracts](adapters.md) for
everything else. Any piece that fits these interfaces can replace another without touching the rest; none is built in.

**Leave the decisions to you.** notepipe never decides what a note says. It gives a new note its first name; after
that, what the note is called and where it lives are yours.

**Stay neutral.** The engine prefers no vendor, website, language, region or notation: nothing in its code depends on
one, and every script and notation passes through it unchanged. It contacts only what the configuration names.

**Produce plain, durable output.** A note is a folder of Markdown, images and JSON that any editor, script or note app
can read. notepipe never leaves you with a half-written note and never discards a good one for a failed run.

## How a note is made

A run is a pipeline of stages. Each stage reads named files and writes one, in the source's work folder, so every step
can be opened and checked; apart from the source's title, author, duration and link, nothing else passes between
stages. With the default configuration:

```
source ─► download ─► media ─┬─► transcript ─► transcript.json ─┐
                             └─► screenshots ─► images/ ────────┴─► draft ─► draft.json
                                                                                 │
                         note.md ◄─ markdown ◄─ summary.md ◄─ summary ◄─ proofread.json ◄─ proofread
```

1. **Download.** This stage runs first. A local file is used where it is. A URL is handed to the download stage's
   program, which returns a media file, metadata and any subtitles.
2. **Transcript.** The transcript stage tries its sources in order: subtitles found next to the file or delivered with
   the download, then speech recognition. Speech is located with earshot, a small neural voice detector compiled into
   notepipe, split into short segments and sent to the stage's provider.
3. **Screenshots.** Frames are sampled with ffmpeg and compared; a screenshot is kept when the picture changes and then
   holds still. Audio-only input has none. This runs at the same time as the transcript.
4. **Document.** Each transcript line goes, whole, under the screenshot on screen at its midpoint, and the lines are
   joined into paragraphs, with word spacing that follows each writing system.
5. **Processing.** `rewrite` stages send a document's paragraphs, with their prompt, to their provider and replace
   them with the answers; `write` stages send a whole transcript, document or Markdown file and keep the Markdown
   answer. `run` stages pass files through
   any program.
6. **Markdown.** The `markdown` stage joins documents and Markdown files into the note's text, in the order given.
7. **Publishing.** When every stage has finished, the files the stages publish, and the screenshots they show, are
   written to a new note folder under a hidden name and renamed into place, unless they are the same as the last
   ones published; one can also go to standard output.

## Interfaces

notepipe meets the outside world in a few places, and each has one shape:

- **The pipeline.** The configuration lists the stages that run, in order. Each stage's own table says what it does
  (`type`), what it reads (`input`), what it writes (`output`) and what it publishes (`publish`). The core checks
  that every input is written by an earlier stage, of a kind the stage reads, before any work starts.
- **Files.** The kinds that pass between stages are a few documented formats: media, transcript, images, document and
  Markdown. Apart from the source's details, which come from the download stage, stages share nothing else.
- **Providers.** Every connection, whether a cloud API, a server on your network or a program that notepipe starts, is a
  provider, defined once with its address and the variable that holds its key. Stages name the provider they use and
  hold only what they ask of it, such as the model and the prompt. A provider does not know what it is used for.
- **Programs.** notepipe runs three kinds of program, each named in one place with one small contract: provider
  programs serve an OpenAI-compatible API, download programs fetch a URL, and run programs read a stage's files and
  write its output.
- **Configuration and options.** The configuration is the complete statement of a run, and the file notepipe creates
  lists every value in effect. Command-line options change what a run does with the note, not where it connects.
- **Output.** The note folder, standard output and the `--json` events are the contract with programs that read
  notes.

## Where things belong

The core boundary is enforced in review (see [AGENTS.md](../AGENTS.md)). Things that are often requested, and where they
live instead:

| Need | Place |
|---|---|
| Downloading from a website, sign-in, playlists | a download program |
| A local speech model (llama.cpp, OpenVINO, Core ML …) | a provider program |
| A vendor API that is not OpenAI-compatible | a provider program that translates the request |
| AI through a different API, or through a subscription | a provider program |
| Cleaning up one model's output quirks | that model's provider program |
| A different summary, a translation, a glossary, study questions | a `rewrite` or `write` stage's prompt in the configuration |
| A different note layout | a `write` or `run` stage that writes the Markdown |
| Processing that needs no language model, such as fixed replacements | a `run` stage |
| Another output format, such as Word or HTML | a `run` stage, for example with pandoc |
| Syncing to a note app | a program run after notepipe, on the note folder |
| A graphical interface | a separate program that drives the CLI |

## Guarantees

- **Notes are never modified.** A note folder is built completely before it appears, and is never changed afterwards. A
  rerun with unchanged results leaves it as it is, with your edits; a changed result goes into a new folder next to it.
  A run that fails or is cancelled writes nothing.
- **Finished work is reused.** Downloads, screenshots, speech transcripts and successful AI responses are cached per
  source and keyed by the inputs that affect them, so changing a `rewrite` or `write` prompt does not repeat speech
  recognition, and a rerun with nothing changed repeats no paid or slow work and leaves the note as it is. Failed
  responses are never cached. `run` stages, whose dependencies notepipe cannot see, run every time.
- **Cancellation is clean.** Ctrl-C (and SIGTERM on Unix) stops the run and every program it started. A descendant
  that starts a session of its own is not stopped.
- **Data goes only where the configuration sends it.** notepipe contacts the providers and runs the programs that the
  configuration names, and nothing else; it has no telemetry. The configuration it creates names OpenAI.

## Stability

notepipe is at version 0.x. The [CLI contract](cli.md), the configuration format and the `note.json` schema may still
change; every such change is listed in the [changelog](../CHANGELOG.md). `note.json` carries a `schema` number that
changes when its structure does.
