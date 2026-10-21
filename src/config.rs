use anyhow::{Context, Result as AnyhowResult};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

// —— 内置默认值（唯一来源）：options.rs 的合并与 settings.rs 的展示统一引用这里 ——
/// SSIM 画面相似度阈值（越高越敏感、截图越多）
pub const DEFAULT_SIMILARITY: f64 = 0.85;
/// 画面采样间隔（秒）
pub const DEFAULT_SAMPLE_INTERVAL: f64 = 1.0;
/// 两张截图之间的最小间隔（秒）
pub const DEFAULT_COOLDOWN: f64 = 10.0;
/// stable 模式下画面需保持不变的秒数
pub const DEFAULT_STABLE_SECS: f64 = 0.8;
/// 单段语音最长秒数（过长会在静音点切分）
pub const DEFAULT_MAX_SPEECH: f32 = 20.0;
/// 默认输出根目录
pub const DEFAULT_OUT_DIR: &str = ".";
/// 校验服务地址为完整的 HTTP(S) URL（llm base_url 与 asr 共用同一判定）。
pub fn ensure_http_url(raw: &str) -> AnyhowResult<url::Url> {
    let url =
        url::Url::parse(raw.trim()).context("Invalid service URL; use a complete HTTP(S) URL.")?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
        "Invalid service URL; use a complete HTTP(S) URL."
    );
    Ok(url)
}

/// Append `suffix` unless the path already ends with it — never double-append.
/// 端点后缀策略的唯一事实源（ASR 与 LLM 共用）。
pub fn append_endpoint_suffix(base: &str, suffix: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with(suffix) {
        base.to_string()
    } else {
        format!("{base}{suffix}")
    }
}

/// `path` ends with a known endpoint suffix that conflicts with the desired one.
/// 调用方应拒绝而不是静默改路由到另一个协议（凭据可能因此发到错误端点）。
pub fn conflicting_endpoint_suffix(path: &str, desired: &str, known: &[&str]) -> bool {
    let path = path.trim_end_matches('/');
    !path.ends_with(desired) && known.iter().any(|k| path.ends_with(k))
}

/// Accept a service root or its exact supported endpoint, never double-append paths.
pub fn asr_endpoint(api: &crate::settings::Asr) -> AnyhowResult<String> {
    let base = api.base_url.trim().trim_end_matches('/');
    let suffix = match api.mode {
        crate::settings::AsrMode::Transcriptions => "/audio/transcriptions",
        crate::settings::AsrMode::Chat => "/chat/completions",
    };
    anyhow::ensure!(
        !conflicting_endpoint_suffix(
            base,
            suffix,
            &["/audio/transcriptions", "/chat/completions"]
        ),
        "Service endpoint and protocol do not match"
    );
    Ok(append_endpoint_suffix(base, suffix))
}

/// 云端 STT API key 环境变量：`NOTEPIPE_ASR_API_KEY`。
pub fn asr_api_key_from_env() -> Option<String> {
    std::env::var("NOTEPIPE_ASR_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
}

/// CLI fallback for an empty LLM key.
pub fn llm_api_key_from_env() -> Option<String> {
    std::env::var("NOTEPIPE_LLM_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
}

/// 截图发射模式：first（首个不同帧）| stable（等待画面稳定，适合动画课件）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlideMode {
    First,
    #[default]
    Stable,
}

impl fmt::Display for TranscriptSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Auto => "auto",
            Self::Subtitle => "subtitle",
            Self::Asr => "asr",
        })
    }
}

impl fmt::Display for SlideMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::First => "first",
            Self::Stable => "stable",
        })
    }
}

/// 转写来源策略。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptSource {
    /// 优先字幕，无字幕时识别语音 / Prefer subtitles; transcribe speech if unavailable
    #[default]
    Auto,
    /// 仅字幕；本地需同名 .srt/.vtt / Subtitles only; local files need a matching .srt/.vtt
    Subtitle,
    /// 使用所选后端识别，不查字幕 / Transcribe with the selected backend; skip subtitles
    Asr,
}

