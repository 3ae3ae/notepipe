//! The configuration file (~/.config/notepipe/config.toml).
//! Precedence: command-line options, then the file, then built-in defaults.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Output {
    pub dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub on_complete: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Transcript {
    pub source: Option<crate::config::TranscriptSource>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Screenshots {
    pub mode: Option<crate::config::ScreenshotMode>,
    pub sensitivity: Option<f64>,
    pub frame_interval: Option<f64>,
    pub min_gap: Option<f64>,
    pub settle: Option<f64>,
    pub region: Option<String>,
}

/// External program that resolves an online URL into local media and subtitles.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Source {
    #[serde(rename = "adapter")]
    pub command: Vec<String>,
}

impl Source {
    fn is_default(&self) -> bool {
        self.command.is_empty()
    }
}

/// HTTP transcription dialect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AsrMode {
    /// POST /audio/transcriptions, the OpenAI speech-to-text API
    #[default]
    Transcriptions,
    /// Audio input to /chat/completions, for audio-capable chat models
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
    pub service: String,
    pub api_key_env: String,
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
            service: String::new(),
            api_key_env: String::new(),
            mode: AsrMode::Transcriptions,
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            language: String::new(),
            concurrency: Service::default().concurrency as usize,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Detection {
    pub threshold: f32,
    pub min_silence: f32,
    pub max_segment: f32,
}

impl Default for Detection {
    fn default() -> Self {
        let asr = Asr::default();
        Self {
            threshold: asr.vad_threshold,
            min_silence: asr.min_silence,
            max_segment: asr.max_speech,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Service {
    pub base_url: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub adapter: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key_env: String,
    pub concurrency: i64,
}

impl Default for Service {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            adapter: Vec::new(),
            api_key: String::new(),
            api_key_env: String::new(),
            concurrency: 4,
        }
    }
}

impl Service {
    pub fn is_connected(&self) -> bool {
        !self.base_url.trim().is_empty() || !self.adapter.is_empty()
    }

    pub fn validate(&self, name: &str) -> Result<()> {
        anyhow::ensure!(
            self.base_url.trim().is_empty() || self.adapter.is_empty(),
            "Service {name}: set base_url or adapter, not both"
        );
        anyhow::ensure!(
            self.api_key.is_empty() || self.api_key_env.is_empty(),
            "Service {name}: set api_key or api_key_env, not both"
        );
        anyhow::ensure!(
            self.adapter
                .first()
                .is_none_or(|program| !program.trim().is_empty()),
            "Service {name}: the adapter command must name an executable"
        );
        anyhow::ensure!(
            (1..=16).contains(&self.concurrency),
            "Service {name}: concurrency must be 1 to 16"
        );
        if !self.base_url.trim().is_empty() {
            crate::config::ensure_http_url(&self.base_url)?;
        }
        Ok(())
    }

