# notepipe-asr-llama

A [notepipe](https://github.com/3ae3ae/notepipe) speech adapter that runs speech recognition locally with
[llama.cpp](https://github.com/ggml-org/llama.cpp)'s `llama-server`, on CPU or GPU.

It starts `llama-server` on an internal port with the arguments you give it, serves notepipe's adapter API in front of
it, and removes Qwen3-ASR's output markers (`language …<asr_text>`) so notepipe receives plain text.

## Install

Install `llama-server` (for example from your package manager or the llama.cpp releases), then:

```sh
uv tool install "notepipe[llama]"
```

## Use

```toml
[providers.llama]
command = ["notepipe-asr-llama", "-hf", "ggml-org/Qwen3-ASR-1.7B-GGUF:Q8_0"]
concurrency = 1

[stages.transcript.speech]
provider = "llama"
```

Every argument other than `--host`, `--port` and `--llama-server PATH` is passed to `llama-server` unchanged, so you can
add options such as `-ngl 0` (CPU only). The first run downloads the model. The transcript stage's `language` is passed
to the model as a hint.