/// 运行期管线配置（由 CLI 参数归一而来）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    pub url: String,
    pub source: crate::settings::Source,
    pub out_dir: PathBuf,
    pub similarity: f64,
    pub sample_interval: f64,
    pub cooldown: f64,
    /// 截图发射模式：first（首个不同帧）| stable（等待画面稳定，适合动画课件）
    pub slide_mode: SlideMode,
    /// stable 模式下画面需保持不变的秒数
    pub stable_secs: f64,
    pub roi: Option<Roi>,
    /// Clear this source's cache before starting.
    pub no_resume: bool,
    pub subtitle: Option<PathBuf>,
    /// LLM 字幕校对（已合并 CLI 覆盖后的生效配置）
    pub llm: crate::llm::LlmSettings,
    /// HTTP transcription settings with CLI overrides applied.
    pub asr: crate::settings::Asr,
    /// 转写来源：字幕优先 / 强制字幕 / 强制 ASR
    pub transcript_source: TranscriptSource,
    /// Root containing published note folders.
    pub out_root: PathBuf,
}

/// 感兴趣区域；坐标以百分比表示（0–100%）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Roi {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

impl Roi {
    /// 解析 "x1,y1-x2,y2"，坐标可带 `%` 或为像素。
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let (a, b) = s
            .split_once('-')
            .ok_or_else(|| anyhow::anyhow!("ROI must use x1,y1-x2,y2: {s:?}"))?;
        let (x1, y1) = parse_xy(a)?;
        let (x2, y2) = parse_xy(b)?;
        let (x1, x2) = (x1.min(x2), x1.max(x2));
        let (y1, y2) = (y1.min(y2), y1.max(y2));
        if x2 <= x1 || y2 <= y1 {
            anyhow::bail!("ROI must have a positive width and height: {s:?}");
        }
        Ok(Self { x1, y1, x2, y2 })
    }

    /// 按帧尺寸换算为像素矩形。
    pub fn pixels(&self, w: u32, h: u32) -> (u32, u32, u32, u32) {
        let sx = |v: f64| (v * w as f64).round() as u32;
        let sy = |v: f64| (v * h as f64).round() as u32;
        let (x1, x2) = (
            sx(self.x1).min(w.saturating_sub(1)),
            sx(self.x2).clamp(1, w),
        );
        let (y1, y2) = (
            sy(self.y1).min(h.saturating_sub(1)),
            sy(self.y2).clamp(1, h),
        );
        (x1, y1, x2.max(x1 + 1), y2.max(y1 + 1))
    }
}

fn parse_xy(pair: &str) -> anyhow::Result<(f64, f64)> {
    let (a, b) = pair
        .split_once(',')
        .ok_or_else(|| anyhow::anyhow!("ROI coordinates must use x,y: {pair:?}"))?;
    Ok((parse_coord(a)?, parse_coord(b)?))
}

fn parse_coord(s: &str) -> anyhow::Result<f64> {
    let s = s.trim();
    let p = s
        .strip_suffix('%')
        .ok_or_else(|| anyhow::anyhow!("ROI coordinates must use percentages: {s:?}"))?;
    let percent: f64 = p.trim().parse()?;
    anyhow::ensure!(
        (0.0..=100.0).contains(&percent),
        "ROI percentages must be between 0 and 100"
    );
    let v = percent / 100.0;
    anyhow::ensure!(
        v.is_finite() && v >= 0.0,
        "ROI coordinates must be finite, non-negative numbers"
    );
    Ok(v)
}

