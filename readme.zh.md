# course2md

把 **YouTube、Bilibili 或本地视频**转换成带截图的 Markdown / HTML 笔记。在桌面应用里添加视频、生成笔记，再用笔记库整理和阅读。

[English](readme.md) · **中文** · [GitHub Wiki](https://github.com/mizorewww/course2md/wiki)

**试用 2.0（RC11）：**[发布说明](https://github.com/mizorewww/course2md/releases/tag/v2.0.0-rc.11)。Homebrew 用户运行 `brew install --cask mizorewww/tap/course2md-gui@rc`。下面的安装方式提供的是稳定版。

## 功能特性

- 幻灯片式笔记：画面变化时自动截图，文字稿按截图分段，保存为 `course.md` 与 `course.html`。
- 支持 YouTube、Bilibili 与本地文件。有字幕时直接使用字幕，没有字幕时自动识别语音。
- 语音识别可在本机完成：Apple 芯片开箱即用；其他系统可用显卡、CPU 或 Intel Core Ultra 的 NPU。也可选择云端服务，只有选择云端时音频才会上传。
- 可选 AI 校对与摘要：支持任意 OpenAI 兼容服务、本地 Ollama 或 ChatGPT 订阅，默认关闭。
- 桌面应用提供可暂停的后台任务、支持文件夹与搜索的笔记库、带目录与查找的阅读器，以及 10 套主题。


## 界面语言

桌面应用与命令行支持**英语、简体中文、繁体中文和日语**。桌面应用默认跟随系统语言，可在**设置 → 外观 → 界面语言**中更改。命令行跟随系统语言，也可用 `--lang en|zh-Hans|zh-Hant|ja`（或 `COURSE2MD_LANG`）指定。`--json` 的机器可读消息保持简体中文，便于脚本稳定解析。

## 安装桌面应用（GUI）

桌面应用自带转换引擎，不需要另外安装 CLI；但需要 `ffmpeg`，下面的步骤会说明如何安装。

### macOS：M 系列芯片，macOS 15 及以上

**已安装 Homebrew：**在终端运行以下命令，它会安装应用及视频工具 `ffmpeg`、`yt-dlp`：

```sh
brew install --cask mizorewww/tap/course2md-gui
```

装完后，从「访达 → 应用程序」打开 **course2md**。

**不使用 Homebrew：**[下载 DMG](https://github.com/mizorewww/course2md/releases/latest/download/course2md-gui-macos-arm64.dmg)，打开后把 **course2md** 拖入「应用程序」。另外需要安装 `ffmpeg`，见 [macOS 指南](https://github.com/mizorewww/course2md/wiki/%E5%AE%89%E8%A3%85%E6%A1%8C%E9%9D%A2%E5%BA%94%E7%94%A8#macos-apple-silicon)。

### Arch Linux / CachyOS：x86_64

已安装 `yay` 的用户运行以下命令；它会安装桌面应用、视频工具和所需图形库：

```sh
yay -S course2md-gui-bin
```

使用 `paru` 时运行 `paru -S course2md-gui-bin`。装完后从应用菜单打开 **course2md**。显卡需有可用的 Vulkan 驱动，详见 [Linux 安装指南](https://github.com/mizorewww/course2md/wiki/%E5%AE%89%E8%A3%85%E6%A1%8C%E9%9D%A2%E5%BA%94%E7%94%A8#arch-linux--cachyos)。

### Windows：x64

1. [下载 Windows 便携包（ZIP）](https://github.com/mizorewww/course2md/releases/latest/download/course2md-desktop-windows-AMD64.zip)，**完整解压**到固定文件夹。
2. 在 PowerShell 中安装 `ffmpeg`：

   ```powershell
   winget install --id Gyan.FFmpeg -e
   ```

3. 打开解压目录中的 **course2md-desktop.exe**，其他文件保持在同一目录。

### Ubuntu / 其他 Linux：x86_64

[下载 Linux 包（tar.gz）](https://github.com/mizorewww/course2md/releases/latest/download/course2md-desktop-linux-x86_64.tar.gz)，按 [Linux 指南](https://github.com/mizorewww/course2md/wiki/%E5%AE%89%E8%A3%85%E6%A1%8C%E9%9D%A2%E5%BA%94%E7%94%A8#其他-linux-x64)安装 `ffmpeg` 和图形库，解压后运行 `./course2md-desktop`。基于 Ubuntu 24.04 构建。

Intel Mac 和 Linux ARM64 暂无桌面应用，可以使用 [CLI](https://github.com/mizorewww/course2md/wiki/CLI-%E6%8C%87%E5%8D%97)。

## 第一次使用

1. 打开 **course2md**，按提示选择笔记保存位置和语音识别方式。
2. 在「设置 → 应用」的「运行检查」中确认视频工具已被检测到。
3. 添加视频链接或本地文件，确认预览后点击「开始转换」，完成后在「笔记库」打开笔记。

第一次在本机识别语音时会下载模型（1–2.4 GB），详见[首次使用指南](https://github.com/mizorewww/course2md/wiki/%E5%AE%89%E8%A3%85%E6%A1%8C%E9%9D%A2%E5%BA%94%E7%94%A8#首次启动)。

## 安装 CLI

CLI 与桌面应用共享同一份配置文件，可以同时安装。

**macOS / Linux（预编译二进制）：**

```sh
curl -fsSL https://raw.githubusercontent.com/mizorewww/course2md/main/install.sh | bash
```

脚本把 `course2md` 装进 `~/bin`（可用 `COURSE2MD_BIN_DIR` 修改）。也可以用 `brew install mizorewww/tap/course2md`、`yay -S course2md-bin`，或从源码 `cargo install --path .`。

需要 `ffmpeg`；在线链接还需要 `yt-dlp`（缺少时会自动安装）；非 Apple 系统的本地识别需要 `llama-server`。运行 `course2md doctor` 检查。

## 快速上手

```sh
course2md ./lecture.mp4                        # 本地文件
course2md https://www.youtube.com/watch?v=…    # 或 Bilibili 链接
course2md ./lecture.mp4 -o ./notes             # 指定输出根目录
```

第一次运行会询问几项设置。笔记保存在输出目录的 `平台/标题/ID/` 下。

## 常用命令

| 命令 | 用途 |
| --- | --- |
| `course2md <链接或文件>` | 把视频转换成笔记 |
| `course2md --login bilibili` / `--logout bilibili` | 扫码登录 Bilibili / 清除保存的登录 |
| `course2md --login ollama` / `--login codex` | 使用本地 Ollama 服务 / 导入 codex CLI 的 ChatGPT 登录态（未安装时可用浏览器授权） |
| `course2md doctor` | 检查依赖、识别后端和配置 |
| `course2md config init` / `config show` | 生成配置模板 / 显示配置路径和文件设置 |
| `course2md models list` / `models prepare` | 检查已下载模型 / 预先下载模型 |
| `course2md llm setup` / `llm status` / `llm disable` | 配置、查看或关闭 AI 校对 |
| `course2md summarize ./notes` | 为已有笔记生成 AI 摘要 |

使用 `<命令> --help` 查看每个子命令的参数。

## 配置

设置保存在 `~/.config/course2md/config.toml`（Windows 为 `%APPDATA%\course2md\config.toml`），与桌面应用共用。命令行参数优先于配置文件。`course2md remove` 清除已保存的 AI 服务设置。

## 常见问题

**转换时报缺少 `ffmpeg`。**安装 `ffmpeg`（含 `ffprobe`）；处理在线链接还需 `yt-dlp`，例如 macOS 上 `brew install ffmpeg yt-dlp`，然后运行 `course2md doctor` 确认。

**Bilibili 视频失败或清晰度低。**运行 `course2md --login bilibili` 扫码登录；`--logout bilibili` 清除保存的登录。

**模型下载有多大？**Apple 芯片约 1–2.3 GB（保存在 `~/Library/Caches/qwen3-speech/`），其他平台约 2.4 GB（`~/.cache/course2md/models/`）。可用 `course2md models prepare` 预先下载。

## 文档与帮助

[全部文档（GitHub Wiki）](https://github.com/mizorewww/course2md/wiki) · [CLI 安装与使用](https://github.com/mizorewww/course2md/wiki/CLI-%E6%8C%87%E5%8D%97) · [升级与卸载](https://github.com/mizorewww/course2md/wiki/%E5%AE%89%E8%A3%85%E6%A1%8C%E9%9D%A2%E5%BA%94%E7%94%A8#升级与卸载) · [故障排查](https://github.com/mizorewww/course2md/wiki/%E6%95%85%E9%9A%9C%E6%8E%92%E6%9F%A5) · [反馈问题](https://github.com/mizorewww/course2md/issues)

面向开发者：[贡献约定](AGENTS.md) · [桌面端开发](desktop/README.md) · [引擎架构](docs/DESIGN.md) · [性能基准](docs/BENCHMARKS.md) · [发布](docs/PACKAGING.md) · [更新记录](CHANGELOG.md) · [MIT 许可证](LICENSE)
