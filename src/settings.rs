//! 配置文件（~/.config/notepipe/config.toml）。
//! 优先级：命令行参数 > 配置文件 > 内置默认值。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// `[defaults]`：命令行参数的默认值。全部可选，未设置的项回落内置默认。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    pub out: Option<PathBuf>,
    pub cookies: Option<PathBuf>,
    pub cookies_from_browser: Option<String>,
    pub similarity: Option<f64>,
    pub sample_interval: Option<f64>,
    pub cooldown: Option<f64>,
    pub slide_mode: Option<crate::config::SlideMode>,
    pub stable_secs: Option<f64>,
    pub max_height: Option<u32>,
    pub roi: Option<String>,
    /// 转写来源：auto（字幕优先）| subtitle | asr
    pub transcript_source: Option<crate::config::TranscriptSource>,
    pub formats: Option<Vec<crate::config::OutputFormat>>,
    pub keep_video: Option<bool>,
    pub no_download: Option<bool>,
    pub resume: Option<bool>,
}

/// HTTP transcription dialect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AsrMode {
    #[default]
    Transcriptions,
    Chat,
    Xai,
    Meta,
}

impl std::fmt::Display for AsrMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Transcriptions => "transcriptions",
            Self::Chat => "chat",
            Self::Xai => "xai",
            Self::Meta => "meta",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Asr {
    pub mode: AsrMode,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub language: String,
    pub concurrency: usize,
    pub command: Vec<String>,
    pub max_speech: f32,
    pub vad_threshold: f32,
    pub min_silence: f32,
}

impl Default for Asr {
    fn default() -> Self {
        Self {
            mode: AsrMode::Transcriptions,
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            language: String::new(),
            concurrency: 4,
            command: Vec::new(),
            max_speech: crate::config::DEFAULT_MAX_SPEECH,
            vad_threshold: 0.5,
            min_silence: 0.4,
        }
    }
}

