use anyhow::{Context, Result as AnyhowResult};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

// —— 内置默认值（唯一来源）：options.rs 的合并与 settings.rs 的展示统一引用这里 ——
/// SSIM 画面相似度阈值（越高越敏感、截图越多）
pub const DEFAULT_SIMILARITY: f64 = 0.85;
/// 画面采样间隔（秒）
pub const DEFAULT_SAMPLE_INTERVAL: f64 = 1.0;
/// 两张截图之间的最小间隔（秒）
pub const DEFAULT_COOLDOWN: f64 = 10.0;
/// 下载视频的高度上限（讲义截图质量优先）
pub const DEFAULT_MAX_HEIGHT: u32 = 1080;
/// stable 模式下画面需保持不变的秒数
pub const DEFAULT_STABLE_SECS: f64 = 0.8;
/// 单段语音最长秒数（过长会在静音点切分）
pub const DEFAULT_MAX_SPEECH: f32 = 20.0;
/// 默认输出根目录
pub const DEFAULT_OUT_DIR: &str = "out";
/// 校验服务地址为完整的 HTTP(S) URL（llm base_url 与 asr 共用同一判定）。
pub fn ensure_http_url(raw: &str) -> AnyhowResult<url::Url> {
    let url = url::Url::parse(raw.trim()).context(crate::tr!(
        "Invalid service URL; use a complete HTTP(S) URL."
    ))?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
        crate::tr!("Invalid service URL; use a complete HTTP(S) URL.")
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
        crate::settings::AsrMode::Xai => "/stt",
        crate::settings::AsrMode::Meta => "/asr/transcribe",
    };
    anyhow::ensure!(
        !conflicting_endpoint_suffix(
            base,
            suffix,
            &[
                "/audio/transcriptions",
                "/chat/completions",
                "/stt",
                "/asr/transcribe"
            ]
        ),
        crate::tr!("Service endpoint and protocol do not match")
    );
    Ok(append_endpoint_suffix(base, suffix))
}

/// 云端 STT API key 环境变量：`NOTEPIPE_ASR_API_KEY`。
pub fn asr_api_key_from_env() -> Option<String> {
    std::env::var("NOTEPIPE_ASR_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
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

/// 输出格式。非法值在配置解析期即报错，不再拖到渲染阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Md,
    Html,
    Json,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Md => "md",
            Self::Html => "html",
            Self::Json => "json",
        })
    }
}

/// 运行期管线配置（由 CLI 参数归一而来）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    pub url: String,
    pub cookies: Option<PathBuf>,
    pub cookies_from_browser: Option<String>,
    pub out_dir: PathBuf,
    pub similarity: f64,
    pub sample_interval: f64,
    pub cooldown: f64,
    /// 下载视频的高度上限（默认 1080；讲义截图质量优先）
    pub max_height: u32,
    /// 截图发射模式：first（首个不同帧）| stable（等待画面稳定，适合动画课件）
    pub slide_mode: SlideMode,
    /// stable 模式下画面需保持不变的秒数
    pub stable_secs: f64,
    pub roi: Option<Roi>,
    pub formats: Vec<OutputFormat>,
    pub keep_video: bool,
    pub no_download: bool,
    /// 断点续跑：跳过输出目录中已完成的 ASR chunk
    pub resume: bool,
    /// LLM 字幕校对（已合并 CLI 覆盖后的生效配置）
    pub llm: crate::llm::LlmSettings,
    /// HTTP transcription settings with CLI overrides applied.
    pub asr: crate::settings::Asr,
    /// 转写来源：字幕优先 / 强制字幕 / 强制 ASR
    pub transcript_source: TranscriptSource,
    /// `-o` 根目录（笔记库根），课程目录由 workspace/artifact 在其下布局
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
        crate::tr!("ROI percentages must be between 0 and 100")
    );
    let v = percent / 100.0;
    anyhow::ensure!(
        v.is_finite() && v >= 0.0,
        crate::tr!("ROI coordinates must be finite, non-negative numbers")
    );
    Ok(v)
}

