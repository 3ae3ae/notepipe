use anyhow::{Context, Result as AnyhowResult};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

// —— 内置默认值（唯一来源）：main.rs 的 CLI 合并、settings.rs 的展示统一引用这里 ——
/// 默认本地识别模型（canonical id）。
pub const DEFAULT_ASR_MODEL: &str = "qwen3-1.7b";
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
/// ASR 识别线程数
pub const DEFAULT_THREADS: i32 = 4;
/// 单段语音最长秒数（过长会在静音点切分）
pub const DEFAULT_MAX_SPEECH: f32 = 20.0;
/// 默认输出根目录
pub const DEFAULT_OUT_DIR: &str = "out";
/// GPU 卸载层数默认（llama-server -ngl）：全量卸载。没有哪个非零层数在 AMD/ROCm
/// 核显上被证实稳定，因此不按平台区分默认；Linux+AMD 只做风险警告，由用户手动降载。
pub const DEFAULT_GPU_LAYERS: u32 = 99;

/// 校验服务地址为完整的 HTTP(S) URL（llm base_url 与 asr_api 共用同一判定）。
pub fn ensure_http_url(raw: &str) -> AnyhowResult<url::Url> {
    let url = url::Url::parse(raw.trim()).context(
        "服务地址无效，请使用完整的 HTTP(S) URL。 / Invalid service URL; use a complete HTTP(S) URL.",
    )?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
        "服务地址无效，请使用完整的 HTTP(S) URL。 / Invalid service URL; use a complete HTTP(S) URL."
    );
    Ok(url)
}

/// Append `suffix` unless the path already ends with it — never double-append.
/// 端点后缀策略的唯一事实源（core 的 asr/llm 与 desktop 的服务编辑器共用）。
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
pub fn asr_endpoint(api: &crate::settings::AsrApi) -> AnyhowResult<String> {
    let base = api.base_url.trim().trim_end_matches('/');
    let suffix = match api.mode {
        crate::settings::AsrApiMode::Transcriptions => "/audio/transcriptions",
        crate::settings::AsrApiMode::Chat => "/chat/completions",
    };
    anyhow::ensure!(
        !conflicting_endpoint_suffix(
            base,
            suffix,
            &["/audio/transcriptions", "/chat/completions"]
        ),
        "服务地址与所选接口类型不一致 / Service endpoint and protocol do not match"
    );
    Ok(append_endpoint_suffix(base, suffix))
}

/// 云端 STT API key 环境变量：`COURSE2MD_ASR_API_KEY`。
pub fn asr_api_key_from_env() -> Option<String> {
    std::env::var("COURSE2MD_ASR_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
}

/// ASR 后端。typed enum 取代散落各处的字符串比较（`eq_ignore_ascii_case`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AsrProvider {
    Coreml,
    Gpu,
    Cpu,
    Npu,
    Api,
}

impl AsrProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Coreml => "coreml",
            Self::Gpu => "gpu",
            Self::Cpu => "cpu",
            Self::Npu => "npu",
            Self::Api => "api",
        }
    }
}

impl fmt::Display for AsrProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一个本地识别模型的规范 id 与展示名。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalModel {
    pub id: &'static str,
    pub label: &'static str,
}

const QWEN3_1_7B: LocalModel = LocalModel {
    id: DEFAULT_ASR_MODEL,
    label: "Qwen3 1.7B",
};
const QWEN3_0_6B: LocalModel = LocalModel {
    id: "qwen3-0.6b",
    label: "Qwen3 0.6B",
};
const WHISPER: LocalModel = LocalModel {
    id: "whisper",
    label: "Whisper",
};
const WHISPER_TINY: LocalModel = LocalModel {
    id: "whisper-tiny",
    label: "Whisper Tiny",
};
const WHISPER_BASE: LocalModel = LocalModel {
    id: "whisper-base",
    label: "Whisper Base",
};
const WHISPER_SMALL: LocalModel = LocalModel {
    id: "whisper-small",
    label: "Whisper Small",
};