impl PipelineConfig {
    /// 全量预检：所有配置错误必须在任何昂贵操作（下载/抽帧/模型加载）之前暴露。
    /// 返回 Err 的送进 main 后直接退出，不会碰网络与模型。
    pub fn validate(&self) -> AnyhowResult<()> {
        if let Some(target) = &self.llm.translate {
            crate::language::target(target).map_err(anyhow::Error::msg)?;
        }
        let finite = |name: &str, v: f64| -> AnyhowResult<()> {
            anyhow::ensure!(
                v.is_finite(),
                "{name} must be a finite number (received {v})"
            );
            Ok(())
        };
        finite("similarity", self.similarity)?;
        finite("sample_interval", self.sample_interval)?;
        finite("cooldown", self.cooldown)?;
        finite("stable_secs", self.stable_secs)?;
        finite("asr.max_speech", self.asr.max_speech as f64)?;
        finite("asr.vad_threshold", self.asr.vad_threshold as f64)?;
        finite("asr.min_silence", self.asr.min_silence as f64)?;

        anyhow::ensure!(
            self.similarity > 0.0 && self.similarity <= 1.0,
            "--similarity must be greater than 0 and at most 1 (received {})",
            self.similarity
        );
        anyhow::ensure!(
            self.sample_interval >= 0.2,
            "--sample-interval must be at least 0.2 seconds (received {})",
            self.sample_interval
        );
        anyhow::ensure!(
            self.cooldown >= 0.0,
            "--cooldown must be at least 0 seconds (received {})",
            self.cooldown
        );
        anyhow::ensure!(
            self.stable_secs >= 0.0,
            "--stable-secs must be at least 0 seconds (received {})",
            self.stable_secs
        );
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.asr.vad_threshold),
            "asr.vad_threshold must be between 0 and 1."
        );
        anyhow::ensure!(
            self.asr.min_silence >= 0.0,
            "asr.min_silence must be non-negative."
        );
        // 上限防御 split_smart 的 clamp panic（0 会让 clamp(min>max) abort）
        anyhow::ensure!(
            self.asr.max_speech >= 1.0 && self.asr.max_speech <= 600.0,
            "--max-speech must be between 1 and 600 seconds (received {})",
            self.asr.max_speech
        );

        Ok(())
    }

    pub fn validate_asr(&self) -> AnyhowResult<()> {
        anyhow::ensure!(
            self.asr.is_configured(),
            "Configure [asr] with a base_url or command; run `notepipe config init` to create the template."
        );
        if self.asr.command.is_empty() {
            ensure_http_url(&asr_endpoint(&self.asr)?)?;
        } else {
            anyhow::ensure!(
                !self.asr.command[0].trim().is_empty(),
                "The ASR adapter command must name an executable."
            );
        }
        Ok(())
    }

    pub fn audio_path(&self) -> PathBuf {
        self.out_dir.join("audio.wav")
    }
    pub fn frames_dir(&self) -> PathBuf {
        self.out_dir.join("frames")
    }
}

/// 用户主目录：HOME，Windows 上 HOME 可能缺席时退回 USERPROFILE。
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// 配置目录（XDG_CONFIG_HOME / Windows %APPDATA% / ~/.config）。
pub fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(d).join("notepipe");
    }
    #[cfg(windows)]
    {
        if let Some(d) = std::env::var_os("APPDATA") {
            return PathBuf::from(d).join("notepipe");
        }
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("notepipe")
}

/// Platform cache root; XDG overrides the native location on every platform.
pub fn cache_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_CACHE_HOME").filter(|p| !p.is_empty()) {
        return PathBuf::from(path).join("notepipe");
    }
    #[cfg(windows)]
    if let Some(path) = std::env::var_os("LOCALAPPDATA").filter(|p| !p.is_empty()) {
        return PathBuf::from(path).join("notepipe/cache");
    }
    let home = home_dir().unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library/Caches/notepipe")
    } else {
        home.join(".cache/notepipe")
    }
}

/// 展开 `~` / `~/...`（仅 Unix 主目录约定；无 HOME 时原样返回）。
/// 防止配置里的 "~/cache" 真的在当前目录创建名为 `~` 的子目录。
pub fn expand_tilde(p: PathBuf) -> PathBuf {
    let Some(s) = p.to_str() else { return p };
    if s == "~" {
        if let Some(h) = home_dir() {
            return h;
        }
    } else if let Some(rest) = s.strip_prefix("~/")
        && let Some(h) = home_dir()
    {
        return h.join(rest);
    }
    p
}

