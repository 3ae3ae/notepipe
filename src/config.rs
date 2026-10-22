use anyhow::{Context, Result as AnyhowResult};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

// Built-in defaults, defined once here for option resolution and the config template.
/// SSIM similarity threshold; higher is more sensitive and takes more screenshots.
pub const DEFAULT_SIMILARITY: f64 = 0.85;
/// Seconds between sampled frames.
pub const DEFAULT_SAMPLE_INTERVAL: f64 = 1.0;
/// Minimum seconds between two screenshots.
pub const DEFAULT_COOLDOWN: f64 = 10.0;
/// Seconds the picture must stay unchanged in stable mode.
pub const DEFAULT_STABLE_SECS: f64 = 0.8;
/// Longest speech segment in seconds; longer ones are split at quiet points.
pub const DEFAULT_MAX_SPEECH: f32 = 20.0;
/// Default output directory.
pub const DEFAULT_OUT_DIR: &str = ".";
/// Checks that a service address is a complete http(s) URL; shared by the speech and AI settings.
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
/// The single rule for endpoint suffixes, shared by speech and AI requests.
pub fn append_endpoint_suffix(base: &str, suffix: &str) -> String {
    if let Ok(mut url) = url::Url::parse(base) {
        let path = url.path().trim_end_matches('/');
        let path = if path.ends_with(suffix) {
            path.to_owned()
        } else {
            format!("{path}{suffix}")
        };
        url.set_path(&path);
        return url.into();
    }
    let base = base.trim_end_matches('/');
    if base.ends_with(suffix) {
        base.to_string()
    } else {
        format!("{base}{suffix}")
    }
}

/// `path` ends with a known endpoint suffix that conflicts with the desired one.
/// Callers reject a mismatch instead of silently switching protocols, which could send credentials to the wrong endpoint.
pub fn conflicting_endpoint_suffix(path: &str, desired: &str, known: &[&str]) -> bool {
    let path = path.trim_end_matches('/');
    !path.ends_with(desired) && known.iter().any(|k| path.ends_with(k))
}

/// Accept a service root or its exact supported endpoint, never double-append paths.
pub fn asr_endpoint(api: &crate::settings::Asr) -> AnyhowResult<String> {
    let base = api.base_url.trim();
    let url = ensure_http_url(base)?;
    let suffix = match api.mode {
        crate::settings::AsrMode::Transcriptions => "/audio/transcriptions",
        crate::settings::AsrMode::Chat => "/chat/completions",
    };
    anyhow::ensure!(
        !conflicting_endpoint_suffix(
            url.path(),
            suffix,
            &["/audio/transcriptions", "/chat/completions"]
        ),
        "Service endpoint and protocol do not match"
    );
    Ok(append_endpoint_suffix(base, suffix))
}

/// When to take a screenshot: `first` at the first changed frame, `stable` once the picture settles (for animated slides).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScreenshotMode {
    /// Once the picture stops changing, for animated slides
    #[default]
    Stable,
    /// As soon as the picture changes
    First,
}

impl fmt::Display for ScreenshotMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::First => "first",
            Self::Stable => "stable",
        })
    }
}

/// Effective settings of a run, resolved from options, the configuration and defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    pub url: String,
    pub transcript_name: Option<String>,
    pub screenshots_name: Option<String>,
    pub source: crate::settings::Source,
    pub out_dir: PathBuf,
    pub similarity: f64,
    pub sample_interval: f64,
    pub cooldown: f64,
    /// When to take a screenshot: `first` at the first changed frame, `stable` once the picture settles.
    pub screenshot_mode: ScreenshotMode,
    /// Seconds the picture must stay unchanged in stable mode.
    pub stable_secs: f64,
    pub roi: Option<Roi>,
    /// Clear this source's cache before starting.
    pub no_resume: bool,
    pub subtitle: Option<PathBuf>,
    pub stages: Vec<(String, crate::settings::Stage)>,
    pub skip: Vec<String>,
    pub verbose: bool,
    pub providers: std::collections::BTreeMap<String, crate::settings::Provider>,
    /// HTTP transcription settings with CLI overrides applied.
    pub asr: crate::settings::Asr,
    /// Root containing published note folders.
    pub out_root: PathBuf,
}

/// Region of interest, in percent (0–100%).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Roi {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

impl Roi {
    /// Parses "x1,y1-x2,y2" with percentage coordinates.
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let (a, b) = s
            .split_once('-')
            .ok_or_else(|| anyhow::anyhow!("screenshots.region must use x1,y1-x2,y2: {s:?}"))?;
        let (x1, y1) = parse_xy(a)?;
        let (x2, y2) = parse_xy(b)?;
        let (x1, x2) = (x1.min(x2), x1.max(x2));
        let (y1, y2) = (y1.min(y2), y1.max(y2));
        if x2 <= x1 || y2 <= y1 {
            anyhow::bail!("screenshots.region must have a positive width and height: {s:?}");
        }
        Ok(Self { x1, y1, x2, y2 })
    }

    /// Converts to a pixel rectangle for the given frame size.
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
        .ok_or_else(|| anyhow::anyhow!("screenshots.region coordinates must use x,y: {pair:?}"))?;
    Ok((parse_coord(a)?, parse_coord(b)?))
}

