# Security policy

## Reporting a vulnerability

Please report security problems privately through GitHub's
[private vulnerability reporting](https://github.com/3ae3ae/notepipe/security/advisories/new) rather than in a public
issue. Include the version (`notepipe --version`), your platform, and steps to reproduce. Fixes are released as soon as
they are ready and credited in the changelog unless you prefer otherwise.

Only the latest release receives security fixes while notepipe is at version 0.x.

## Scope and trust model

notepipe is a local command-line tool. Keep these properties in mind when you evaluate a report:

- **Configuration is trusted.** It names the programs notepipe runs (provider, download and run programs) and
  the providers it sends audio and text to. Treat a configuration file, or configuration passed with `--config -`, like a script.
- **`run` stages** run the commands written in the configuration, without a shell. Only the paths of the stage's
  input and output files are put into their arguments; values from the internet, such as a downloaded title, never
  are.
- **Credentials** are read from the environment variables that providers name in `api_key_env`, and sent only to that
  provider; they are never part of the configuration. notepipe does not write API keys to notes, caches or logs, but
  the programs it starts inherit its environment, keys included, and what they print is theirs. A
  credential written into a URL or into a program's arguments is part of that setting and can appear in the cache and in
  `notepipe doctor` output.
- **Provider programs** started by notepipe listen on `127.0.0.1` while the stages that use them run. Other processes
  on the same machine can reach that port while it is open.
- **Media and subtitles** from the internet are untrusted input to ffmpeg and to notepipe's parsers; problems there are in
  scope.
- **Notes** are written with your user's default permissions (umask). Protect the output directory if notes are private.