/// 每种本地后端可选的模型（第一项为默认）。云端后端没有本地模型。
pub fn local_models(provider: AsrProvider) -> &'static [LocalModel] {
    match provider {
        AsrProvider::Gpu | AsrProvider::Cpu => &[QWEN3_1_7B],
        AsrProvider::Coreml => &[QWEN3_1_7B, QWEN3_0_6B, WHISPER],
        AsrProvider::Npu => &[
            QWEN3_1_7B,
            QWEN3_0_6B,
            WHISPER,
            WHISPER_TINY,
            WHISPER_BASE,
            WHISPER_SMALL,
        ],
        AsrProvider::Api => &[],
    }
}

/// 模型展示名；未知 id（如自定义仓库）原样返回。
pub fn model_label(id: &str) -> &str {
    [
        QWEN3_1_7B,
        QWEN3_0_6B,
        WHISPER,
        WHISPER_TINY,
        WHISPER_BASE,
        WHISPER_SMALL,
    ]
    .into_iter()
    .find(|m| m.id == id)
    .map_or(id, |m| m.label)
}

/// 别名 → 规范 id（大小写不敏感）。空值表示默认模型。
fn model_alias(lower: &str) -> Option<&'static str> {
    Some(match lower {
        ""
        | "qwen"
        | "qwen3"
        | "1.7"
        | "1.7b"
        | "qwen3-1.7b"
        | "qwen3-asr-1.7b"
        | "qwen3-1.7b-gguf"
        | "qwen3-asr-1.7b-q8_0.gguf" => QWEN3_1_7B.id,
        "0.6" | "0.6b" | "qwen3-0.6b" | "qwen3-asr-0.6b" => QWEN3_0_6B.id,
        "whisper"
        | "whisper-large-v3-turbo"
        | "whisper-turbo"
        | "whisper-large"
        | "turbo"
        | "large" => WHISPER.id,
        "tiny" | "whisper-tiny" => WHISPER_TINY.id,
        "base" | "whisper-base" => WHISPER_BASE.id,
        "small" | "whisper-small" => WHISPER_SMALL.id,
        _ => return None,
    })
}

/// 模型名的唯一规范化入口：别名 → 规范 id，并确认所选后端支持它。
/// npu 另外接受 Hugging Face 仓库 ID（含 `/`，原样保留大小写）；云端后端不使用本地模型，原样返回。
pub fn canonical_model(provider: AsrProvider, raw: &str) -> AnyhowResult<String> {
    let raw = raw.trim();
    if provider == AsrProvider::Api {
        return Ok(raw.to_owned());
    }
    if provider == AsrProvider::Npu && raw.contains('/') {
        return Ok(raw.to_owned());
    }
    let models = local_models(provider);
    match model_alias(&raw.to_ascii_lowercase()) {
        Some(id) if models.iter().any(|m| m.id == id) => Ok(id.to_owned()),
        _ => {
            let available = models.iter().map(|m| m.id).collect::<Vec<_>>().join(", ");
            let (repo_zh, repo_en) = if provider == AsrProvider::Npu {
                (
                    "，或 Hugging Face 仓库名（如 org/model）",
                    ", or a Hugging Face repository (org/model)",
                )
            } else {
                ("", "")
            };
            anyhow::bail!(
                "所选识别方式不支持模型「{raw}」。可用：{available}{repo_zh} / The {provider} backend does not support model \"{raw}\". Available: {available}{repo_en}"
            )
        }
    }
}

/// 截图发射模式：first（首个不同帧）| stable（等待画面稳定，适合动画课件）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlideMode {
    First,
    #[default]
    Stable,
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
    pub threads: i32,
    pub provider: AsrProvider,
    pub max_speech: f32,
    pub formats: Vec<OutputFormat>,
    pub model_dir: PathBuf,
    pub keep_video: bool,
    pub no_download: bool,
    /// 断点续跑：跳过输出目录中已完成的 ASR chunk
    pub resume: bool,
    /// LLM 字幕润色（已合并 CLI 覆盖后的生效配置）
    pub llm: crate::llm::LlmSettings,
    /// 云端 STT（provider=api；已合并 CLI 覆盖）
    pub asr_api: crate::settings::AsrApi,
    /// 转写来源：字幕优先 / 强制字幕 / 强制 ASR
    pub transcript_source: TranscriptSource,
    /// 本地识别模型（别名或规范 id，见 [`canonical_model`]）；None = 后端默认
    pub asr_model: Option<String>,
    /// GPU 卸载层数（llama-server -ngl；gpu 后端生效，cpu 后端恒为 0）
    pub gpu_layers: u32,
    /// 是否把多模态 projector（mmproj）卸载到 GPU
    pub mmproj_offload: bool,
    /// `-o` 根目录，实际课程目录是 `{out_root}/{platform}/{title}/{id}/`
    pub out_root: PathBuf,
}