impl PipelineConfig {
    /// 身份文件用的脱敏副本：clone 并清空 llm.api_key 与 asr.api_key。
    ///
    /// 安全不变量：身份/binding JSON 会落盘进输出目录，任何情况下不得携带
    /// 凭据——两处手写清密钥（pipeline.rs / execution.rs）已合并为这一单一事实源，
    /// 新增身份序列化路径必须走这里而不是再写一遍。
    pub fn redacted_for_identity(&self) -> Self {
        let mut cfg = self.clone();
        cfg.llm.api_key.clear();
        cfg.asr.api_key.clear();
        cfg
    }

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
            crate::tr!("asr.vad_threshold must be between 0 and 1.")
        );
        anyhow::ensure!(
            self.asr.min_silence >= 0.0,
            crate::tr!("asr.min_silence must be non-negative.")
        );
        // 上限防御 split_smart 的 clamp panic（0 会让 clamp(min>max) abort）
        anyhow::ensure!(
            self.asr.max_speech >= 1.0 && self.asr.max_speech <= 600.0,
            "--max-speech must be between 1 and 600 seconds (received {})",
            self.asr.max_speech
        );
        anyhow::ensure!(
            self.max_height >= 144,
            "--max-height must be at least 144 (received {})",
            self.max_height
        );

        Ok(())
    }

    pub fn validate_asr(&self) -> AnyhowResult<()> {
        anyhow::ensure!(
            self.asr.is_configured(),
            crate::tr!(
                "Configure [asr] with a base_url or command; run `notepipe config init` to create the template."
            )
        );
        if self.asr.command.is_empty() {
            ensure_http_url(&asr_endpoint(&self.asr)?)?;
        } else {
            anyhow::ensure!(
                !self.asr.command[0].trim().is_empty(),
                crate::tr!("The ASR adapter command must name an executable.")
            );
        }
        Ok(())
    }

    pub fn media_path(&self) -> PathBuf {
        self.out_dir.join("media.mp4")
    }
    pub fn audio_path(&self) -> PathBuf {
        self.out_dir.join("audio.wav")
    }
    pub fn frames_dir(&self) -> PathBuf {
        self.out_dir.join("frames")
    }
    pub fn timeline_path(&self) -> PathBuf {
        self.out_dir.join("timeline.jsonl")
    }
    pub fn meta_path(&self) -> PathBuf {
        self.out_dir.join("meta.json")
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

/// 已知视频站点（含子域名与短链），输入可以不带 http(s) 前缀。
pub fn known_video_site(input: &str) -> Option<&'static str> {
    let input = input.trim();
    let url = url::Url::parse(input)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .or_else(|| url::Url::parse(&format!("https://{input}")).ok())?;
    let host = url.host_str()?;
    let under = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    if under("youtube.com") || host == "youtu.be" {
        Some("youtube")
    } else {
        None
    }
}

/// 像 URL、已知视频站点或已存在的本地文件才当作输入。
pub fn looks_like_source(s: &str) -> bool {
    let t = s.trim();
    Path::new(s).is_file()
        || t.starts_with("http://")
        || t.starts_with("https://")
        || known_video_site(t).is_some()
}

pub fn platform_from(source: &str, extractor: &str) -> String {
    let e = extractor.to_ascii_lowercase();
    if Path::new(source).is_file() || e == "local" {
        "local".into()
    } else if e.contains("youtube") || known_video_site(source) == Some("youtube") {
        "youtube".into()
    } else if !e.is_empty() {
        sanitize_component(&e)
    } else {
        "web".into()
    }
}

pub fn infer_slug(source: &str) -> String {
    let p = Path::new(source);
    if p.is_file() {
        return sanitize_component(p.file_stem().and_then(|s| s.to_str()).unwrap_or("local"));
    }
    if let Some(id) = youtube_id(source) {
        return id;
    }
    sanitize_component(source)
}

fn youtube_id(s: &str) -> Option<String> {
    let url = url::Url::parse(s)
        .or_else(|_| url::Url::parse(&format!("https://{}", s.trim())))
        .ok()?;
    if known_video_site(url.as_str()) != Some("youtube") {
        return None;
    }
    let id = if url.host_str()?.eq_ignore_ascii_case("youtu.be") {
        url.path_segments()?.next()?.to_owned()
    } else {
        url.query_pairs()
            .find(|(key, _)| key == "v")
            .map(|(_, value)| value.into_owned())?
    };
    (id.len() >= 6
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    .then_some(id)
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
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
    }
}

/// 保留中文等标题字符，去掉路径非法符（= sanitize_filename_with_fallback(_, "untitled")）。
pub fn sanitize_component(s: &str) -> String {
    sanitize_filename_with_fallback(s, "untitled")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_gate() {
        assert!(!looks_like_source("notepipe"));
        assert!(looks_like_source("https://example.invalid/watch/video"));
        assert!(looks_like_source("https://youtu.be/dQw4w9WgXcQ"));
    }

    #[test]
    fn slug_from_source() {
        assert_eq!(
            infer_slug("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            "dQw4w9WgXcQ"
        );
        assert_eq!(infer_slug("https://youtu.be/dQw4w9WgXcQ"), "dQw4w9WgXcQ");
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
            url: "https://youtu.be/x".into(),
            cookies: None,
            cookies_from_browser: None,
            out_dir: "out".into(),
            out_root: "out".into(),
            similarity: DEFAULT_SIMILARITY,
            sample_interval: DEFAULT_SAMPLE_INTERVAL,
            cooldown: DEFAULT_COOLDOWN,
            max_height: DEFAULT_MAX_HEIGHT,
            slide_mode: SlideMode::Stable,
            stable_secs: DEFAULT_STABLE_SECS,
            roi: None,
            formats: vec![OutputFormat::Md],
            keep_video: false,
            no_download: false,
            resume: false,
            llm: Default::default(),
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

        c.formats = vec![];
        assert!(
            c.validate().is_ok(),
            "Internal reading does not depend on optional exports"
        );
    }

    #[test]
    fn known_video_site_covers_short_links_and_rejects_lookalikes() {
        assert_eq!(known_video_site("https://youtu.be/x"), Some("youtube"));
        assert_eq!(known_video_site("https://example.invalid.evil.test/"), None);
        assert_eq!(
            known_video_site("https://youtube.com/?example.invalid"),
            Some("youtube")
        );
        assert_eq!(known_video_site("file:///example.invalid"), None);
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
            (AsrMode::Xai, "/stt"),
            (AsrMode::Meta, "/asr/transcribe"),
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
                    "/stt"
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
