# Install this checkout as /Applications/course2md.app and open it.
install:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ "$(uname -s)" != Darwin ]]; then
        echo "just install is macOS-only." >&2
        exit 1
    fi
    if [[ ! -d desktop/.deps/zed || ! -d desktop/.deps/component ]]; then
        echo "Run python3 desktop/scripts/sources.py first." >&2
        exit 1
    fi
    app="$(python3 desktop/scripts/package.py --app-only | tail -1)"

    dest=/Applications/course2md.app
    if pgrep -f "$dest/Contents/MacOS/course2md-desktop" >/dev/null; then
        osascript -e 'tell application "course2md" to quit' >/dev/null 2>&1 || true
        # Give the running app a moment to save state before it is replaced.
        for _ in {1..30}; do
            pgrep -f "$dest/Contents/MacOS/course2md-desktop" >/dev/null || break
            sleep 0.3
        done
        if pgrep -f "$dest/Contents/MacOS/course2md-desktop" >/dev/null; then
            echo "course2md did not quit; refusing to replace the installed app." >&2
            exit 1
        fi
    fi
    staging=/Applications/.course2md.app.installing
    rm -rf "$staging"
    ditto "$app" "$staging"
    rm -rf "$dest"
    mv "$staging" "$dest"
    open "$dest"
