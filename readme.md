# course2md

Turn **YouTube, Bilibili, or local videos** into illustrated Markdown / HTML notes. Add a video in the desktop app, generate notes, then organize and read them in your notes library.

**English** · [中文](readme.zh.md) · [GitHub Wiki](https://github.com/mizorewww/course2md/wiki)

**Preview 2.0 (RC9):** [release notes](https://github.com/mizorewww/course2md/releases/tag/v2.0.0-rc.9). Install with `brew install --cask mizorewww/tap/course2md-gui@rc`. The instructions below install the stable version.

## Features

- Slide-style notes: screenshots are captured when the picture changes, the transcript is organized into paragraphs under each screenshot, and everything is exported as `course.md` and `course.html` (add JSON with `--formats`).
- YouTube, Bilibili and local files. Platform subtitles are preferred; speech recognition runs when no subtitles exist (`--transcript-source subtitle|asr` forces either side).
- Speech recognition on your own computer: Apple Silicon works out of the box, and other systems can use a graphics card, the CPU, or an Intel Core Ultra NPU. A cloud service is optional; audio leaves your computer only if you choose it.
- Optional AI proofreading and summaries through any OpenAI-compatible endpoint, a local Ollama server (`--login ollama`), or a ChatGPT subscription via Codex login (`--login codex`) — off by default; `course2md llm setup` configures a custom endpoint.
- Resume interrupted conversions with `--resume`. For scripts: machine-readable progress (`--json`) and quiet mode (`-q`). CLI help is bilingual (中文 / English).
- The desktop app adds background tasks with pause and cancel, a notes library with folders and search, a reader with outline, find, version history and export, and 10 built-in themes.

## Install the desktop app (GUI)

Choose one installation method for your system below. **The GUI includes its conversion engine; no separate CLI install is needed.**

### macOS: Apple Silicon, macOS 15 or later

**If you have Homebrew:** run this in Terminal to install the app and the video tools `ffmpeg` and `yt-dlp`:

```sh
brew install --cask mizorewww/tap/course2md-gui
```

After installation, open **course2md** from **Finder → Applications**.

**Manual installation:** [download the macOS installer (DMG)](https://github.com/mizorewww/course2md/releases/latest/download/course2md-gui-macos-arm64.dmg), open it, and drag **course2md.app** to Applications. The app bundles yt-dlp; only `ffmpeg` needs to be installed separately using the [macOS installation guide](https://github.com/mizorewww/course2md/wiki/GUI-Installation#macos-apple-silicon).

### Arch Linux / CachyOS: x86_64

If you have `yay`, run this to install the desktop app, video tools and required graphics libraries:

```sh
yay -S course2md-gui-bin
```

With `paru`, run `paru -S course2md-gui-bin`. Open **course2md** from your applications menu. Your graphics hardware needs a working Vulkan driver; see the [Linux installation guide](https://github.com/mizorewww/course2md/wiki/GUI-Installation#arch-linux--cachyos).

### Windows: x64

1. [Download the Windows portable package (ZIP)](https://github.com/mizorewww/course2md/releases/latest/download/course2md-desktop-windows-AMD64.zip) and **extract everything** to a permanent folder.
2. Install `ffmpeg` in PowerShell (`yt-dlp.exe` is bundled in the package; a yt-dlp already on PATH is also used):

   ```powershell
   winget install --id Gyan.FFmpeg -e
   ```

3. Open **course2md-desktop.exe** from the extracted folder. Keep `course2md.exe` beside it: this is the bundled engine. If the app was already open, quit and reopen it after installing tools.

### Ubuntu / other Linux distributions: x86_64

[Download the Linux desktop package (tar.gz)](https://github.com/mizorewww/course2md/releases/latest/download/course2md-desktop-linux-x86_64.tar.gz), install video tools and graphics libraries using the [Linux installation guide](https://github.com/mizorewww/course2md/wiki/GUI-Installation#other-linux-x64-distributions), then extract everything. Enter the extracted folder and run `./course2md-desktop`. Keep the `course2md` engine in the same folder.

The prebuilt package is based on Ubuntu 24.04. **Intel Mac and Linux ARM64 do not yet have prebuilt GUIs**; use the [CLI version](https://github.com/mizorewww/course2md/wiki/CLI-Guide). GitHub's **Source code** archives are developer sources, not app installers.

## First use

1. Open **course2md** and follow the setup guide to choose a notes folder and speech recognition method.
2. Open **Settings → App** and check **Runtime checks** to confirm the video tools are detected.
3. Add a video URL or local file, review the preview, and click **Start conversion**. Open the finished notes from **Notes library**.

Local recognition downloads models on first use; the app provides setup controls for each backend. See the [first-launch guide](https://github.com/mizorewww/course2md/wiki/GUI-Installation#first-launch) for details.

## Install the CLI

The CLI and the desktop app share one configuration file and can be installed side by side.

**macOS / Linux (prebuilt binary):**

```sh
curl -fsSL https://raw.githubusercontent.com/mizorewww/course2md/main/install.sh | bash
```

This installs `course2md` into `~/bin` (override with `COURSE2MD_BIN_DIR`), checks dependencies, and installs a bundled `yt-dlp` when none is on PATH. Also available: Homebrew `brew install mizorewww/tap/course2md`; Arch Linux `yay -S course2md-bin` (x86_64, aarch64); or from source with `cargo install --path .` (Rust stable).

Requires `ffmpeg`. Online links also need `yt-dlp` (installed automatically if missing). Local recognition on non-Apple systems needs `llama-server`. Run `course2md doctor` to check.

## Quick start

```sh
course2md ./lecture.mp4                        # local file
course2md https://www.youtube.com/watch?v=…    # or a Bilibili link
course2md ./lecture.mp4 -o ./notes             # choose the output root
```

The first conversion in a terminal offers interactive setup; scripts should pass options explicitly. Each note is created under the output root as `platform/title/ID/`, containing `course.md`, `course.html` and the screenshots.

## Common commands

| Command | Purpose |
| --- | --- |
| `course2md <URL or file>` | Convert a video into notes |
| `course2md --login bilibili` / `--logout bilibili` | Scan a QR code to log in to Bilibili / remove the saved login |
| `course2md --login ollama` / `--login codex` | Use a local Ollama server / import the Codex CLI's ChatGPT login (browser authorization offered when absent) |
| `course2md doctor` | Check tools, speech backends and settings |
| `course2md config init` / `config show` | Create a configuration template / show the config path and settings |
| `course2md models list` / `models prepare` | Check downloaded models / pre-download a model |
| `course2md llm setup` / `llm status` / `llm disable` | Configure, inspect or turn off AI proofreading |
| `course2md summarize ./notes` | Add AI summaries to existing notes |

Use `<command> --help` for each command's options.

## Configuration

The configuration file is `~/.config/course2md/config.toml` (`%APPDATA%\course2md\config.toml` on Windows; `XDG_CONFIG_HOME` is respected). Precedence: command-line arguments > `config.toml` > built-in defaults. `course2md config init` writes a template to get started. AI proofreading is optional and off by default: `course2md llm setup` saves the endpoint, key and model, and `course2md remove` clears saved AI service settings.

## Frequently asked questions

**Conversion fails with a missing `ffmpeg`.** Install `ffmpeg` (it includes `ffprobe`) and `yt-dlp` for online links — for example `brew install ffmpeg yt-dlp` on macOS — then run `course2md doctor` to confirm.

**A Bilibili video fails or comes back in low quality.** Run `course2md --login bilibili` and scan the QR code; `--logout bilibili` removes the saved login.

**How large are the model downloads?** The first local recognition downloads the model automatically: about 1–2.3 GB for macOS CoreML models (cached in `~/Library/Caches/qwen3-speech/`) and about 2.4 GB for the llama.cpp GGUF (cached in `~/.cache/course2md/models/`). Pre-download with `course2md models prepare`, or inspect the cache with `course2md models inspect`.

## Documentation and help

[All documentation (GitHub Wiki)](https://github.com/mizorewww/course2md/wiki) · [CLI installation and usage](https://github.com/mizorewww/course2md/wiki/CLI-Guide) · [Upgrade and uninstall](https://github.com/mizorewww/course2md/wiki/GUI-Installation#upgrade-and-uninstall) · [Troubleshooting](https://github.com/mizorewww/course2md/wiki/Troubleshooting) · [Report a problem](https://github.com/mizorewww/course2md/issues)

For developers: [desktop build guide](desktop/README.md) · [development docs](docs/README.md) · [architecture](docs/DESIGN.md) · [benchmarks](docs/BENCHMARKS.md) · [packaging and releases](docs/PACKAGING.md) · [Changelog](CHANGELOG.md) · [MIT license](LICENSE)
