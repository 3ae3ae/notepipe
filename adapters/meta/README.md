# notepipe-asr-meta

A [notepipe](https://github.com/3ae3ae/notepipe) speech adapter that forwards transcription requests to
[Meta's speech-to-text API](https://dev.meta.ai/docs/speech-to-text).

## Install

```sh
uv tool install "notepipe[meta]"
```

## Use

```toml
[asr]
command = ["notepipe-asr-meta"]
model = "muse-voice-transcribe-1.0"
api_key = "…"            # or NOTEPIPE_ASR_API_KEY
```

notepipe's API key is forwarded as is. `[asr].language` (a language code) is sent as a language-name hint.
`--base-url` overrides the default `https://api.meta.ai/v1`. Meta accepts segments up to 10 minutes and 32 MB, well above
notepipe's default segment length.