/// 感兴趣区域；坐标可为像素或比例（0.0-1.0）。
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
            .ok_or_else(|| anyhow::anyhow!("ROI 格式应为 / ROI must use x1,y1-x2,y2: {s:?}"))?;
        let (x1, y1) = parse_xy(a)?;
        let (x2, y2) = parse_xy(b)?;
        let (x1, x2) = (x1.min(x2), x1.max(x2));
        let (y1, y2) = (y1.min(y2), y1.max(y2));
        if x2 <= x1 || y2 <= y1 {
            anyhow::bail!("ROI 区域不能为空 / ROI must have a positive width and height: {s:?}");
        }
        Ok(Self { x1, y1, x2, y2 })
    }

    /// 按帧尺寸换算为像素矩形。约定：坐标值 ≤1.0 视为比例，>1.0 视为像素。
    pub fn pixels(&self, w: u32, h: u32) -> (u32, u32, u32, u32) {
        let sx = |v: f64| {
            if v <= 1.0 {
                (v * w as f64).round() as u32
            } else {
                (v.round() as u32).min(w)
            }
        };
        let sy = |v: f64| {
            if v <= 1.0 {
                (v * h as f64).round() as u32
            } else {
                (v.round() as u32).min(h)
            }
        };
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
    let (a, b) = pair.split_once(',').ok_or_else(|| {
        anyhow::anyhow!("ROI 坐标格式应为 / ROI coordinates must use x,y: {pair:?}")
    })?;
    Ok((parse_coord(a)?, parse_coord(b)?))
}

fn parse_coord(s: &str) -> anyhow::Result<f64> {
    let s = s.trim();
    let v = if let Some(p) = s.strip_suffix('%') {
        let percent: f64 = p.trim().parse()?;
        anyhow::ensure!(
            (0.0..=100.0).contains(&percent),
            "ROI 百分比必须在 0–100 / ROI percentages must be between 0 and 100"
        );
        percent / 100.0
    } else {
        s.parse::<f64>()?
    };
    anyhow::ensure!(
        v.is_finite() && v >= 0.0,
        "ROI 坐标必须为非负有限数值 / ROI coordinates must be finite, non-negative numbers"
    );
    Ok(v)
}

impl PipelineConfig {
    /// 身份文件用的脱敏副本：clone 并清空 llm.api_key 与 asr_api.api_key。
    ///
    /// 安全不变量：身份/binding JSON 会落盘进输出目录，任何情况下不得携带
    /// 凭据——两处手写清密钥（pipeline.rs / execution.rs）已合并为这一单一事实源，
    /// 新增身份序列化路径必须走这里而不是再写一遍。
    pub fn redacted_for_identity(&self) -> Self {
        let mut cfg = self.clone();
        cfg.llm.api_key.clear();
        cfg.asr_api.api_key.clear();
        cfg
    }