fn parse_coord(s: &str) -> anyhow::Result<f64> {
    let s = s.trim();
    let p = s.strip_suffix('%').ok_or_else(|| {
        anyhow::anyhow!("screenshots.region coordinates must use percentages: {s:?}")
    })?;
    let percent: f64 = p.trim().parse()?;
    anyhow::ensure!(
        (0.0..=100.0).contains(&percent),
        "screenshots.region percentages must be between 0 and 100"
    );
    let v = percent / 100.0;
    anyhow::ensure!(
        v.is_finite() && v >= 0.0,
        "screenshots.region coordinates must be finite, non-negative numbers"
    );
    Ok(v)
}

impl PipelineConfig {
    /// Checks all settings up front, so every configuration error appears before downloads or other expensive work.
    pub fn validate(&self) -> AnyhowResult<()> {
        validate_tuning(
            self.similarity,
            self.sample_interval,
            self.cooldown,
            self.stable_secs,
            self.asr.max_speech,
            self.asr.vad_threshold,
            self.asr.min_silence,
        )
    }

    pub fn validate_asr(&self) -> AnyhowResult<()> {
        if self.asr.command.is_empty() {
            ensure_http_url(&asr_endpoint(&self.asr)?)?;
        }
        Ok(())
    }

    pub fn frames_dir(&self) -> PathBuf {
        self.out_dir.join("frames")
    }
}

pub(crate) fn validate_tuning(
    similarity: f64,
    sample_interval: f64,
    cooldown: f64,
    stable_secs: f64,
    max_speech: f32,
    vad_threshold: f32,
    min_silence: f32,
) -> AnyhowResult<()> {
    let finite = |name: &str, v: f64| -> AnyhowResult<()> {
        anyhow::ensure!(
            v.is_finite(),
            "{name} must be a finite number (received {v})"
        );
        Ok(())
    };
    finite("screenshots.sensitivity", similarity)?;
    finite("screenshots.frame_interval", sample_interval)?;
    finite("screenshots.min_gap", cooldown)?;
    finite("screenshots.settle", stable_secs)?;
    finite("speech.max_segment", max_speech as f64)?;
    finite("speech.threshold", vad_threshold as f64)?;
    finite("speech.min_silence", min_silence as f64)?;

    anyhow::ensure!(
        similarity > 0.0 && similarity <= 1.0,
        "--sensitivity (screenshots.sensitivity) must be greater than 0 and at most 1 (received {})",
        similarity
    );
    anyhow::ensure!(
        sample_interval >= 0.2,
        "--frame-interval (screenshots.frame_interval) must be at least 0.2 seconds (received {})",
        sample_interval
    );
    anyhow::ensure!(
        cooldown >= 0.0,
        "--min-gap (screenshots.min_gap) must be at least 0 seconds (received {})",
        cooldown
    );
    anyhow::ensure!(
        stable_secs >= 0.0,
        "--settle (screenshots.settle) must be at least 0 seconds (received {})",
        stable_secs
    );
    anyhow::ensure!(
        (0.0..=1.0).contains(&vad_threshold),
        "speech.threshold must be between 0 and 1."
    );
    anyhow::ensure!(
        min_silence >= 0.0,
        "speech.min_silence must be non-negative."
    );
    // The lower bound keeps split_smart's clamp(min, max) valid; 0 would panic.
    anyhow::ensure!(
        (1.0..=600.0).contains(&max_speech),
        "speech.max_segment must be between 1 and 600 seconds (received {})",
        max_speech
    );

    Ok(())
}

/// The home directory: HOME, or USERPROFILE on Windows when HOME is unset.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The configuration directory: XDG_CONFIG_HOME, %APPDATA% on Windows, otherwise ~/.config.
pub fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_CONFIG_HOME").filter(|p| !p.is_empty()) {
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

/// Expands `~` and `~/...` to the home directory; returns the path unchanged without one.
/// Otherwise "~/notes" in a config file would create a directory named `~`.
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

const MAX_COMPONENT_CHARS: usize = 80;
// Also bound UTF-8 bytes, leaving room for collision suffixes and the backup name.
const MAX_COMPONENT_BYTES: usize = 200;

