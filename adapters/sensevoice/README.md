# notepipe-asr-sensevoice

A [notepipe](https://github.com/3ae3ae/notepipe) speech adapter that runs
[SenseVoice](https://github.com/FunAudioLLM/SenseVoice) locally with [FunASR](https://github.com/modelscope/FunASR), on
an NVIDIA GPU, Apple silicon or the CPU. SenseVoice recognizes Chinese, English, Japanese, Korean and Cantonese.

## Install

```sh
uv tool install "notepipe[sensevoice]"
```

On Linux this installs PyTorch with its CUDA libraries, about 3 GB of downloads. Without an NVIDIA GPU, install the
CPU-only build of PyTorch instead:

```sh
uv tool install "notepipe[sensevoice]" --torch-backend cpu
```

On Windows, PyTorch from PyPI runs on the CPU only; add `--torch-backend auto` to use an NVIDIA GPU.

## Use

```toml
[providers.sensevoice]
command = ["notepipe-asr-sensevoice"]
concurrency = 1

[stages.transcript.speech]
provider = "sensevoice"
```

| Option | Default | Meaning |
|---|---|---|
| `--model` | `FunAudioLLM/SenseVoiceSmall` | Hugging Face repository or local directory of a SenseVoice model |
| `--device` | `cuda`, else `mps`, else `cpu` | PyTorch device, such as `cpu` or `cuda:1` |

The first run downloads the model, about 940 MB. The transcript stage's `language` must be `zh`, `en`, `ja`, `ko`,
`yue` or `auto`, with or without a region such as `zh-TW`. Any other language fails every segment, and notepipe reports
that the service rejected the request parameters.
