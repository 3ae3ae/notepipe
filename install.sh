#!/usr/bin/env bash
set -euo pipefail

# 安装 notepipe 预编译二进制到 ~/bin（或 NOTEPIPE_BIN_DIR）。
# Install the prebuilt notepipe binary into ~/bin (or NOTEPIPE_BIN_DIR).
#   curl -fsSL https://raw.githubusercontent.com/3ae3ae/notepipe/main/install.sh | bash

REPO="${NOTEPIPE_REPO:-3ae3ae/notepipe}"
BIN_DIR="${NOTEPIPE_BIN_DIR:-$HOME/bin}"
RELEASE="https://github.com/${REPO}/releases/latest/download"
YTDLP_RELEASE="https://github.com/yt-dlp/yt-dlp/releases/latest/download"

say() { printf '%s\n' "$@"; }
warn() { printf '%s\n' "$@" >&2; }

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"
case "$os-$arch" in
  darwin-arm64|darwin-aarch64) ASSET="notepipe-macos-arm64"; YTDLP="yt-dlp_macos" ;;
  darwin-x86_64) ASSET="notepipe-macos-x86_64"; YTDLP="yt-dlp_macos" ;;
  linux-x86_64|linux-amd64) ASSET="notepipe-linux-x86_64"; YTDLP="yt-dlp_linux" ;;
  linux-aarch64|linux-arm64) ASSET="notepipe-linux-aarch64"; YTDLP="yt-dlp_linux_aarch64" ;;
  *)
    warn "暂无 $os/$arch 的预编译版本，请从源码安装：cargo install --path . / No prebuilt binary for $os/$arch; build from source with cargo install --path ."
    exit 1
    ;;
esac

if ! command -v ffmpeg >/dev/null 2>&1 || ! command -v ffprobe >/dev/null 2>&1; then
  warn "缺少 ffmpeg（含 ffprobe）/ ffmpeg (with ffprobe) is required:"
  warn "  macOS:         brew install ffmpeg"
  warn "  Arch/CachyOS:  sudo pacman -S ffmpeg"
  warn "  Debian/Ubuntu: sudo apt install ffmpeg"
  exit 1
fi

sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# Verify against the release's SHA256SUMS when it publishes one; older releases do not.
sums="$tmp/SHA256SUMS"
curl -fsSL "$RELEASE/SHA256SUMS" -o "$sums" 2>/dev/null || rm -f "$sums"
verify() {
  local file="$1" name="$2" expected
  [ -f "$sums" ] || return 0
  expected="$(awk -v n="$name" '$2 == n || $2 == "*" n {print $1}' "$sums")"
  [ -n "$expected" ] || return 0
  if [ "$(sha256 "$file")" != "$expected" ]; then
    warn "校验和不匹配：$name / Checksum mismatch: $name"
    exit 1
  fi
}

mkdir -p "$BIN_DIR"
curl -fsSL "$RELEASE/$ASSET" -o "$tmp/notepipe"
verify "$tmp/notepipe" "$ASSET"
install -m 755 "$tmp/notepipe" "$BIN_DIR/notepipe"
say "已安装 / Installed: $BIN_DIR/notepipe"

# 在线视频需要 yt-dlp：PATH 中已有就沿用，否则在程序旁安装官方独立版本。
if command -v yt-dlp >/dev/null 2>&1; then
  say "使用已安装的 yt-dlp / Using installed yt-dlp: $(command -v yt-dlp)"
elif curl -fsSL --max-time 300 "$YTDLP_RELEASE/$YTDLP" -o "$tmp/yt-dlp" &&
     curl -fsSL "$YTDLP_RELEASE/SHA2-256SUMS" -o "$tmp/yt-dlp.sums" &&
     [ "$(sha256 "$tmp/yt-dlp")" = "$(awk -v n="$YTDLP" '$2 == n {print $1}' "$tmp/yt-dlp.sums")" ]; then
  install -m 755 "$tmp/yt-dlp" "$BIN_DIR/yt-dlp"
  say "已安装 / Installed: $BIN_DIR/yt-dlp"
else
  warn "警告：yt-dlp 安装失败，在线视频需要它（本地文件不受影响）。/ Warning: yt-dlp could not be installed; online videos need it (local files still work)."
  warn "  macOS: brew install yt-dlp    其他 / other: pipx install yt-dlp"
fi

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) say "请把 $BIN_DIR 加入 PATH / Add $BIN_DIR to PATH:  export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac
say "运行 notepipe doctor 检查环境 / Run notepipe doctor to check your setup."