    /// 全量预检：所有配置错误必须在任何昂贵操作（下载/抽帧/模型加载）之前暴露。
    /// 返回 Err 的送进 main 后直接退出，不会碰网络与模型。
    pub fn validate(&self) -> AnyhowResult<()> {
        let finite = |name: &str, v: f64| -> AnyhowResult<()> {
            anyhow::ensure!(
                v.is_finite(),
                "{name} 必须是有限数值 / must be a finite number (received {v})"
            );
            Ok(())
        };
        finite("similarity", self.similarity)?;
        finite("sample_interval", self.sample_interval)?;
        finite("cooldown", self.cooldown)?;
        finite("stable_secs", self.stable_secs)?;
        finite("max_speech", self.max_speech as f64)?;

        anyhow::ensure!(
            self.similarity > 0.0 && self.similarity <= 1.0,
            "--similarity 必须大于 0 且不超过 1 / must be greater than 0 and at most 1 (received {})",
            self.similarity
        );
        anyhow::ensure!(
            self.sample_interval >= 0.2,
            "--sample-interval 必须 >= 0.2 秒 / must be at least 0.2 seconds (received {})",
            self.sample_interval
        );
        anyhow::ensure!(
            self.cooldown >= 0.0,
            "--cooldown 必须 >= 0 秒 / must be at least 0 seconds (received {})",
            self.cooldown
        );
        anyhow::ensure!(
            self.stable_secs >= 0.0,
            "--stable-secs 必须 >= 0 秒 / must be at least 0 seconds (received {})",
            self.stable_secs
        );
        // 上限防御 split_smart 的 clamp panic（0 会让 clamp(min>max) abort）
        anyhow::ensure!(
            self.max_speech >= 1.0 && self.max_speech <= 600.0,
            "--max-speech 必须在 1–600 秒 / must be between 1 and 600 seconds (received {})",
            self.max_speech
        );
        anyhow::ensure!(
            self.threads >= 1,
            "--threads 必须 >= 1 / must be at least 1 (received {})",
            self.threads
        );
        anyhow::ensure!(
            self.max_height >= 144,
            "--max-height 必须 >= 144 / must be at least 144 (received {})",
            self.max_height
        );
        anyhow::ensure!(
            self.gpu_layers <= 99,
            "gpu_layers 必须在 0..=99（收到 {0}）/ gpu_layers must be in 0..=99 (got {0})",
            self.gpu_layers
        );

        Ok(())
    }

    /// 仅在确实需要语音识别时验证后端配置；字幕路径不依赖 ASR。
    pub fn validate_asr(&self) -> AnyhowResult<()> {
        self.validate_asr_with_auth(true, true)
    }

    pub fn validate_asr_with_auth(
        &self,
        require_key: bool,
        allow_environment: bool,
    ) -> AnyhowResult<()> {
        // provider × 模型兼容性：静默忽略用户指定的模型属于静默错误结果
        if let Some(model) = &self.asr_model {
            canonical_model(self.provider, model)?;
        }

        // api 后端：key 缺失时立即报错，而不是切完音频才发现
        if self.provider == AsrProvider::Api {
            ensure_http_url(&asr_endpoint(&self.asr_api)?)?;
            anyhow::ensure!(
                !self.asr_api.model.trim().is_empty(),
                "请输入语音服务提供的模型 ID / Speech model ID is required"
            );
            anyhow::ensure!(
                !require_key
                    || !self.asr_api.api_key.trim().is_empty()
                    || (allow_environment && asr_api_key_from_env().is_some()),
                "provider api 需要 API key：配置文件 [asr_api].api_key、--asr-api-key \
                 或环境变量 COURSE2MD_ASR_API_KEY。/ Cloud transcription requires an API key; set COURSE2MD_ASR_API_KEY or [asr_api].api_key."
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
        return PathBuf::from(d).join("course2md");
    }
    #[cfg(windows)]
    {
        if let Some(d) = std::env::var_os("APPDATA") {
            return PathBuf::from(d).join("course2md");
        }
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("course2md")
}

/// 缓存目录（模型等）。
pub fn cache_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(d).join("course2md");
    }
    #[cfg(windows)]
    {
        if let Some(d) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(d).join("course2md");
        }
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache")
        .join("course2md")
}

pub fn model_dir_from(opt: Option<&Path>) -> PathBuf {
    opt.map(|p| expand_tilde(p.to_path_buf()))
        .unwrap_or_else(|| cache_dir().join("models"))
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

/// resume 三态解析：`--no-resume` > `--resume` > 配置文件 > 默认开启。
/// 两级 CLI flag 互斥（clap conflicts_with），不会同时为 true。
pub fn resolve_resume(cli_resume: bool, cli_no_resume: bool, file_resume: Option<bool>) -> bool {
    if cli_no_resume {
        return false;
    }
    if cli_resume {
        return true;
    }
    file_resume.unwrap_or(true)
}

/// Linux + AMD GPU 检测：任一 /sys/class/drm/card*/device/vendor
/// 内容为 "0x1002"（AMD PCI vendor id）即视为 AMD 显卡机器。
/// 文件不存在（非 Linux、无 DRM 设备）→ false，不影响其他平台。
/// 只用于 provider=gpu 时的 ROCm 风险警告，不参与默认值计算。
pub fn linux_amd_gpu_present() -> bool {
    cfg!(target_os = "linux") && any_amd_drm_card(Path::new("/sys/class/drm"))
}

/// 扫描 `<sys_drm>/card*/device/vendor`，任一内容为 AMD vendor id 即 true。
/// 独立成可注入路径的函数以便单测（/sys 在测试机上不可构造）。
fn any_amd_drm_card(sys_drm: &Path) -> bool {
    const AMD_VENDOR: &str = "0x1002";
    let Ok(entries) = std::fs::read_dir(sys_drm) else {
        return false;
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("card"))
        })
        .any(|e| {
            std::fs::read_to_string(e.path().join("device/vendor"))
                .is_ok_and(|v| v.trim() == AMD_VENDOR)
        })
}

