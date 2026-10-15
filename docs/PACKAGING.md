# Package channels and release automation

[GitHub Wiki](https://github.com/mizorewww/course2md/wiki)

| Channel | GUI | CLI |
| --- | --- | --- |
| Homebrew tap `mizorewww/tap` | Cask `course2md-gui` (Apple Silicon, macOS 15+) | Formula `course2md` |
| Homebrew prerelease channels | Cask `course2md-gui@{alpha,beta,rc}` | Formula `course2md-{alpha,beta,rc}` (keg-only) |
| AUR (stable releases only) | `course2md-gui-bin` (x86_64) | `course2md-bin` (x86_64, aarch64) |

GUI packages contain a matching private CLI engine. They do not install a terminal `course2md` command and can coexist with the standalone CLI package. Both channels install ffmpeg and yt-dlp. The desktop packages additionally bundle a standalone yt-dlp next to the executables (skipped when the download fails, `COURSE2MD_NO_YTDLP` is set, or `COURSE2MD_YTDLP` supplies a local build); at runtime both app and CLI resolve yt-dlp in the order `COURSE2MD_YTDLP` → beside the executable → PATH → well-known install locations. Model downloads and optional recognition backends are configured on first use; they are not bundled into the package.

## Automatic publication

1. The `release` workflow builds the CLI and native GUI packages and publishes a GitHub Release with a `SHA256SUMS` file. Versions with a `-alpha.N`, `-beta.N` or `-rc.N` suffix are marked as pre-releases and do not replace Latest.
2. `brew-tap` and `aur` run on successful completion through `workflow_run`. This also works when the release was started manually on `main`; `packaging/release-version.py` resolves the published release for that commit.
3. Homebrew updates the formula and cask of the release's channel in `mizorewww/homebrew-tap` (`course2md` / `course2md-gui` for stable, `course2md-rc` / `course2md-gui@rc` for an RC, and so on), including release checksums. Other channels are left unchanged.
4. AUR skips pre-releases. For stable releases it renders both PKGBUILDs and `.SRCINFO` files. It builds the GUI package on Arch, validates the desktop entry, runs the bundled CLI version check and checks for missing shared libraries before pushing either recipe.

Recipes are sourced from `packaging/aur/` and `packaging/homebrew/`. Change these templates, not only the external package repositories; future releases render them again. No release binaries are rebuilt by these synchronization jobs.

## Manual retry and validation

In GitHub Actions, run `brew-tap` (any published version) or `aur` (stable versions only) with `version: <version>`. The AUR workflow supports `dry_run: true` to build and validate without pushing. Normal manual runs publish both packages. Jobs serialize per channel to avoid concurrent pushes.

The workflows use the existing `AUR_SSH_PRIVATE_KEY` secret: the AUR job authenticates to AUR, and the Homebrew job currently reuses the same key, which is also authorized on GitHub, to push the tap. A dedicated tap deploy key is listed in [FOLLOW-UPS](FOLLOW-UPS.md). `GITHUB_TOKEN` reads release metadata. Credentials are never written to package recipes.

On Arch, with the runtime dependencies and `base-devel`, `git`, `curl`, `desktop-file-utils` installed, run as a non-root user:

```sh
packaging/aur/update-aur.sh <version> --dry-run
```

The GUI package installs its application and engine under `/usr/lib/course2md-desktop/`, an application launcher in `/usr/bin/course2md-desktop`, a desktop entry and a 512×512 icon. It does not own `/usr/bin/course2md`.

## Documentation

User documentation is maintained in the [GitHub Wiki](https://github.com/mizorewww/course2md/wiki). Edit pages there or clone the separate wiki repository:

```sh
git clone git@github.com:mizorewww/course2md.wiki.git
```

Keep the Chinese and English guides and the wiki sidebar aligned. README links must point to published Wiki pages. The main repository keeps development records under `docs/`; it does not keep a second copy of the user guides.