/// Makes a safe file name: characters invalid in paths and runs of whitespace become `-`, edges are trimmed and long names truncated.
/// `fallback` is used when nothing is left.
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
    // Reserve one byte for a device-name escape.
    let mut length = out.len().min(MAX_COMPONENT_BYTES - 1);
    while !out.is_char_boundary(length) {
        length -= 1;
    }
    let out = &out[..length];
    let out = out.trim_end_matches(['-', '.', ' ']);
    if out.is_empty() {
        fallback.to_string()
    } else {
        let stem = out.split('.').next().unwrap_or(out).to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ["COM", "LPT"].iter().any(|prefix| {
                stem.strip_prefix(prefix).is_some_and(|suffix| {
                    matches!(
                        suffix,
                        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                    )
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
    fn endpoint_suffix_preserves_queries_and_fragments() {
        for suffix in ["/chat/completions", "/audio/transcriptions"] {
            let expected = format!("https://example.test/v1{suffix}?api-version=1#fragment");
            for base in [
                "https://example.test/v1?api-version=1#fragment".to_owned(),
                "https://example.test/v1/?api-version=1#fragment".to_owned(),
                expected.clone(),
                format!("https://example.test/v1{suffix}/?api-version=1#fragment"),
            ] {
                assert_eq!(append_endpoint_suffix(&base, suffix), expected);
            }
        }
        assert_eq!(
            crate::llm::endpoint("https://example.test/v1?api-version=1"),
            "https://example.test/v1/chat/completions?api-version=1"
        );
    }

    #[test]
    fn empty_xdg_roots_use_platform_defaults() {
        if let Some(home) = std::env::var_os("NOTEPIPE_TEST_XDG_HOME") {
            let home = PathBuf::from(home);
            let config = if cfg!(windows) {
                home.join("config/notepipe")
            } else {
                home.join(".config/notepipe")
            };
            let cache = if cfg!(windows) {
                home.join("local/notepipe/cache")
            } else if cfg!(target_os = "macos") {
                home.join("Library/Caches/notepipe")
            } else {
                home.join(".cache/notepipe")
            };
            assert_eq!(config_dir(), config);
            assert_eq!(cache_dir(), cache);
            return;
        }
        let home = tempfile::tempdir().unwrap();
        for empty in [false, true] {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "config::tests::empty_xdg_roots_use_platform_defaults",
                ])
                .env("NOTEPIPE_TEST_XDG_HOME", home.path())
                .env("HOME", home.path())
                .env("APPDATA", home.path().join("config"))
                .env("LOCALAPPDATA", home.path().join("local"));
            for name in ["XDG_CONFIG_HOME", "XDG_CACHE_HOME"] {
                if empty {
                    command.env(name, "");
                } else {
                    command.env_remove(name);
                }
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }

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
        for name in ["COM¹", "com².txt", "COM³", "LPT¹", "LPT²", "LPT³.ext"] {
            let sanitized = sanitize_filename_with_fallback(name, "untitled");
            assert!(sanitized.contains('_'), "{name}");
        }
        for title in [
            "한".repeat(100),
            "📝".repeat(100),
            format!("{}📝", "a".repeat(199)),
        ] {
            let sanitized = sanitize_filename_with_fallback(&title, "untitled");
            assert!(sanitized.len() <= MAX_COMPONENT_BYTES);
            assert!(title.starts_with(&sanitized));
        }
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
        crate::options::resolve("video".into(), &Default::default(), &Default::default()).unwrap()
    }

    #[test]
    fn validate_accepts_defaults() {
        valid_cfg().validate().unwrap();
    }

    #[test]
    fn validate_rejects_out_of_range() {
        let mut c = valid_cfg();
        // max_speech = 0 used to make split_smart panic in clamp(min > max).
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
            asr.base_url = "https://example.test/v1?api-version=1".into();
            assert_eq!(
                asr_endpoint(&asr).unwrap(),
                format!("{expected}?api-version=1")
            );
            asr.base_url = format!("{expected}?api-version=1");
            assert_eq!(asr_endpoint(&asr).unwrap(), asr.base_url);
            asr.base_url = "https://example.test/v1?redirect=/".into();
            assert_eq!(
                asr_endpoint(&asr).unwrap(),
                format!("{expected}?redirect=/")
            );
            asr.base_url = format!(
                "https://example.test/v1{}",
                if mode == AsrMode::Chat {
                    "/audio/transcriptions"
                } else {
                    "/chat/completions"
                }
            );
            assert!(asr_endpoint(&asr).is_err());
            asr.base_url.push_str("?api-version=1");
            assert!(asr_endpoint(&asr).is_err());
        }
    }

    #[test]
    fn asr_configuration_requires_a_destination_but_never_a_model_or_key() {
        let mut cfg =
            crate::options::resolve("video".into(), &Default::default(), &Default::default())
                .unwrap();
        cfg.asr.base_url.clear();
        assert!(cfg.validate_asr().is_err());
        cfg.asr.base_url = "http://localhost:8000/v1".into();
        cfg.validate_asr().unwrap();
        cfg.asr.base_url = "invalid ignored URL".into();
        cfg.asr.command = vec!["adapter".into()];
        cfg.validate_asr().unwrap();
        for field in ["vad_threshold", "min_silence"] {
            let mut file = ConfigFile::default();
            if field == "vad_threshold" {
                let crate::settings::StageType::Transcript(t) =
                    &mut file.stages.get_mut("transcript").unwrap().kind
                else {
                    unreachable!()
                };
                t.speech.threshold = 1.1;
            } else {
                let crate::settings::StageType::Transcript(t) =
                    &mut file.stages.get_mut("transcript").unwrap().kind
                else {
                    unreachable!()
                };
                t.speech.min_silence = -0.1;
            }
            assert!(crate::options::resolve("video".into(), &Default::default(), &file).is_err());
        }
    }
}
