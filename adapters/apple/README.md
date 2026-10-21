# notepipe-asr-apple

A [notepipe](https://github.com/3ae3ae/notepipe) speech adapter that runs speech recognition locally on Apple silicon
with Core ML and MLX.

This adapter is built in CI but has not yet been verified on hardware.

## Install

macOS 15 or later on Apple silicon:

```sh
uv tool install "notepipe[apple]"
```

The package installs `mlx.metallib` next to the executable, where MLX looks for it. Building from source needs Xcode with
its Metal toolchain (`xcodebuild -downloadComponent MetalToolchain`); a plain `cargo install` does not copy
`mlx.metallib`.

## Use

```toml
[asr]
command = ["notepipe-asr-apple"]
concurrency = 1
```

`--model` selects `qwen3-1.7b` (default), `qwen3-0.6b` or `whisper`. The first run downloads the model.
`[asr].language` is passed to the model.
