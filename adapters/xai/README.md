# notepipe-asr-xai

A [notepipe](https://github.com/3ae3ae/notepipe) speech adapter that forwards transcription requests to
[xAI's speech-to-text API](https://docs.x.ai/developers/model-capabilities/audio/speech-to-text).

## Install

```sh
uv tool install "notepipe[xai]"
```

## Use

```toml
[asr]
command = ["notepipe-asr-xai"]
model = "grok-voice-transcribe-2.0"
api_key = "…"            # or NOTEPIPE_ASR_API_KEY
```

notepipe's API key is forwarded as is. `[asr].language` is sent as xAI's `language` field. `--base-url` overrides the
default `https://api.x.ai/v1`.