impl Asr {
    pub fn is_configured(&self) -> bool {
        !self.command.is_empty() || !self.base_url.trim().is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ConfigFile {
    pub defaults: Defaults,
    pub llm: crate::llm::LlmSettings,
    pub asr: Asr,
}

pub fn config_path() -> PathBuf {
    crate::config::config_dir().join("config.toml")
}

/// 读取配置文件。文件不存在 → 默认值；存在但无法解析 → 硬错误（带位置信息），
/// Invalid settings must not silently fall back to defaults.
pub fn load() -> anyhow::Result<ConfigFile> {
    let p = config_path();
    if !p.is_file() {
        return Ok(ConfigFile::default());
    }
    let s = std::fs::read_to_string(&p).with_context(|| {
        format!(
            "无法读取配置文件 / Cannot read configuration: {}",
            p.display()
        )
    })?;
    toml::from_str(&s).with_context(|| {
        format!(
            "配置格式有误，请修正后重试 / Fix the configuration syntax and retry: {}",
            p.display()
        )
    })
}

pub fn save(cfg: &ConfigFile) -> Result<PathBuf> {
    let p = config_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // 覆盖前备份旧配置（用户可能有手改内容），失败只告警不阻断
    if p.is_file() {
        let bak = p.with_extension("toml.bak");
        if let Err(e) = std::fs::read(&p)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| crate::checkpoint::atomic_write(&bak, &bytes))
        {
            tracing::warn!(
                "无法备份旧配置 / Could not back up configuration to {}: {e}",
                bak.display()
            );
        }
    }
    let bytes = toml::to_string_pretty(cfg)?;
    crate::checkpoint::atomic_write(&p, bytes.as_bytes())?;
    Ok(p)
}

/// `config init` 写入的带注释模板。默认值占位符由内置默认渲染，注释不会与实际默认脱节。
pub fn template() -> String {
    use crate::config as c;
    let formats = crate::options::default_formats()
        .iter()
        .map(|f| format!("\"{f}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let asr = Asr::default();
    let llm = crate::llm::LlmSettings::default();
    [
        ("{OUT}", c::DEFAULT_OUT_DIR.to_string()),
        ("{SIMILARITY}", c::DEFAULT_SIMILARITY.to_string()),
        (
            "{SAMPLE_INTERVAL}",
            format!("{:?}", c::DEFAULT_SAMPLE_INTERVAL),
        ),
        ("{COOLDOWN}", format!("{:?}", c::DEFAULT_COOLDOWN)),
        ("{MAX_HEIGHT}", c::DEFAULT_MAX_HEIGHT.to_string()),
        ("{STABLE_SECS}", format!("{:?}", c::DEFAULT_STABLE_SECS)),
        ("{MAX_SPEECH}", format!("{:?}", asr.max_speech)),
        ("{ASR_CONCURRENCY}", asr.concurrency.to_string()),
        ("{VAD_THRESHOLD}", asr.vad_threshold.to_string()),
        ("{MIN_SILENCE}", asr.min_silence.to_string()),
        ("{FORMATS}", formats),
        ("{SLIDE_MODE}", c::SlideMode::default().to_string()),
        ("{KEEP_VIDEO}", "false".to_string()),
        ("{RESUME}", "true".to_string()),
        (
            "{TRANSCRIPT_SOURCE}",
            c::TranscriptSource::default().to_string(),
        ),
        ("{LLM_ENABLED}", llm.enabled.to_string()),
        ("{LLM_BASE_URL}", llm.base_url),
        ("{LLM_MODEL}", llm.model),
        ("{LLM_VISION}", llm.vision.to_string()),
        ("{LLM_SUMMARIZE}", llm.summarize.to_string()),
        ("{LLM_DISABLE_HINT}", llm.disable_hint.to_string()),
        ("{LLM_CONCURRENCY}", llm.concurrency.to_string()),
    ]
    .into_iter()
    .fold(TEMPLATE.to_string(), |text, (key, value)| {
        text.replace(key, &value)
    })
}

const TEMPLATE: &str = r#"# notepipe 配置 / Configuration
# 优先级：命令行 > 本文件 > 内置默认 / Priority: CLI > this file > built-in defaults
# 取消注释以设置默认值 / Uncomment values to set defaults
# 查看当前设置：notepipe config show / View settings: notepipe config show

[defaults]
# 可选的 yt-dlp Cookie 来源，可同时设置两项 / Optional yt-dlp cookie sources; both may be set
#cookies = "/path/to/cookies.txt"
#cookies_from_browser = "firefox:default-release"
# 输出根目录，按平台/标题/ID 分类 / Output root, organized by platform/title/ID
#out = "{OUT}"
# 截图相似度 (0,1]，越高截图越多 / Similarity (0,1]; higher captures more slides
#similarity = {SIMILARITY}
# 画面采样间隔（秒）/ Frame sampling interval (seconds)
#sample_interval = {SAMPLE_INTERVAL}
# 截图最短间隔（秒）/ Minimum interval between slides (seconds)
#cooldown = {COOLDOWN}
# 下载视频最大高度 / Maximum downloaded video height
#max_height = {MAX_HEIGHT}
# first 立即截图，stable 等动画结束 / first: immediate; stable: wait for animations
#slide_mode = "{SLIDE_MODE}"
#stable_secs = {STABLE_SECS}
# 仅比较指定画面区域 / Compare only this region
#roi = "40%,0%-100%,100%"
# auto 优先字幕，无字幕再识别 / auto: subtitles first, then speech recognition
# subtitle 仅字幕；asr 仅识别 / subtitle: subtitles only; asr: speech recognition only
#transcript_source = "{TRANSCRIPT_SOURCE}"
# 输出格式 / Output formats: md, html, json
#formats = [{FORMATS}]
# 成功后保留下载的视频 / Keep downloaded video after success
#keep_video = {KEEP_VIDEO}
# 继续上次中断的识别 / Continue interrupted transcription
#resume = {RESUME}

[asr]
# HTTP dialect: transcriptions, chat, xai, meta
#mode = "transcriptions"
# Service root or full endpoint; ignored when command is set
#base_url = ""
# Optional; CLI also reads NOTEPIPE_ASR_API_KEY
#api_key = ""
#model = ""
# Empty language means automatic detection
#language = ""
# Parallel requests, clamped to 1..=16
#concurrency = {ASR_CONCURRENCY}
# Managed adapter argv; notepipe adds --host and --port
#command = []
#max_speech = {MAX_SPEECH}
#vad_threshold = {VAD_THRESHOLD}
#min_silence = {MIN_SILENCE}

[llm]
# 可选 AI 校对；运行 notepipe llm setup 配置 / Optional AI proofreading; configure with notepipe llm setup
enabled = {LLM_ENABLED}
#base_url = "{LLM_BASE_URL}"
#api_key = ""
#model = "{LLM_MODEL}"
# 自定义校对要求 / Custom proofreading instructions
#prompt = ""
# 隐藏完成后的提示 / Hide the completion tip
#disable_hint = {LLM_DISABLE_HINT}
# 附截图辅助校对，需要视觉模型 / Attach slides; requires an image-capable model
#vision = {LLM_VISION}
# 并发请求数 / Concurrent requests
#concurrency = {LLM_CONCURRENCY}
# 在笔记中加入 AI 摘要，需要 enabled = true / Add AI summaries; requires enabled = true
#summarize = {LLM_SUMMARIZE}
# 双语笔记的目标语言（zh-Hans、zh-Hant、en、ja、ko、fr、de、es）；已是该语言时不翻译
# Bilingual notes: target language; skipped when the text is already in it
#translate = "en"
"#;

/// 打印配置文件与内置默认合并后的设置（不含本次命令行与环境变量覆盖）。
/// 默认值经 [`crate::options::resolve`] 得出，与实际转换使用同一套解析。
pub fn print_effective(cfg: &ConfigFile) -> Result<()> {
    let d = &cfg.defaults;
    let r = crate::options::resolve(String::new(), &crate::cli::RunOpts::default(), cfg)?;
    println!("配置文件 / Configuration: {}", config_path().display());
    crate::println_tr!(
        "文件设置与内置默认值；不含本次命令行或环境变量覆盖 / File settings and built-in defaults, before CLI and environment overrides"
    );
    let rows: Vec<(&str, String)> = vec![
        ("out", r.out_root.display().to_string()),
        (
            "cookies",
            r.cookies
                .as_ref()
                .map_or_else(|| "-".into(), |path| path.display().to_string()),
        ),
        (
            "cookies_from_browser",
            r.cookies_from_browser.clone().unwrap_or_else(|| "-".into()),
        ),
        ("similarity", r.similarity.to_string()),
        ("sample_interval", r.sample_interval.to_string()),
        ("cooldown", r.cooldown.to_string()),
        ("max_height", r.max_height.to_string()),
        ("roi", d.roi.clone().unwrap_or_else(|| "-".into())),
        ("slide_mode", r.slide_mode.to_string()),
        ("stable_secs", r.stable_secs.to_string()),
        ("transcript_source", r.transcript_source.to_string()),
        (
            "formats",
            r.formats
                .iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(","),
        ),
        ("keep_video", r.keep_video.to_string()),
        ("no_download", r.no_download.to_string()),
        ("resume", r.resume.to_string()),
    ];
    println!("[defaults]");
    for (key, value) in rows {
        println!("  {key:<17}: {value}");
    }
    let hidden = |key: &str| {
        if key.is_empty() {
            "-"
        } else {
            crate::tr!("(已设置，隐藏 / set, hidden)")
        }
    };
    println!("[asr]");
    println!("  {:<17}: {}", "base_url", cfg.asr.base_url);
    println!("  {:<17}: {}", "model", cfg.asr.model);
    println!("  {:<17}: {}", "mode", cfg.asr.mode);
    println!("  {:<17}: {}", "api_key", hidden(&cfg.asr.api_key));
    println!("  {:<17}: {}", "language", r.asr.language);
    println!("  {:<17}: {}", "concurrency", r.asr.concurrency);
    println!("  {:<17}: {:?}", "command", r.asr.command);
    println!("  {:<17}: {}", "max_speech", r.asr.max_speech);
    println!("  {:<17}: {}", "vad_threshold", r.asr.vad_threshold);
    println!("  {:<17}: {}", "min_silence", r.asr.min_silence);
    println!("[llm]");
    println!("  {:<17}: {}", "enabled", cfg.llm.enabled);
    let model = if cfg.llm.model.is_empty() {
        "-"
    } else {
        &cfg.llm.model
    };
    println!("  {:<17}: {model}", "model");
    println!("  {:<17}: {}", "vision", cfg.llm.vision);
    println!("  {:<17}: {}", "concurrency", cfg.llm.concurrency);
    println!("  {:<17}: {}", "summarize", cfg.llm.summarize);
    println!(
        "  {:<17}: {}",
        "translate",
        cfg.llm.translate.as_deref().unwrap_or("-")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn template_parses_to_built_in_defaults_when_uncommented() {
        let text: String = super::template()
            .lines()
            .map(|line| {
                line.strip_prefix('#')
                    .filter(|l| {
                        l.starts_with(|c: char| c.is_ascii_lowercase()) && l.contains(" = ")
                    })
                    .unwrap_or(line)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let file: super::ConfigFile = toml::from_str(&text).unwrap();
        let from_template =
            crate::options::resolve(String::new(), &Default::default(), &file).unwrap();
        let built_in =
            crate::options::resolve(String::new(), &Default::default(), &Default::default())
                .unwrap();
        assert_eq!(from_template.resume, built_in.resume);
        assert_eq!(from_template.stable_secs, built_in.stable_secs);
        assert_eq!(from_template.similarity, built_in.similarity);
        assert_eq!(from_template.cooldown, built_in.cooldown);
        assert_eq!(from_template.asr, built_in.asr);
        assert_eq!(from_template.formats, built_in.formats);
        assert_eq!(file.asr, super::Asr::default());
    }
}
