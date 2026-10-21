# notepipe-asr-openvino

A [notepipe](https://github.com/3ae3ae/notepipe) speech adapter that runs speech recognition locally with
[OpenVINO GenAI](https://github.com/openvinotoolkit/openvino.genai), on an Intel NPU by default.

## Install

```sh
uv tool install notepipe --with notepipe-asr-openvino
```

## Use

```toml
[asr]
command = ["notepipe-asr-openvino"]
concurrency = 1
```

| Option | Default | Meaning |
|---|---|---|
| `--model` | `dseditor/Qwen3-ASR-1.7B-INT8_OpenVINO` | Hugging Face repository or local directory of an OpenVINO Qwen3-ASR or Whisper model |
| `--device` | `NPU` | `NPU`, `CPU` or `GPU` |

The first run downloads the model. `[asr].language` is passed to the model.
