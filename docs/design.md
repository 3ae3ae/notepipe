# Design

This document explains what notepipe is for, the principles behind it, and how the pieces fit together. It is the
reference for deciding where a change belongs.

## Principles

**Do one thing.** notepipe turns a recording into a readable note. Everything in the core serves that path from a local
media file to a note folder.

**Speak standard interfaces.** The core uses ffmpeg for media, OpenAI-compatible HTTP APIs for speech recognition and
language models, and two small, documented [adapter contracts](adapters.md) for everything else. Any service, engine or
website that fits those interfaces works; none is built in.

**Stay light.** The default installation contains no speech recognition or language models and no platform SDKs. Speech
recognition and stages run wherever you point them: a cloud API, a server on your network, or a local engine started
as an adapter.

**Stay neutral.** No vendor, website, language or region is preferred. Prompts are English defaults that you replace;
notes follow the language of the recording, not of the tool. The CLI speaks English, the common language of developer
tools.

**Produce plain, durable output.** A note is a folder of Markdown, images and JSON that any editor, script or note app can
read. notepipe never leaves you with a half-written note and never discards a good one for a failed run.

**Keep it small and explicit.** Every option has a current use. Problems are reported as warnings or errors, not hidden.
Each change is the smallest one that fully solves its problem.

## How a note is made

```
local file ─────────────────┐
URL ──► download adapter ───┴──► media file
                                      │
            ┌─────────────────────────┴─────────────┐
            ▼                                       ▼
     frame sampling                               audio                 subtitles
            │                                       │                (sidecar file,
            ▼                                       ▼               download adapter
       screenshots                          speech detection         or --subtitle)
            │                                       │                       │
            │                                       ▼                       │
            │                              speech recognition               │
            │                                       │                       │
            │                                       ▼                       │
            │                                  transcript ◄─────────────────┘
            │                                       │
            └─────────────────────────┬─────────────┘
                                      ▼
                      timeline: sections and paragraphs
                                      │
                                      ▼
                          stages, in configured order
                                      │
                                      ▼
                                 note folder ──► completion hook
```

1. **Source.** A local file is used as is. A URL is handed to a download adapter, which returns a media file, metadata and
   any subtitles.
2. **Screenshots.** Frames are sampled with ffmpeg and compared; a screenshot is kept when the picture changes and then
   holds still. Audio-only input skips this step.
3. **Transcript.** Subtitles are used when available. Otherwise speech is located with earshot, a small neural voice
   detector compiled into notepipe, split into short segments and sent to the service that `[speech]` names.
4. **Timeline.** Transcript fragments are grouped under the screenshot that was on screen and joined into paragraphs,
   with word spacing that follows each writing system.
5. **Stages.** Each configured stage sends the paragraphs or the whole transcript, with its prompt, to its service (a
   language model or an adapter) and replaces paragraphs, adds text below them or adds Markdown at the top.
6. **Note.** `note.md`, `images/` and `note.json` are written to a staging folder and moved into place, then the
   completion hook runs.

## Interfaces

notepipe meets the outside world in a few places, and each has one shape:

- **Services.** Every connection, whether a cloud API, a server on your network or an adapter that notepipe starts, is
  a service, defined once under `[services]` with its address and credentials. A service does not know what it is used
  for.
- **Steps.** Speech recognition and each stage name the service they use and hold only what they ask of it: the API, the
  model, the prompt. Nothing is inherited from another section, so a step's own block says where its requests go.
- **Adapters.** Every program started as a service follows one lifecycle: `--host` and `--port`, `GET /health`, and
  stopping with its step. Whether it serves the transcription API, the chat API or both depends only on the steps that
  use it. A download adapter has its own small contract, because it runs once per URL instead of serving requests.
- **Configuration and options.** The configuration is the complete statement of a run, and the file notepipe creates
  lists every value in effect. Command-line options change what a run does with the note, not where it connects.
- **Output.** `note.md`, `note.json` and the `--json` events are the contract with programs that read notes.

## Where things belong

The core boundary is enforced in review (see [AGENTS.md](../AGENTS.md)). Things that are often requested, and where they
live instead:

| Need | Place |
|---|---|
| Downloading from a website, sign-in, playlists | a download adapter |
| A local speech model (llama.cpp, OpenVINO, Core ML …) | a service adapter |
| A vendor API that is not OpenAI-compatible | a service adapter that translates the request |
| AI through a different API, or through a subscription | a service adapter |
| A processing step that is not a language model | a stage whose service is an adapter |
| Cleaning up one model's output quirks | that model's adapter |
| A different summary, a translation, a glossary, study questions | a stage prompt in the configuration |
| Another output format, syncing to a note app | the completion hook |
| A graphical interface | a separate program that drives the CLI |

## Guarantees

- **Notes are replaced whole.** A new note is built completely before it replaces the previous one. A run that fails or
  is cancelled before its note is written leaves the previous note as it was. If a run is interrupted while swapping
  folders, the next run finishes or undoes the swap from a transaction record before it writes a note.
- **Finished work is reused.** Downloads, screenshots, transcripts and successful AI responses are cached per source and
  keyed by the inputs that affect them, so changing an AI prompt does not repeat speech recognition. Failed responses are
  never cached.
- **Cancellation is clean.** Ctrl-C (and SIGTERM on Unix) stops the run and every adapter process it started.
- **Data goes only where you send it.** notepipe contacts the endpoints and adapters you configure and nothing else; it has
  no telemetry.

## Stability

notepipe is at version 0.x. The [CLI contract](cli.md), the configuration format and the `note.json` schema may still
change; every such change is listed in the [changelog](../CHANGELOG.md). `note.json` carries a `schema` number that
changes when its structure does.