    pub fn key(&self, name: &str) -> Result<String> {
        resolve_api_key(name, &self.api_key, &self.api_key_env)
    }
}

pub(crate) fn resolve_api_key(name: &str, key: &str, var: &str) -> Result<String> {
    if var.is_empty() {
        return Ok(key.to_owned());
    }
    std::env::var(var)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("Service {name}: the environment variable {var} is not set"))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Speech {
    pub service: String,
    #[serde(rename = "api")]
    pub mode: AsrMode,
    pub model: String,
    pub detection: Detection,
}

impl From<&Asr> for Speech {
    fn from(asr: &Asr) -> Self {
        Self {
            service: asr.service.clone(),
            mode: asr.mode,
            model: asr.model.clone(),
            detection: Detection {
                threshold: asr.vad_threshold,
                min_silence: asr.min_silence,
                max_segment: asr.max_speech,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ConfigFile {
    pub output: Output,
    pub transcript: Transcript,
    pub speech: Speech,
    pub screenshots: Screenshots,
    pub services: BTreeMap<String, Service>,
    #[serde(default, deserialize_with = "crate::llm::deserialize_stages")]
    pub stages: Vec<crate::llm::Stage>,
    #[serde(skip_serializing_if = "Source::is_default")]
    pub download: Source,
}

pub fn config_path() -> PathBuf {
    crate::config::config_dir().join("config.toml")
}

/// Reads the configuration file: defaults when it does not exist, an error when it cannot be parsed.
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
    // Back up the old file before overwriting it, since it may hold manual edits; a failed backup only warns.
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
        let mut value = serde_json::to_value(cfg)?;
        for service in value["services"].as_object_mut().unwrap().values_mut() {
            if service.get("adapter").is_some() {
                service.as_object_mut().unwrap().remove("base_url");
            }
        }
        serde_json::to_vec_pretty(&value)?
    } else {
        let mut value = toml::Value::try_from(cfg)?;
        for (_, service) in value["services"].as_table_mut().unwrap().iter_mut() {
            if service.get("adapter").is_some() {
                service.as_table_mut().unwrap().remove("base_url");
            }
        }
        toml::to_string_pretty(&value)?.into_bytes()
    };
    crate::checkpoint::atomic_write(&p, &bytes)?;
    Ok(p)
}

/// The template written by `config init`; its values come from the built-in defaults, so they cannot drift.
pub fn template() -> String {
    use crate::config as c;
    let asr = Asr::default();
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
        ("{SPEECH_API}", AsrMode::default().to_string()),
        ("{CONCURRENCY}", Service::default().concurrency.to_string()),
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
    ]
    .into_iter()
    .fold(TEMPLATE.to_owned(), |text, (key, value)| {
        text.replace(key, &value)
    })
}

const TEMPLATE: &str = r#"# Every setting is described in https://github.com/3ae3ae/notepipe/blob/main/docs/configuration.md

[services.speech]
base_url = ""
concurrency = {CONCURRENCY}

[services.ai]
base_url = ""
concurrency = {CONCURRENCY}

[output]
dir = "{OUT}"

[transcript]
source = "{TRANSCRIPT_SOURCE}"

[speech]
service = "speech"
api = "{SPEECH_API}"
model = ""

[speech.detection]
threshold = {VAD_THRESHOLD}
min_silence = {MIN_SILENCE}
max_segment = {MAX_SPEECH}

[screenshots]
mode = "{SCREENSHOT_MODE}"
sensitivity = {SIMILARITY}
frame_interval = {SAMPLE_INTERVAL}
min_gap = {COOLDOWN}
settle = {STABLE_SECS}

[[stages]]
name = "proofread"
service = "ai"
model = ""
input = "paragraphs"
output = "replace"
prompt = '''
You proofread transcripts of spoken recordings. Each input item is a stretch of continuous speech,
already split at natural pauses. Fix obvious speech-recognition errors (misheard words, homophones,
misspelled names and terms), remove filler that adds nothing, and repair awkward sentence breaks and
punctuation so the text reads naturally. Do not summarize, expand, translate, add or remove
substantive content, or change the meaning. Keep the original language. If an item contains only
filler or meaningless fragments (such as a lone "um" or "you know"), return an empty text for it;
never empty an item that has real content. Write mathematical formulas, symbols and expressions as
LaTeX: wrap inline math in $...$ and put display math on its own line in $$...$$; rewrite spoken
formulas, such as "x squared", as LaTeX too; do not add $ when there is no math. If a screenshot is
attached, use it only to correct the spelling of terms and proper names; do not describe or comment
on it.
'''

[[stages]]
name = "summary"
service = "ai"
model = ""
input = "transcript"
output = "top"
prompt = '''
You summarize recordings from their timestamped transcript. Use only what the transcript says: never
invent facts, numbers, names or opinions, and leave out anything uncertain rather than guess. Write a
one-sentence overview, a list of 3 to 6 key points, and an outline of 3 to 8 sections in time order
that covers the whole recording; start each outline item with the section's start time as [mm:ss]
and describe the section in one or two sentences. Write mathematical formulas as LaTeX in $...$ or
$$...$$. Use Markdown with headings of level 2 or lower, and do not repeat the title. Write in the
language of the transcript.
'''
"#;

/// Prints the file merged with the built-in defaults, without command-line or environment overrides.
/// Uses [`crate::options::resolve`], the same resolution as a real run.
pub fn print_effective(cfg: &ConfigFile) -> Result<()> {
    let r = crate::options::resolve(String::new(), &crate::cli::RunOpts::default(), cfg)?;
    let path = config_path();
    println!("Configuration file: {}", path.display());
    if !path.is_file() {
        println!("(not created yet; `notepipe config init` creates it)");
    }
    println!(
        "Settings in effect, from the file and the built-in defaults (command-line options and environment variables are not included):"
    );
    println!();
    println!("[output]");
    println!("  {:<17}: {}", "dir", r.out_root.display());
    println!("  {:<17}: {:?}", "on_complete", r.on_complete);
    println!("[transcript]");
    println!("  {:<17}: {}", "source", r.transcript_source);
    println!("  {:<17}: {}", "language", r.asr.language);
    let hidden = |key: &str| {
        if key.is_empty() { "-" } else { "(set, hidden)" }
    };
    for (name, service) in &cfg.services {
        println!("[services.{name}]");
        println!("  {:<17}: {}", "base_url", service.base_url);
        println!("  {:<17}: {:?}", "adapter", service.adapter);
        println!("  {:<17}: {}", "api_key", hidden(&service.api_key));
        println!("  {:<17}: {}", "api_key_env", service.api_key_env);
        println!("  {:<17}: {}", "concurrency", service.concurrency);
    }
    println!("[speech]");
    println!("  {:<17}: {}", "service", cfg.speech.service);
    println!("  {:<17}: {}", "api", r.asr.mode);
    println!("  {:<17}: {}", "model", r.asr.model);
    println!("[speech.detection]");
    println!("  {:<17}: {}", "threshold", r.asr.vad_threshold);
    println!("  {:<17}: {}", "min_silence", r.asr.min_silence);
    println!("  {:<17}: {}", "max_segment", r.asr.max_speech);
    println!("[screenshots]");
    println!("  {:<17}: {}", "mode", r.screenshot_mode);
    println!("  {:<17}: {}", "sensitivity", r.similarity);
    println!("  {:<17}: {}", "frame_interval", r.sample_interval);
    println!("  {:<17}: {}", "min_gap", r.cooldown);
    println!("  {:<17}: {}", "settle", r.stable_secs);
    println!(
        "  {:<17}: {}",
        "region",
        cfg.screenshots.region.as_deref().unwrap_or("-")
    );
    for stage in &cfg.stages {
        println!("[[stages]]");
        println!("  {:<17}: {}", "name", stage.name);
        println!("  {:<17}: {}", "service", stage.service);
        println!("  {:<17}: {}", "model", stage.model);
        println!("  {:<17}: {:?}", "input", stage.input);
        println!("  {:<17}: {:?}", "output", stage.output);
        println!("  {:<17}: {}", "images", stage.images);
        println!(
            "  {:<17}: {:?}",
            "request keys",
            stage.request.keys().collect::<Vec<_>>()
        );
    }
    println!("[download]");
    println!("  {:<17}: {:?}", "adapter", r.source.command);
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn readme_shows_the_generated_file_with_shortened_prompts() {
        let mut in_prompt = false;
        let shortened: String = super::template()
            .lines()
            .filter_map(|line| {
                if in_prompt {
                    in_prompt = line != "'''";
                    return None;
                }
                in_prompt = line == "prompt = '''";
                Some(if in_prompt {
                    "prompt = '''…'''\n".to_owned()
                } else {
                    format!("{line}\n")
                })
            })
            .collect();
        // Windows checkouts may turn the README into CRLF.
        assert!(
            include_str!("../README.md")
                .replace("\r\n", "\n")
                .contains(&shortened)
        );
    }

    #[test]
    fn old_sections_are_rejected() {
        assert!(super::parse("[asr]\nbase_url = 'https://example.test/v1'\n").is_err());
        assert!(super::parse("[[ai.stages]]\nname = 'summary'\n").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn saving_a_symlinked_configuration_updates_and_backs_up_its_target() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("dotfiles/settings.toml");
        let link = root.path().join("config.toml");
        std::fs::create_dir(target.parent().unwrap()).unwrap();
        let original = "[output]\ndir = 'my-notes'\n";
        std::fs::write(&target, original).unwrap();
        std::os::unix::fs::symlink("dotfiles/settings.toml", &link).unwrap();
        let mut cfg = super::load_from(Some(&link)).unwrap();
        cfg.services.insert(
            "speech".into(),
            super::Service {
                base_url: "https://example.test/v1".into(),
                ..Default::default()
            },
        );
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
    fn template_contains_defaults_and_two_explicit_stages() {
        let file: super::ConfigFile = toml::from_str(&super::template()).unwrap();
        let from_template =
            crate::options::resolve(String::new(), &Default::default(), &file).unwrap();
        let mut built_in =
            crate::options::resolve(String::new(), &Default::default(), &Default::default())
                .unwrap();
        assert_eq!(from_template.stable_secs, built_in.stable_secs);
        assert_eq!(from_template.similarity, built_in.similarity);
        assert_eq!(from_template.sample_interval, built_in.sample_interval);
        assert_eq!(from_template.cooldown, built_in.cooldown);
        assert_eq!(from_template.screenshot_mode, built_in.screenshot_mode);
        assert_eq!(from_template.roi, built_in.roi);
        assert_eq!(from_template.transcript_source, built_in.transcript_source);
        assert_eq!(file.transcript.language, built_in.asr.language);
        assert_eq!(from_template.source, built_in.source);
        built_in.asr.service = "speech".into();
        assert_eq!(from_template.asr, built_in.asr);
        assert_eq!(
            file.speech,
            super::Speech {
                service: "speech".into(),
                ..Default::default()
            }
        );
        assert_eq!(
            file.services,
            [
                ("speech".into(), super::Service::default()),
                ("ai".into(), super::Service::default())
            ]
            .into()
        );
        assert_eq!(
            file.output.dir.as_deref(),
            Some(std::path::Path::new(crate::config::DEFAULT_OUT_DIR))
        );
        assert_eq!(file.transcript.source, Some(built_in.transcript_source));
        assert_eq!(file.screenshots.mode, Some(built_in.screenshot_mode));
        assert_eq!(file.screenshots.sensitivity, Some(built_in.similarity));
        assert_eq!(
            file.screenshots.frame_interval,
            Some(built_in.sample_interval)
        );
        assert_eq!(file.screenshots.min_gap, Some(built_in.cooldown));
        assert_eq!(file.screenshots.settle, Some(built_in.stable_secs));
        assert_eq!(file.stages.len(), 2);
        for (stage, (name, input, output)) in file.stages.iter().zip([
            (
                "proofread",
                crate::llm::Input::Paragraphs,
                crate::llm::Output::Replace,
            ),
            (
                "summary",
                crate::llm::Input::Transcript,
                crate::llm::Output::Top,
            ),
        ]) {
            assert_eq!(
                (stage.name.as_str(), stage.input, stage.output),
                (name, input, output)
            );
            assert_eq!(stage.service, "ai");
            assert!(stage.model.is_empty());
            assert!(!stage.prompt.is_empty());
            assert_eq!(stage.prompt, stage.prompt.trim());
        }
        let value: toml::Value = toml::from_str(&super::template()).unwrap();
        let serialized = toml::Value::try_from(&file).unwrap();
        for key in ["threshold", "min_silence", "max_segment"] {
            assert!(value["speech"]["detection"].get(key).is_some());
        }
        for field in ["service", "api", "model"] {
            assert_eq!(value["speech"][field], serialized["speech"][field]);
        }
        for name in ["speech", "ai"] {
            assert_eq!(value["services"][name]["base_url"].as_str(), Some(""));
            assert_eq!(value["services"][name]["concurrency"].as_integer(), Some(4));
        }
    }

    #[test]
    fn wizard_saves_omit_empty_options_and_round_trip_the_template() {
        let root = tempfile::tempdir().unwrap();
        let mut cfg: super::ConfigFile = toml::from_str(&super::template()).unwrap();
        cfg.services.get_mut("speech").unwrap().base_url = "https://speech.test/v1".into();
        let path = root.path().join("config.toml");
        super::save(&cfg, Some(&path)).unwrap();
        let value: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for service in value["services"].as_table().unwrap().values() {
            for key in ["api_key", "api_key_env", "adapter"] {
                assert!(service.get(key).is_none(), "{key}");
            }
            assert!(service.get("concurrency").is_some());
        }
        for key in ["service", "api", "model"] {
            assert!(value["speech"].get(key).is_some());
        }
        assert!(value["output"].get("on_complete").is_none());
        assert!(value["transcript"].get("language").is_none());
        assert!(value.get("download").is_none());
        for stage in value["stages"].as_array().unwrap() {
            for key in ["name", "service", "model", "input", "output", "prompt"] {
                assert!(stage.get(key).is_some(), "{key}");
            }
            for key in ["images", "request"] {
                assert!(stage.get(key).is_none(), "{key}");
            }
        }
        assert_eq!(value["services"]["ai"]["base_url"].as_str(), Some(""));
        assert_eq!(super::load_from(Some(&path)).unwrap(), cfg);
        let speech = cfg.services.get_mut("speech").unwrap();
        speech.base_url.clear();
        speech.adapter = vec!["adapter".into()];
        super::save(&cfg, Some(&path)).unwrap();
        let value: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(value["services"]["speech"].get("base_url").is_none());
        assert_eq!(super::load_from(Some(&path)).unwrap(), cfg);
    }

    #[test]
    fn services_reject_conflicting_connections_keys_empty_programs_and_invalid_concurrency() {
        for (fields, expected) in [
            (
                serde_json::json!({"base_url":"http://localhost:1","adapter":["adapter"]}),
                "Service test: set base_url or adapter, not both",
            ),
            (
                serde_json::json!({"api_key":"key","api_key_env":"VAR"}),
                "Service test: set api_key or api_key_env, not both",
            ),
            (
                serde_json::json!({"adapter":[""]}),
                "Service test: the adapter command must name an executable",
            ),
            (
                serde_json::json!({"concurrency":-1}),
                "Service test: concurrency must be 1 to 16",
            ),
            (
                serde_json::json!({"concurrency":0}),
                "Service test: concurrency must be 1 to 16",
            ),
            (
                serde_json::json!({"concurrency":17}),
                "Service test: concurrency must be 1 to 16",
            ),
        ] {
            let service: super::Service = serde_json::from_value(fields).unwrap();
            assert_eq!(service.validate("test").unwrap_err().to_string(), expected);
        }
        super::Service::default().validate("empty").unwrap();
        for base_url in ["not a URL", "file:///tmp/service", "https://"] {
            assert!(
                super::Service {
                    base_url: base_url.into(),
                    ..Default::default()
                }
                .validate("test")
                .is_err()
            );
        }
    }
}
