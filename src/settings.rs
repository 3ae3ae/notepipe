//! 配置文件（~/.config/notepipe/config.toml）。
//! 优先级：命令行参数 > 配置文件 > 内置默认值。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// `[defaults]`：命令行参数的默认值。全部可选，未设置的项回落内置默认。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    pub out: Option<PathBuf>,
    pub similarity: Option<f64>,
    pub sample_interval: Option<f64>,
    pub cooldown: Option<f64>,
    pub screenshot_mode: Option<crate::config::ScreenshotMode>,
    pub stable_secs: Option<f64>,
    pub roi: Option<String>,
    /// 转写来源：auto（字幕优先）| subtitle | asr
    pub transcript_source: Option<crate::config::TranscriptSource>,
}

/// External program that resolves an online URL into local media and subtitles.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Source {
    pub command: Vec<String>,
}

/// HTTP transcription dialect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AsrMode {
    #[default]
    Transcriptions,
    Chat,
}

impl std::fmt::Display for AsrMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Transcriptions => "transcriptions",
            Self::Chat => "chat",
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
    pub source: Source,
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
    let s = std::fs::read_to_string(&p)
        .with_context(|| format!("Cannot read configuration: {}", p.display()))?;
    parse(&s).with_context(|| format!("Fix the configuration syntax and retry: {}", p.display()))
}

/// Explicit config replaces the default file; '-' reads stdin.
pub fn load_from(path: Option<&std::path::Path>) -> Result<ConfigFile> {
    let Some(path) = path else {
        return load();
    };
    let text = if path == std::path::Path::new("-") {
        use std::io::Read;
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        text
    } else {
        std::fs::read_to_string(path)
            .with_context(|| format!("Cannot read configuration: {}", path.display()))?
    };
    parse(&text)
}

fn parse(text: &str) -> Result<ConfigFile> {
    // Parse errors can quote credentials from the input; report only the format.
    if text.trim_start().starts_with('{') {
        serde_json::from_str(text).map_err(|_| anyhow::anyhow!("Invalid JSON configuration"))
    } else {
        toml::from_str(text).map_err(|_| anyhow::anyhow!("Invalid TOML configuration"))
    }
}

pub fn save(cfg: &ConfigFile, path: Option<&Path>) -> Result<PathBuf> {
    let p = path.map(Path::to_path_buf).unwrap_or_else(config_path);
    anyhow::ensure!(p != Path::new("-"), "Cannot save configuration to stdin");
    let p = match p.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !p.is_symlink() => p,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Cannot resolve configuration: {}", p.display()));
        }
    };
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let previous = if p.is_file() {
        Some(std::fs::read(&p)?)
    } else {
        None
    };
    // 覆盖前备份旧配置（用户可能有手改内容），失败只告警不阻断
    if let Some(bytes) = &previous {
        let mut bak = p.as_os_str().to_os_string();
        bak.push(".bak");
        let bak = PathBuf::from(bak);
        if let Err(e) = crate::checkpoint::atomic_write(&bak, bytes) {
            tracing::warn!("Could not back up configuration to {}: {e}", bak.display());
        }
    }
    let bytes = if previous
        .as_ref()
        .is_some_and(|bytes| bytes.trim_ascii_start().starts_with(b"{"))
    {
        serde_json::to_vec_pretty(cfg)?
    } else {
        toml::to_string_pretty(cfg)?.into_bytes()
    };
    crate::checkpoint::atomic_write(&p, &bytes)?;
    Ok(p)
}

/// `config init` 写入的带注释模板。默认值占位符由内置默认渲染，注释不会与实际默认脱节。
pub fn template() -> String {
    use crate::config as c;
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
        ("{STABLE_SECS}", format!("{:?}", c::DEFAULT_STABLE_SECS)),
        ("{MAX_SPEECH}", format!("{:?}", asr.max_speech)),
        ("{ASR_CONCURRENCY}", asr.concurrency.to_string()),
        ("{VAD_THRESHOLD}", asr.vad_threshold.to_string()),
        ("{MIN_SILENCE}", asr.min_silence.to_string()),
        (
            "{SCREENSHOT_MODE}",
            c::ScreenshotMode::default().to_string(),
        ),
        (
            "{TRANSCRIPT_SOURCE}",
            c::TranscriptSource::default().to_string(),
        ),
        ("{LLM_BASE_URL}", llm.base_url),
        ("{LLM_MODEL}", llm.model),
        ("{LLM_CONCURRENCY}", llm.concurrency.to_string()),
    ]
    .into_iter()
    .fold(TEMPLATE.to_owned(), |text, (key, value)| {
        text.replace(key, &value)
    })
}

const TEMPLATE: &str = r#"# Configuration
# Priority: CLI > this file > built-in defaults
# Uncomment values to set defaults
# View settings: notepipe config show

[defaults]
# Output root; each note gets its own title folder
#out = "{OUT}"
# Similarity (0,1]; higher captures more screenshots
#similarity = {SIMILARITY}
# Frame sampling interval (seconds)
#sample_interval = {SAMPLE_INTERVAL}
# Minimum interval between screenshots (seconds)
#cooldown = {COOLDOWN}
# first: immediate; stable: wait for animations
#screenshot_mode = "{SCREENSHOT_MODE}"
#stable_secs = {STABLE_SECS}
# Compare only this region
#roi = "40%,0%-100%,100%"
# auto: subtitles first, then speech recognition
# subtitle: subtitles only; asr: speech recognition only
#transcript_source = "{TRANSCRIPT_SOURCE}"

[source]
# Source adapter argv; empty discovers a single notepipe-source-* executable
#command = []