/// Tri-state flag: an explicit off wins over on; otherwise the file value,
/// then the built-in default.
pub fn resolve_flag(cli_on: bool, cli_off: bool, file: Option<bool>, default: bool) -> bool {
    if cli_off {
        false
    } else if cli_on {
        true
    } else {
        file.unwrap_or(default)
    }
}

/// 路径组件的字符数上限（净化后的标题/平台名）：防止超长标题撑出超过
/// 文件系统限制的路径。
const MAX_COMPONENT_CHARS: usize = 80;

/// 统一的文件名净化原语：路径非法符与空白折叠为 `-`，裁剪首尾，超长截断。
/// `fallback` 为净化后为空时的名字（调用方语义：untitled / summary）。
pub fn sanitize_filename_with_fallback(s: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        let bad =
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|');
        if bad || c.is_whitespace() {
            if !prev_dash && !out.is_empty() {
                out.push('-');
                prev_dash = true;
            }
        } else {
            out.push(c);
            prev_dash = false;
        }
    }
    let out = out.trim_matches(['-', '.', ' ']).to_string();
    let out: String = out.chars().take(MAX_COMPONENT_CHARS).collect();
    let out = out.trim_end_matches(['-', '.', ' ']);
    if out.is_empty() {
        fallback.to_string()
    } else {
        let stem = out.split('.').next().unwrap_or(out).to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ["COM", "LPT"].iter().any(|prefix| {
                stem.strip_prefix(prefix).is_some_and(|suffix| {
                    suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9')
                })
            });
        if reserved {
            match out.split_once('.') {
                Some((name, extension)) => format!("{name}_.{extension}"),
                None => format!("{out}_"),
            }
        } else {
            out.to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_names_trim_after_truncation_and_avoid_windows_reserved_names() {
        assert_eq!(sanitize_filename_with_fallback("CON", "untitled"), "CON_");
        assert_eq!(
            sanitize_filename_with_fallback("con.txt", "untitled"),
            "con_.txt"
        );
        assert_eq!(sanitize_filename_with_fallback("LPT9", "untitled"), "LPT9_");
        assert_eq!(
            sanitize_filename_with_fallback("COM10", "untitled"),
            "COM10"
        );
        assert_eq!(
            sanitize_filename_with_fallback(&format!("{} rest", "a".repeat(79)), "untitled"),
            "a".repeat(79)
        );
        assert_eq!(
            sanitize_filename_with_fallback(" ... ", "untitled"),
            "untitled"
        );
    }

    #[test]
    fn expand_tilde_paths() {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            assert_eq!(expand_tilde("~/m".into()), PathBuf::from(&home).join("m"));
            assert_eq!(expand_tilde("~".into()), PathBuf::from(&home));
        }
        assert_eq!(expand_tilde("rel/x".into()), PathBuf::from("rel/x"));
        assert_eq!(expand_tilde("/abs/x".into()), PathBuf::from("/abs/x"));
    }

    fn valid_cfg() -> PipelineConfig {
        PipelineConfig {
            url: "https://example.test/media".into(),
            out_dir: "out".into(),
            out_root: "out".into(),
            similarity: DEFAULT_SIMILARITY,
            sample_interval: DEFAULT_SAMPLE_INTERVAL,
            cooldown: DEFAULT_COOLDOWN,
            slide_mode: SlideMode::Stable,
            stable_secs: DEFAULT_STABLE_SECS,
            roi: None,
            no_resume: false,
            subtitle: None,
            llm: Default::default(),
            source: Default::default(),
            asr: Default::default(),
            transcript_source: TranscriptSource::Auto,
        }
    }

    #[test]
    fn validate_accepts_defaults() {
        valid_cfg().validate().unwrap();
    }

    #[test]
    fn validate_rejects_out_of_range() {
        let mut c = valid_cfg();
        // max_speech=0 曾导致 split_smart 中 clamp(min>max) panic
        c.asr.max_speech = 0.0;
        assert!(c.validate().is_err());
        c.asr.max_speech = f32::INFINITY;
        assert!(c.validate().is_err());
        c.asr.max_speech = DEFAULT_MAX_SPEECH;

        c.similarity = 0.0;
        assert!(c.validate().is_err());
        c.similarity = 1.5;
        assert!(c.validate().is_err());
        c.similarity = DEFAULT_SIMILARITY;

        c.sample_interval = 0.0;
        assert!(c.validate().is_err());
        c.sample_interval = DEFAULT_SAMPLE_INTERVAL;
    }

    #[test]
    fn flag_priority() {
        // Explicit off beats on; then on; then the file; then the default.
        assert!(!resolve_flag(true, true, Some(true), true));
        assert!(!resolve_flag(false, true, Some(true), true));
        assert!(resolve_flag(true, false, Some(false), false));
        assert!(resolve_flag(false, false, Some(true), false));
        assert!(!resolve_flag(false, false, Some(false), true));
        assert!(resolve_flag(false, false, None, true));
    }

    #[test]
    fn roi_parse() {
        let r = Roi::parse("25%,0%-100%,100%").unwrap();
        assert_eq!(r.pixels(1000, 800), (250, 0, 1000, 800));
        assert!(Roi::parse("nonsense").is_err());
    }
}

