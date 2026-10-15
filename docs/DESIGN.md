# course2md 架构：转换引擎

本文说明转换引擎的管线与边界。桌面界面见 [设计 skill](../.agents/skills/course2md-design/SKILL.md)。

course2md 是单一用途的工具：把 YouTube、Bilibili 或本地网课视频转换为按时间组织的截图与文字稿。有平台字幕时默认直接使用字幕（`--transcript-source subtitle|asr` 可强制指定），下图的语音识别分支只在没有字幕时运行。

## 管线

```text
URL ── yt-dlp ─┐
               ├─> 视频源 + meta.json
本地文件 ──────┘     (本地原地读取，在线下载为 media.mp4)
                         │
              ┌──────────┴──────────┐
              │                     │
              ▼                     ▼
  ffmpeg 定时灰度采样        ffmpeg 提取 16 kHz
  ROI + SSIM 比较            单声道 audio.wav
  cooldown 去抖                         │
  精确抽帧到 frames/         ┌──────────┴──────────┐
              │              ▼                     ▼
              │      [macOS Apple Silicon]    [通用 GPU / CPU]
              │      Silero VAD (CoreML)      silencedetect 分段
              │              │                     │
              │              ▼                     ▼
              │      Qwen3-ASR (MLX/CoreML)   llama-server + Qwen3-ASR
              │      (speech-swift 运行时)    GGUF (300s 就绪超时)
              │              │                     │
              │              └──────────┬──────────┘
              │                         │
              │                         ▼
              │              按截图与自然停顿组织段落
              │                         │
              │                         ▼
              │              [可选] LLM 段落校对 (默认关闭)
              │              OpenAI 兼容接口 / 20段批量校对
              └──────────┬──────────────┘
                         ▼
              按时间合并截图与语音
                         │
              timeline.jsonl
                         │
          course.md / course.html / structured.json
```

截图与音频提取并行执行。画面检测是三状态机（last_emitted / candidate / 发射）：采样不休眠，`--cooldown` 只限制发射频率，发射时间戳取候选画面首次出现的时间；默认 `--slide-mode stable` 要求画面稳定 `--stable-secs`（默认 0.8s）后才发射，从而跳过 PPT 动画的中间帧。

语音分段（各本地后端共享）：超过 `--max-speech` 的段在目标切点 ±3s 窗口内的能量最低点切断，避免切在词中间；切音频时向两侧静音各填充 0.25s，不进入相邻语音，因此不会产生重复文本。事件时间与切分范围分离（`Seg { start, end, cut_start, cut_end }`）。

时间线合并：每段语音按时间中点归属截图；跨截图边界时只在边界附近的句读或空格处拆分（保持图文对应），找不到自然断点则整段保留，避免按字符比例从词中间截断。细粒度原始事件先完整写入 `timeline.jsonl`；随后同一截图下短停顿（<3.5s）内的连续片段组织为自然段（上限 420 字），独立出现的无语义填充词被过滤。`course.md`/`course.html`/`structured.json` 使用这些段落；LLM 校对的单位是段落而非 VAD 碎片（视觉润色时另附该节截图），校对后以 `raw` 保留校对前文本。

语音识别后端（`--provider`）：
1. **CoreML 原生路径**（macOS Apple Silicon 预编译包默认）：通过静态链接的 `speech-swift` 运行 Silero VAD CoreML（ANE）与 Qwen3-ASR（默认 1.7B MLX 8bit，走 GPU；可选 0.6B CoreML 走 ANE 省电，或 Whisper large-v3-turbo），不需要外部运行时。失败时报错，保留任务参数与进度，不会自动换用其他后端。
2. **llama.cpp 路径**（`gpu` / `cpu`，Linux / Windows / 通用兜底）：由 ffmpeg `silencedetect` 分段，逐段提交给本地 `llama-server` 进程。模型为约 2.4GB 的 Qwen3-ASR GGUF，缺失时自动下载。
3. **Intel NPU**（`npu`）：通过 OpenVINO GenAI 运行 Whisper 导出模型。
4. **云端**（`api`）：音频分段上传到 OpenAI 兼容的语音识别接口。

若开启 LLM 润色（默认关闭），段落按 20 条一批发送到 OpenAI 兼容端点、本地 Ollama 或 Codex 订阅，做口语修正与同音字纠错；单批失败时保留原文，不阻断转换。可选的 AI 总结（`--summarize`、`course2md summarize`）复用同一服务。

配置优先级：命令行参数 > `config.toml` > 内置默认值。配置文件位于 `~/.config/course2md/config.toml`（Windows 为 `%APPDATA%\course2md\config.toml`）。

默认生成 Markdown 和 HTML；JSON 通过 `--formats` 启用。本地文件原地读取，不改动原文件；在线下载的 `media.mp4` 在转换完成后默认删除。每次生成写入课程目录下的 `versions/<version_id>/`，`current.json` 指向当前版本。

## 模块

模块清单以 `src/lib.rs` 为准，每个文件开头的文档注释说明其职责。主要入口：`main.rs`（CLI 分发）、`pipeline.rs`（全流程编排）、`execution.rs`（桌面任务协议 `run-task`）、`artifact.rs`（版本化输出）。`build.rs` 在 macOS arm64 上编译 `native/apple-asr`（封装 speech-swift 的 Swift 静态库）并复制 `mlx.metallib`。除非选择云端识别或 AI 服务，模型与识别数据都保留在本地。

## 非目标

- 不做说话人分离或翻译；校对与摘要是可选的 LLM 功能，默认关闭。
- 转换内核不做 GUI 或服务端；任务队列与批量转换由 `desktop/` 的桌面应用负责，它通过 `course2md run-task` 把本 CLI 当作转换引擎。
- 不接管 CUDA、Vulkan 等繁杂运行时配置；GPU 能力由用户安装的 llama.cpp 决定，`--provider cpu` 可强制 CPU。
- 不内建视频网站解析、音视频编解码；这些职责分别交给 yt-dlp 与 ffmpeg。
