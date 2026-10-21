# Security policy

## Reporting a vulnerability

Please report security problems privately through GitHub's
[private vulnerability reporting](https://github.com/3ae3ae/notepipe/security/advisories/new) rather than in a public
issue. Include the version (`notepipe --version`), your platform, and steps to reproduce. Fixes are released as soon as
they are ready and credited in the changelog unless you prefer otherwise.

Only the latest release receives security fixes while notepipe is at version 0.x.

## Scope and trust model

notepipe is a local command-line tool. Keep these properties in mind when you evaluate a report:

- **Configuration is trusted.** It names the programs notepipe runs (adapters and the completion hook) and the endpoints it
  sends audio and text to. Treat a configuration file, or configuration passed with `--config -`, like a script.
- **Credentials** are read from the configuration or from `NOTEPIPE_SPEECH_API_KEY` / `NOTEPIPE_AI_API_KEY` and sent only
  to the endpoints you configure. notepipe does not write API keys to notes, caches or logs. A credential written into a
  URL or into an adapter's arguments is part of that setting and can appear in the cache and in `notepipe doctor`
  output.
- **Speech adapters** started by notepipe listen on `127.0.0.1` for the duration of a run. Other processes on the same machine can
  reach that port while it is open.
- **Media and subtitles** from the internet are untrusted input to ffmpeg and to notepipe's parsers; problems there are in
  scope.
- **Notes** are written with your user's default permissions (umask). Protect the output directory if notes are private.
