# Design

This document explains what notepipe is for, the principles behind it, and how the pieces fit together. It is the
reference for deciding where a change belongs.

## Principles

**Do one thing.** notepipe turns a recording into a readable note. Everything in the core serves that path from a local
media file to a note folder.

**Speak standard interfaces.** The core uses ffmpeg for media, OpenAI-compatible HTTP APIs for speech recognition and
language models, and a few small, documented [program contracts](adapters.md) for everything else. Any service, engine
or website that fits those interfaces works; none is built in.

**Stay light.** The default installation contains no speech recognition or language models and no platform SDKs. Speech
recognition and stages run wherever you point them: a cloud API, a server on your network, or a local engine started
as a program.

**Stay neutral.** The engine prefers no vendor, website, language or region: nothing in its code depends on one, and it
contacts only what the configuration names. The configuration it creates uses OpenAI so that a fresh install works with
an API key, and it is a file you change like any other. Prompts are English defaults that you replace; notes follow the language of the
recording, not of the tool. The CLI speaks English, the common language of developer tools.

**Produce plain, durable output.** A note is a folder of Markdown, images and JSON that any editor, script or note app can
read. notepipe never leaves you with a half-written note and never discards a good one for a failed run.

**Keep it small and explicit.** Every option has a current use. Problems are reported as warnings or errors, not hidden.
Each change is the smallest one that fully solves its problem.

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
4. **Document.** Transcript lines are grouped under the screenshot that was on screen and joined into paragraphs, with
   word spacing that follows each writing system.
5. **Processing.** `llm` stages send a file, with their prompt, to their provider: a document's paragraphs, rewritten
   in place, or a whole transcript, document or Markdown file, answered with Markdown. `run` stages pass files through
   any program.
6. **Markdown.** The `markdown` stage joins documents and Markdown files into the note's text, in the order given.
7. **Publishing.** When every stage has finished, the files the stages publish, and the screenshots they show, are
   written to a staging folder and moved into place as the note folder; one can also go to standard output.

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
| A different summary, a translation, a glossary, study questions | an `llm` stage's prompt in the configuration |
| A different note layout | an `llm` or `run` stage that writes the Markdown |
| Processing that needs no language model, such as fixed replacements | a `run` stage |
| Another output format, such as Word or HTML | a `run` stage, for example with pandoc |
| Syncing to a note app | a program run after notepipe, on the note folder |
| A graphical interface | a separate program that drives the CLI |

## Guarantees

- **Notes are replaced whole.** A new note is built completely before it replaces the previous one. A run that fails or
  is cancelled before its note is written leaves the previous note as it was. If a run is interrupted while swapping
  folders, the next run finishes or undoes the swap from a transaction record before it writes a note.
- **Finished work is reused.** Downloads, screenshots, transcripts and successful AI responses are cached per source and
  keyed by the inputs that affect them, so changing an AI prompt does not repeat speech recognition. Failed responses are
  never cached.
- **Cancellation is clean.** Ctrl-C (and SIGTERM on Unix) stops the run and every program it started.
- **Data goes only where the configuration sends it.** notepipe contacts the providers and runs the programs that the
  configuration names, and nothing else; it has no telemetry. The configuration it creates names OpenAI.

## Stability

notepipe is at version 0.x. The [CLI contract](cli.md), the configuration format and the `note.json` schema may still
change; every such change is listed in the [changelog](../CHANGELOG.md). `note.json` carries a `schema` number that
changes when its structure does.
