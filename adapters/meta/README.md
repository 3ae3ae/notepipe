# notepipe-asr-meta

A [notepipe](https://github.com/3ae3ae/notepipe) speech adapter that forwards transcription requests to
[Meta's speech-to-text API](https://dev.meta.ai/docs/speech-to-text).

## Install

```sh
uv tool install "notepipe[meta]"
```

## Use

```toml
[providers.meta]
command = ["notepipe-asr-meta"]
api_key_env = "META_API_KEY"

[stages.transcript.speech]
provider = "meta"
model = "muse-voice-transcribe-1.0"
```

notepipe's API key is forwarded as is. The transcript stage's `language` (a language code) is sent as a language-name hint.
`--base-url` overrides the default `https://api.meta.ai/v1`. Meta accepts segments up to 10 minutes and 32 MB, well above
notepipe's default segment length.