[asr]
# HTTP dialect: transcriptions, chat
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
# Connection defaults for all stages
#base_url = "{LLM_BASE_URL}"
# Optional; CLI also reads NOTEPIPE_LLM_API_KEY
#api_key = ""
#model = "{LLM_MODEL}"
#concurrency = {LLM_CONCURRENCY}
# Extra request fields; model, messages, stream and response_format are reserved
#request = {}
# No stages run by default; --stages proofread,summary selects built-ins
#stages = []

# Example: enable a built-in stage using its default prompt
# [[llm.stages]]
# name = "proofread"
# images = false
# Per-stage overrides: prompt, base_url, api_key, model, request
# Custom stages also require input (paragraphs/transcript), output (replace/below/top), and prompt

"#;

/// 打印配置文件与内置默认合并后的设置（不含本次命令行与环境变量覆盖）。
/// 默认值经 [`crate::options::resolve`] 得出，与实际转换使用同一套解析。
pub fn print_effective(cfg: &ConfigFile) -> Result<()> {
    let d = &cfg.defaults;
    let r = crate::options::resolve(String::new(), &crate::cli::RunOpts::default(), cfg)?;
    println!("Configuration: {}", config_path().display());
    println!("File settings and built-in defaults, before CLI and environment overrides");
    let rows: Vec<(&str, String)> = vec![
        ("out", r.out_root.display().to_string()),
        ("similarity", r.similarity.to_string()),
        ("sample_interval", r.sample_interval.to_string()),
        ("cooldown", r.cooldown.to_string()),
        ("roi", d.roi.clone().unwrap_or_else(|| "-".into())),
        ("screenshot_mode", r.screenshot_mode.to_string()),
        ("stable_secs", r.stable_secs.to_string()),
        ("transcript_source", r.transcript_source.to_string()),
    ];
    println!("[defaults]");
    for (key, value) in rows {
        println!("  {key:<17}: {value}");
    }
    let hidden = |key: &str| {
        if key.is_empty() { "-" } else { "set, hidden)" }
    };
    println!("[source]");
    println!("  {:<17}: {:?}", "command", r.source.command);
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
    println!("  {:<17}: {}", "base_url", cfg.llm.base_url);
    println!("  {:<17}: {}", "api_key", hidden(&cfg.llm.api_key));
    println!("  {:<17}: {}", "model", cfg.llm.model);
    println!("  {:<17}: {}", "concurrency", cfg.llm.concurrency);
    println!(
        "  {:<17}: {:?}",
        "request keys",
        cfg.llm.request.keys().collect::<Vec<_>>()
    );
    for stage in &cfg.llm.stages {
        let effective = cfg.llm.for_stage(stage)?;
        println!("[[llm.stages]]");
        println!("  {:<17}: {}", "name", stage.name);
        println!("  {:<17}: {:?}", "input", stage.input);
        println!("  {:<17}: {:?}", "output", stage.output);
        println!("  {:<17}: {}", "images", stage.images);
        println!("  {:<17}: {}", "base_url", effective.base_url);
        println!("  {:<17}: {}", "api_key", hidden(&effective.api_key));
        println!("  {:<17}: {}", "model", effective.model);
        println!(
            "  {:<17}: {:?}",
            "request keys",
            effective.request.keys().collect::<Vec<_>>()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn saving_a_symlinked_configuration_updates_and_backs_up_its_target() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("dotfiles/settings.toml");
        let link = root.path().join("config.toml");
        std::fs::create_dir(target.parent().unwrap()).unwrap();
        let original = "[defaults]\nout = 'my-notes'\n";
        std::fs::write(&target, original).unwrap();
        std::os::unix::fs::symlink("dotfiles/settings.toml", &link).unwrap();
        let mut cfg = super::load_from(Some(&link)).unwrap();
        cfg.asr.base_url = "https://example.test/v1".into();
        assert_eq!(
            super::save(&cfg, Some(&link)).unwrap(),
            target.canonicalize().unwrap()
        );
        assert!(link.symlink_metadata().unwrap().is_symlink());
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            std::path::PathBuf::from("dotfiles/settings.toml")
        );
        assert_eq!(super::load_from(Some(&link)).unwrap(), cfg);
        assert_eq!(
            std::fs::read_to_string(target.with_extension("toml.bak")).unwrap(),
            original
        );
        assert!(!root.path().join("config.toml.bak").exists());
        std::fs::remove_file(&target).unwrap();
        assert!(super::save(&cfg, Some(&link)).is_err());
        assert!(link.symlink_metadata().unwrap().is_symlink());
    }

    #[test]
    fn vendor_asr_modes_are_rejected() {
        for mode in ["xai", "meta"] {
            assert!(
                toml::from_str::<super::ConfigFile>(&format!("[asr]\nmode = '{mode}'")).is_err()
            );
            assert!(
                <crate::cli::Cli as clap::Parser>::try_parse_from([
                    "notepipe",
                    "video",
                    "--asr-mode",
                    mode,
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn removed_llm_fields_are_rejected() {
        for setting in [
            "provider = 'codex'",
            "disable_hint = true",
            "enabled = true",
            "summarize = true",
            "vision = true",
            "translate = 'ko'",
            "prompt = 'old root prompt'",
        ] {
            assert!(toml::from_str::<super::ConfigFile>(&format!("[llm]\n{setting}")).is_err());
        }
    }

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
        assert_eq!(from_template.stable_secs, built_in.stable_secs);
        assert_eq!(from_template.similarity, built_in.similarity);
        assert_eq!(from_template.cooldown, built_in.cooldown);
        assert_eq!(from_template.source, built_in.source);
        assert_eq!(from_template.asr, built_in.asr);
        assert_eq!(file.asr, super::Asr::default());
    }
}