/// gpu_layers 三态解析：CLI > 配置文件 > 内置默认 99（全量卸载）。
pub fn resolve_gpu_layers(cli: Option<u32>, file: Option<u32>) -> u32 {
    cli.or(file).unwrap_or(DEFAULT_GPU_LAYERS)
}

/// mmproj_offload 三态解析：`--no-mmproj-offload` > `--mmproj-offload` > 配置文件 >
/// 默认开启（llama-server 原生行为）。
/// 两个 CLI flag 互斥（clap conflicts_with），不会同时为 true。
pub fn resolve_mmproj_offload(cli_on: bool, cli_off: bool, file: Option<bool>) -> bool {
    if cli_off {
        return false;
    }
    if cli_on {
        return true;
    }
    file.unwrap_or(true)
}

/// 平台默认后端提示（doctor 展示与 main 实际选择保持同一逻辑）。
/// 注意 NPU 只在 llama-server 缺席时兜底：装了 llama-server 的 Intel 机器默认仍走 gpu，
/// 避免把「能跑 GPU」的机器静默降级到更慢的 NPU 管线。
pub fn default_provider_hint() -> AsrProvider {
    if cfg!(apple_native) {
        AsrProvider::Coreml
    } else if cfg!(target_os = "linux")
        && Path::new(crate::npu::NPU_DEVICE_PATH).exists()
        && crate::error::require_cmd("llama-server").is_err()
    {
        AsrProvider::Npu
    } else {
        AsrProvider::Gpu
    }
}

