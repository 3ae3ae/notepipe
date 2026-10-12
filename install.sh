#!/usr/bin/env bash
set -euo pipefail

# 安装 course2md 预编译二进制到 ~/bin（或 COURSE2MD_BIN_DIR）。
# 用法：
#   curl -fsSL https://raw.githubusercontent.com/mizorewww/course2md/main/install.sh | bash

REPO="${COURSE2MD_REPO:-mizorewww/course2md}"
BIN_DIR="${COURSE2MD_BIN_DIR:-$HOME/bin}"

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"
case "$os-$arch" in
  darwin-arm64|darwin-aarch64) ASSET="course2md-macos-arm64" ;;
  darwin-x86_64) ASSET="course2md-macos-x86_64" ;;
  linux-x86_64|linux-amd64) ASSET="course2md-linux-x86_64" ;;
  linux-aarch64|linux-arm64) ASSET="course2md-linux-aarch64" ;;
  *)
    echo "暂无预编译包：$os $arch。请用 cargo install --path . 从源码安装。" >&2
    exit 1
    ;;
esac

missing=()
need() { command -v "$1" >/dev/null 2>&1 || missing+=("$1"); }
need ffmpeg
need ffprobe
# macOS arm64 预编译包含 CoreML 后端，无需 llama-server；其余平台需要。
# yt-dlp 不在此列：PATH 没有时下面会随包装到 BIN_DIR，兜底逻辑见脚本尾部。
if [ "$os-$arch" != "darwin-arm64" ] && [ "$os-$arch" != "darwin-aarch64" ]; then
  need llama-server
fi
if [ "${#missing[@]}" -gt 0 ]; then
  echo "缺少依赖：${missing[*]}" >&2
  echo "macOS:  brew install ffmpeg yt-dlp llama.cpp" >&2
  echo "Arch:   sudo pacman -S ffmpeg yt-dlp llama-cpp" >&2
  echo "GPU:    Arch/CachyOS also needs a GPU backend (e.g. ggml-vulkan) and the GPU driver; check llama-server --list-devices" >&2
  echo "Debian: sudo apt install ffmpeg yt-dlp && 安装 llama.cpp（见 README）" >&2
  exit 1
fi

mkdir -p "$BIN_DIR"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

if command -v gh >/dev/null 2>&1; then
  gh release download -R "$REPO" -p "$ASSET" -O "$tmp" --clobber
else
  url="$(
    COURSE2MD_REPO="$REPO" ASSET="$ASSET" python3 - <<'PY'
import json, os, urllib.request
repo = os.environ["COURSE2MD_REPO"]
asset = os.environ["ASSET"]
with urllib.request.urlopen(f"https://api.github.com/repos/{repo}/releases/latest") as r:
    data = json.load(r)
for a in data.get("assets", []):
    if a["name"] == asset:
        print(a["browser_download_url"])
        break
else:
    raise SystemExit(f"release 中找不到 {asset}")
PY
  )"
  curl -fsSL "$url" -o "$tmp"
fi

install -m 755 "$tmp" "$BIN_DIR/course2md"
echo "已安装：$BIN_DIR/course2md"

# yt-dlp：PATH 里已有就沿用系统版本；否则从 yt-dlp 官方 release 下载独立二进制
# 装到 BIN_DIR——与 course2md 同目录是运行时搜索清单的一档（随包自带）。
if command -v yt-dlp >/dev/null 2>&1; then
  echo "检测到 yt-dlp：$(command -v yt-dlp)，沿用 PATH 版本"
else
  ytdlp_asset=""
  case "$os-$arch" in
    darwin-*) ytdlp_asset="yt-dlp_macos" ;;
    linux-x86_64|linux-amd64) ytdlp_asset="yt-dlp_linux" ;;
    linux-aarch64|linux-arm64) ytdlp_asset="yt-dlp_linux_aarch64" ;;
  esac
  yt="$(mktemp)"
  if [ -n "$ytdlp_asset" ] &&
     curl -fsSL --max-time 300 "https://github.com/yt-dlp/yt-dlp/releases/latest/download/$ytdlp_asset" -o "$yt"; then
    install -m 755 "$yt" "$BIN_DIR/yt-dlp"
    echo "已安装：$BIN_DIR/yt-dlp（随包自带）"
  else
    echo "警告：yt-dlp 下载失败；远程视频功能需要它，可稍后手动安装" >&2
    echo "      macOS: brew install yt-dlp ；其他: pipx install yt-dlp" >&2
  fi
  rm -f "$yt"
fi

# MLX Metal kernels：CoreML 推理需要与二进制同目录（仅 macOS arm64）
if [ "$os-$arch" = "darwin-arm64" ] || [ "$os-$arch" = "darwin-aarch64" ]; then
  mlb="$(mktemp)"
  mlx_installed=0
  trap 'rm -f "$tmp" "$mlb"' EXIT
  if command -v gh >/dev/null 2>&1; then
    if gh release download -R "$REPO" -p "mlx-macos-arm64.metallib" -O "$mlb" --clobber 2>/dev/null; then
      install -m 644 "$mlb" "$BIN_DIR/mlx.metallib"
      mlx_installed=1
      echo "已安装：$BIN_DIR/mlx.metallib（CoreML 推理所需）"
    fi
  else
    url="$(
      COURSE2MD_REPO="$REPO" python3 - <<'PY'
import json, os, urllib.request
repo = os.environ["COURSE2MD_REPO"]
with urllib.request.urlopen(f"https://api.github.com/repos/{repo}/releases/latest") as r:
    data = json.load(r)
for a in data.get("assets", []):
    if a["name"] == "mlx-macos-arm64.metallib":
        print(a["browser_download_url"])
        break
PY
    )"
    if [ -n "$url" ] && curl -fsSL "$url" -o "$mlb" 2>/dev/null; then
      install -m 644 "$mlb" "$BIN_DIR/mlx.metallib"
      mlx_installed=1
      echo "已安装：$BIN_DIR/mlx.metallib（CoreML 推理所需）"
    fi
  fi
  if [ "$mlx_installed" != 1 ]; then
    echo "警告：MLX Metal 库下载失败，Apple 原生识别（CoreML）将不可用；" >&2
    echo "      可重跑 install.sh 或手动从 release 资产 mlx-macos-arm64.metallib 安装到 $BIN_DIR/mlx.metallib" >&2
  fi
fi

echo "请确保 PATH 包含 ${BIN_DIR}，例如：export PATH=\"\$HOME/bin:\$PATH\""
echo "首次运行会自动下载识别模型（macOS CoreML 约 1-2GB；其他平台 llama.cpp GGUF 约 2.4GB）。"
