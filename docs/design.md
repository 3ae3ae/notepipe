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

**Stay neutral.** The engine prefers no vendor, website, language or region: it names none in its code and contacts only
what the configuration names. The configuration it creates uses OpenAI and yt-dlp so that a fresh install works, and it
is a file you change like any other. Prompts are English defaults that you replace; notes follow the language of the
recording, not of the tool. The CLI speaks English, the common language of developer tools.

**Produce plain, durable output.** A note is a folder of Markdown, images and JSON that any editor, script or note app can
read. notepipe never leaves you with a half-written note and never discards a good one for a failed run.

**Keep it small and explicit.** Every option has a current use. Problems are reported as warnings or errors, not hidden.
Each change is the smallest one that fully solves its problem.

## How a note is made

```
local file ───────────────────────┐
URL ──► download stage ───────────┴──► media file
                                           │
                 ┌─────────────────────────┴──────────────┐
                 ▼                                        ▼
         screenshots stage                        transcript stage
       (frames compared; kept when        (its sources in order: subtitles,
        the picture changes)               then speech recognition)
                 │                                        │
                 └─────────────────────────┬──────────────┘
                                           ▼
                             sections and paragraphs
                                           │
                                           ▼
                  rewrite, add-below, add-top and filter stages,
                               in pipeline order
                                           │
                                           ▼
                               note stage ──► hook stages
```

1. **Download.** A local file is used as is. A URL is handed to the download stage's program, which returns a media
   file, metadata and any subtitles.
2. **Screenshots.** Frames are sampled with ffmpeg and compared; a screenshot is kept when the picture changes and then
   holds still. Audio-only input, or a pipeline without this stage, has none.
3. **Transcript.** The transcript stage tries its sources in order. Subtitles come from files next to the media or from
   the download; speech is located with earshot, a small neural voice detector compiled into notepipe, split into short
   segments and sent to the stage's provider.
4. **Sections and paragraphs.** Transcript fragments are grouped under the screenshot that was on screen and joined into
   paragraphs, with word spacing that follows each writing system.
5. **Processing.** `rewrite`, `add-below` and `add-top` stages send the paragraphs or the whole transcript, with their
   prompt, to their provider and put the answer into the note; `filter` stages pass the note through a program.
6. **Note.** `note.md`, `images/` and `note.json` are written to a staging folder and moved into place, then the hook
   stages run.

## Interfaces

notepipe meets the outside world in a few places, and each has one shape:

- **The pipeline.** The configuration lists the stages that run, in order, and each stage's settings live in its own
  table. Every stage has the same shape: a name, a type and that type's keys. The types are fixed in the core, and their
  order is checked before any work starts.
- **Providers.** Every connection, whether a cloud API, a server on your network or a program that notepipe starts, is a
  provider, defined once with its address and the variable that holds its key. Stages name the provider they use and
  hold only what they ask of it, such as the model and the prompt. A provider does not know what it is used for.
- **Programs.** notepipe runs four kinds of program, each named in one place with one small contract: provider programs
  serve an OpenAI-compatible API, download programs fetch a URL, filter programs change the note as JSON, and hook
  programs run after the note is written.
- **Configuration and options.** The configuration is the complete statement of a run, and the file notepipe creates
  lists every value in effect. Command-line options change what a run does with the note, not where it connects.
- **Output.** `note.md`, `note.json` and the `--json` events are the contract with programs that read notes.

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
| A different summary, a translation, a glossary, study questions | a stage prompt in the configuration |
| Processing that needs no language model, such as fixed replacements | a filter stage |
| Another output format, syncing to a note app | a hook stage |
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