/// Whether this build includes the native Apple speech backend.
pub fn apple_native_available() -> bool {
    cfg!(apple_native)
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
    if under("bilibili.com") || host == "b23.tv" {
        Some("bilibili")
    } else if under("youtube.com") || host == "youtu.be" {
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

/// `{root}/{platform}/{title}/{id}/`
pub fn course_dir(root: &Path, platform: &str, title: &str, id: &str) -> PathBuf {
    root.join(sanitize_component(platform))
        .join(sanitize_component(title))
        .join(sanitize_component(id))
}

pub fn platform_from(source: &str, extractor: &str) -> String {
    let e = extractor.to_ascii_lowercase();
    if Path::new(source).is_file() || e == "local" {
        "local".into()
    } else if e.contains("bili") || known_video_site(source) == Some("bilibili") {
        "bilibili".into()
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
    if let Some(id) = bvid(source) {
        return id;
    }
    if let Some(id) = youtube_id(source) {
        return id;
    }
    sanitize_component(source)
}

fn bvid(s: &str) -> Option<String> {
    // 锚定到 /video/ 路径段之后查找 BV，避免查询参数（?x=BV...）误中
    let rest = s.split_once("/video/").map(|(_, r)| r)?;
    let j = rest.find("BV")?;
    let id: String = rest[j..].chars().take(12).collect();
    if id.len() >= 6 && id.chars().all(|c| c.is_ascii_alphanumeric()) {
        Some(id)
    } else {
        None
    }
}

fn youtube_id(s: &str) -> Option<String> {
    if let Some(rest) = s.split_once("v=").map(|(_, r)| r) {
        let id = rest.split(['&', '#', '/']).next()?;
        if id.len() >= 6 {
            return Some(id.to_string());
        }
    }
    if let Some(rest) = s.split_once("youtu.be/").map(|(_, r)| r) {
        let id = rest.split(['?', '&', '#', '/']).next()?;
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    None
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
        assert!(!looks_like_source("course2md"));
        assert!(looks_like_source(
            "https://www.bilibili.com/video/BV1pb8o6yE8f"
        ));
        assert!(looks_like_source("https://youtu.be/dQw4w9WgXcQ"));
    }

    #[test]
    fn slug_and_layout() {
        assert_eq!(
            infer_slug("https://www.bilibili.com/video/BV1pb8o6yE8f/?spm=1"),
            "BV1pb8o6yE8f"
        );
        assert_eq!(infer_slug("https://youtu.be/dQw4w9WgXcQ"), "dQw4w9WgXcQ");
        let p = course_dir(Path::new("out"), "bilibili", "欢迎来到未来", "BV1pb8o6yE8f");
        assert_eq!(p, PathBuf::from("out/bilibili/欢迎来到未来/BV1pb8o6yE8f"));
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
            out_dir: "out".into(),
            out_root: "out".into(),
            similarity: DEFAULT_SIMILARITY,
            sample_interval: DEFAULT_SAMPLE_INTERVAL,
            cooldown: DEFAULT_COOLDOWN,
            max_height: DEFAULT_MAX_HEIGHT,
            slide_mode: SlideMode::Stable,
            stable_secs: DEFAULT_STABLE_SECS,
            roi: None,
            threads: DEFAULT_THREADS,
            provider: AsrProvider::Gpu,
            max_speech: DEFAULT_MAX_SPEECH,
            formats: vec![OutputFormat::Md],
            model_dir: "/tmp/m".into(),
            keep_video: false,
            no_download: false,
            resume: false,
            llm: Default::default(),
            asr_api: Default::default(),
            asr_model: None,
            gpu_layers: DEFAULT_GPU_LAYERS,
            mmproj_offload: true,
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
        c.max_speech = 0.0;
        assert!(c.validate().is_err());
        c.max_speech = f32::INFINITY;
        assert!(c.validate().is_err());
        c.max_speech = DEFAULT_MAX_SPEECH;

        c.similarity = 0.0;
        assert!(c.validate().is_err());
        c.similarity = 1.5;
        assert!(c.validate().is_err());
        c.similarity = DEFAULT_SIMILARITY;

        c.sample_interval = 0.0;
        assert!(c.validate().is_err());
        c.sample_interval = DEFAULT_SAMPLE_INTERVAL;

        c.threads = 0;
        assert!(c.validate().is_err());
        c.threads = DEFAULT_THREADS;

        c.formats = vec![];
        assert!(
            c.validate().is_ok(),
            "Internal reading does not depend on optional exports"
        );
    }

    #[test]
    fn validate_rejects_model_provider_mismatch() {
        // gpu/cpu 只有 Qwen3 GGUF：显式要 whisper 必须报错而不是静默用 qwen
        let mut c = valid_cfg();
        c.asr_model = Some("whisper".into());
        assert!(c.validate_asr().is_err());
        c.provider = AsrProvider::Coreml;
        c.asr_model = Some("whisper".into());
        c.validate_asr().unwrap();
        c.asr_model = Some("qwen-not-a-supported-model".into());
        assert!(c.validate_asr().is_err());
        c.provider = AsrProvider::Npu;
        c.asr_model = Some("org/custom-ov-model".into());
        c.validate_asr().unwrap();
        c.asr_model = Some("not-a-repo-id".into());
        assert!(c.validate_asr().is_err());
    }

    #[test]
    fn canonical_model_accepts_documented_aliases_per_backend() {
        for (provider, raw, id) in [
            (AsrProvider::Gpu, "qwen3", "qwen3-1.7b"),
            (AsrProvider::Cpu, "Qwen3-ASR-1.7B-Q8_0.gguf", "qwen3-1.7b"),
            (AsrProvider::Coreml, "qwen3", "qwen3-1.7b"),
            (AsrProvider::Coreml, " 0.6b ", "qwen3-0.6b"),
            (AsrProvider::Npu, "turbo", "whisper"),
            (AsrProvider::Npu, "MyOrg/Model-INT8", "MyOrg/Model-INT8"),
        ] {
            assert_eq!(
                canonical_model(provider, raw).unwrap(),
                id,
                "{provider} {raw}"
            );
        }
        assert!(canonical_model(AsrProvider::Gpu, "whisper").is_err());
        assert!(canonical_model(AsrProvider::Coreml, "whisper-tiny").is_err());
        assert!(canonical_model(AsrProvider::Coreml, "foo-0.6-bar").is_err());
        for provider in [AsrProvider::Gpu, AsrProvider::Coreml, AsrProvider::Npu] {
            for model in local_models(provider) {
                assert_eq!(canonical_model(provider, model.id).unwrap(), model.id);
            }
        }
    }

    #[test]
    fn known_video_site_covers_short_links_and_rejects_lookalikes() {
        assert_eq!(known_video_site("b23.tv/abc"), Some("bilibili"));
        assert_eq!(
            known_video_site("m.bilibili.com/video/BV1"),
            Some("bilibili")
        );
        assert_eq!(known_video_site("https://youtu.be/x"), Some("youtube"));
        assert_eq!(known_video_site("https://bilibili.com.evil.test/"), None);
        assert_eq!(
            known_video_site("https://youtube.com/?bilibili.com"),
            Some("youtube")
        );
        assert_eq!(known_video_site("file:///bilibili.com"), None);
    }

    #[test]
    fn resume_priority() {
        // --no-resume 压倒一切
        assert!(!resolve_resume(true, true, Some(true)));
        assert!(!resolve_resume(false, true, Some(true)));
        // --resume 次之
        assert!(resolve_resume(true, false, Some(false)));
        // 都没传：回落配置文件
        assert!(resolve_resume(false, false, Some(true)));
        assert!(!resolve_resume(false, false, Some(false)));
        assert!(resolve_resume(false, false, None));
    }

    #[test]
    fn validate_rejects_gpu_layers_out_of_range() {
        let mut c = valid_cfg();
        c.gpu_layers = 100;
        assert!(c.validate().is_err());
        c.gpu_layers = 0;
        c.validate().unwrap();
        c.gpu_layers = 99;
        c.validate().unwrap();
    }

    #[test]
    fn gpu_offload_priority() {
        // CLI > 配置文件 > 内置默认
        assert_eq!(resolve_gpu_layers(Some(8), Some(4)), 8);
        assert_eq!(resolve_gpu_layers(None, Some(4)), 4);
        assert_eq!(resolve_gpu_layers(None, None), DEFAULT_GPU_LAYERS);

        assert!(!resolve_mmproj_offload(true, true, Some(true)));
        assert!(resolve_mmproj_offload(true, false, Some(false)));
        assert!(!resolve_mmproj_offload(false, true, Some(true)));
        assert!(!resolve_mmproj_offload(false, false, Some(false)));
        assert!(resolve_mmproj_offload(false, false, Some(true)));
        assert!(resolve_mmproj_offload(false, false, None));
    }

    #[test]
    fn amd_drm_card_detection() {
        let tmp = std::env::temp_dir().join(format!("c2md-drm-{}", std::process::id()));
        let vendor = tmp.join("card0/device");
        std::fs::create_dir_all(&vendor).unwrap();
        // 无 vendor 文件 → false
        assert!(!any_amd_drm_card(&tmp));
        // NVIDIA vendor id → false
        std::fs::write(vendor.join("vendor"), "0x10de\n").unwrap();
        assert!(!any_amd_drm_card(&tmp));
        // AMD vendor id（带换行）→ true
        std::fs::write(vendor.join("vendor"), "0x1002\n").unwrap();
        assert!(any_amd_drm_card(&tmp));
        // 不存在的根目录 → false
        assert!(!any_amd_drm_card(Path::new("/nonexistent/c2md-drm")));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn roi_parse() {
        let r = Roi::parse("25%,0%-100%,100%").unwrap();
        assert_eq!(r.pixels(1000, 800), (250, 0, 1000, 800));
        assert!(Roi::parse("nonsense").is_err());
    }
}