#[cfg(test)]
mod asr_tests {
    use super::*;
    use crate::settings::{Asr, AsrMode, ConfigFile};

    #[test]
    fn asr_endpoints_accept_roots_and_full_paths_and_reject_conflicts() {
        for (mode, suffix) in [
            (AsrMode::Transcriptions, "/audio/transcriptions"),
            (AsrMode::Chat, "/chat/completions"),
        ] {
            let mut asr = Asr {
                mode,
                base_url: " https://example.test/v1/ ".into(),
                ..Default::default()
            };
            let expected = format!("https://example.test/v1{suffix}");
            assert_eq!(asr_endpoint(&asr).unwrap(), expected);
            asr.base_url = format!("{expected}/");
            assert_eq!(asr_endpoint(&asr).unwrap(), expected);
            asr.base_url = format!(
                "https://example.test/v1{}",
                if mode == AsrMode::Chat {
                    "/audio/transcriptions"
                } else {
                    "/chat/completions"
                }
            );
            assert!(asr_endpoint(&asr).is_err());
        }
    }

    #[test]
    fn asr_configuration_requires_a_destination_but_never_a_model_or_key() {
        let mut cfg =
            crate::options::resolve("video".into(), &Default::default(), &Default::default())
                .unwrap();
        let error = cfg.validate_asr().unwrap_err().to_string();
        assert!(error.contains("[asr]") && error.contains("notepipe config init"));
        cfg.asr.base_url = "http://localhost:8000/v1".into();
        cfg.validate_asr().unwrap();
        cfg.asr.base_url = "invalid ignored URL".into();
        cfg.asr.command = vec!["adapter".into()];
        cfg.validate_asr().unwrap();
        cfg.asr.command[0] = "".into();
        assert!(cfg.validate_asr().is_err());
        for field in ["vad_threshold", "min_silence"] {
            let mut file = ConfigFile::default();
            if field == "vad_threshold" {
                file.asr.vad_threshold = 1.1;
            } else {
                file.asr.min_silence = -0.1;
            }
            let cfg = crate::options::resolve("video".into(), &Default::default(), &file).unwrap();
            assert!(cfg.validate().is_err());
        }
        for text in [
            "[asr_api]",
            "[defaults]\nprovider = 'api'",
            "[defaults]\nmax_speech = 20.0",
            "[defaults]\nthreads = 4",
        ] {
            assert!(toml::from_str::<ConfigFile>(text).is_err(), "{text}");
        }
    }
}
