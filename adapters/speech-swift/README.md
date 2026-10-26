# notepipe-asr-speech-swift

A [notepipe](https://github.com/3ae3ae/notepipe) speech adapter that runs speech recognition locally on Apple silicon
with [speech-swift](https://github.com/soniqo/speech-swift): Qwen3-ASR 1.7B on MLX by default, or Qwen3-ASR 0.6B and
Whisper on Core ML.

This adapter is built in CI but has not yet been verified on hardware.

## Install

macOS 15 or later on Apple silicon:

```sh
uv tool install "notepipe[speech-swift]"
```

The package installs `mlx.metallib` next to the executable, where MLX looks for it. Building from source needs Xcode with
its Metal toolchain (`xcodebuild -downloadComponent MetalToolchain`); a plain `cargo install` does not copy
`mlx.metallib`.

## Use

```toml
[providers.speech-swift]
command = ["notepipe-asr-speech-swift"]
concurrency = 1

[stages.transcript.speech]
provider = "speech-swift"
```

`--model` selects `qwen3-1.7b` (default), `qwen3-0.6b` or `whisper`. The first run downloads the model.
The transcript stage's `language` is passed to the model.

The Qwen models write at most 448 tokens per segment: roughly a minute of English speech, and less in scripts that take
more tokens per word. Keep the speech `max_segment` at its default of 20 seconds so that segments are not cut short.

Whisper on Core ML writes at most about 220 tokens per segment, and ends a segment early when one word comes three times
in a row, as in "no, no, no". Either way the rest of the segment is lost without an error. Prefer a Qwen model; with
Whisper, a shorter `max_segment` helps only with the token limit.
