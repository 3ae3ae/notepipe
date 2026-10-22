//! Named stages and shared provider connections.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

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

/// Resolved speech settings; credentials never come from literal configuration values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
            mode: AsrMode::default(),
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Source {
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Provider {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    pub api_key_env: String,
    pub concurrency: i64,
}
impl Default for Provider {
    fn default() -> Self {
        Self {
            base_url: None,
            command: None,
            args: None,
            api_key_env: String::new(),
            concurrency: 4,
        }
    }
}
impl Provider {
    pub fn validate(&self, name: &str) -> Result<()> {
        anyhow::ensure!(
            self.base_url.is_none() || self.command.is_none(),
            "Provider {name}: set base_url or command, not both"
        );
        anyhow::ensure!(
            self.base_url.is_some() || self.command.is_some(),
            "Provider {name}: set base_url or command"
        );
        anyhow::ensure!(
            self.args.is_none() || self.command.is_some(),
            "Provider {name}: args need a command"
        );
        anyhow::ensure!(
            (1..=16).contains(&self.concurrency),
            "Provider {name}: concurrency must be 1 to 16"
        );
        if let Some(base) = &self.base_url {
            let url = crate::config::ensure_http_url(base)?;
            let path = url.path().trim_end_matches('/');
            anyhow::ensure!(
                !["/chat/completions", "/audio/transcriptions"]
                    .iter()
                    .any(|suffix| path.ends_with(suffix)),
                "Provider {name}: base_url must be the API root, such as https://api.openai.com/v1"
            );
        }
        if let Some(command) = &self.command {
            anyhow::ensure!(
                !command.trim().is_empty(),
                "The adapter command must name an executable."
            );
        }
        Ok(())
    }
    pub fn argv(&self) -> Vec<String> {
        self.command
            .iter()
            .cloned()
            .chain(self.args.iter().flatten().cloned())
            .collect()
    }
    pub fn key(&self, name: &str) -> Result<String> {
        if self.api_key_env.is_empty() {
            return Ok(String::new());
        }
        std::env::var(&self.api_key_env)
            .ok()
            .filter(|key| !key.trim().is_empty())
            .with_context(|| {
                format!(
                    "Provider {name}: the environment variable {} is not set",
                    self.api_key_env
                )
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Program {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub keep: bool,
}
impl Program {
    pub fn argv(&self) -> Vec<String> {
        std::iter::once(self.command.clone())
            .chain(self.args.clone())
            .collect()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Subtitles {
    pub sidecar: bool,
    pub downloaded: bool,
    pub automatic: bool,
}
impl Default for Subtitles {
    fn default() -> Self {
        Self {
            sidecar: true,
            downloaded: true,
            automatic: true,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Speech {
    pub provider: String,
    pub api: AsrMode,
    pub model: String,
    pub threshold: f32,
    pub min_silence: f32,
    pub max_segment: f32,
}
impl Default for Speech {
    fn default() -> Self {
        Self {
            provider: String::new(),
            api: AsrMode::default(),
            model: String::new(),
            threshold: 0.5,
            min_silence: 0.4,
            max_segment: crate::config::DEFAULT_MAX_SPEECH,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Transcript {
    pub keep: bool,
    pub language: String,
    pub sources: Vec<String>,
    pub subtitles: Subtitles,
    pub speech: Speech,
}
impl Default for Transcript {
    fn default() -> Self {
        Self {
            keep: false,
            language: "auto".into(),
            sources: vec!["subtitles".into(), "speech".into()],
            subtitles: Subtitles::default(),
            speech: Speech::default(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Screenshots {
    pub keep: bool,
    pub mode: crate::config::ScreenshotMode,
    pub sensitivity: f64,
    pub frame_interval: f64,
    pub min_gap: f64,
    pub settle: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}
impl Default for Screenshots {
    fn default() -> Self {
        Self {
            keep: false,
            mode: Default::default(),
            sensitivity: crate::config::DEFAULT_SIMILARITY,
            frame_interval: crate::config::DEFAULT_SAMPLE_INTERVAL,
            min_gap: crate::config::DEFAULT_COOLDOWN,
            settle: crate::config::DEFAULT_STABLE_SECS,
            region: None,
        }
    }
}
const DEFAULT_BATCH_SIZE: i64 = 20;
const DEFAULT_CHUNK_SIZE: i64 = 25_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Llm {
    #[serde(deserialize_with = "read_mode")]
    pub reads: String,
    pub provider: String,
    pub model: String,
    pub prompt: String,
    pub keep: bool,
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub images: Option<bool>,
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub batch_size: Option<i64>,
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub chunk_size: Option<i64>,
    pub request: serde_json::Map<String, serde_json::Value>,
}
fn read_mode<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_str().unwrap_or_default().to_owned())
}
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Note {
    pub dir: PathBuf,
}
impl Default for Note {
    fn default() -> Self {
        Self {
            dir: crate::config::DEFAULT_OUT_DIR.into(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Stage {
    Download(Program),
    Transcript(Transcript),
    Screenshots(Screenshots),
    Llm(Llm),
    Filter(Program),
    Note(Note),
}
impl Stage {
    pub fn rank(&self) -> u8 {
        match self {
            Self::Download(_) => 0,
            Self::Transcript(_) | Self::Screenshots(_) => 1,
            Self::Llm(_) | Self::Filter(_) => 2,
            Self::Note(_) => 3,
        }
    }
    pub fn provider(&self) -> Option<&str> {
        match self {
            Self::Transcript(t)
                if t.sources.iter().any(|s| s == "speech") || !t.speech.provider.is_empty() =>
            {
                Some(&t.speech.provider)
            }
            Self::Llm(p) => Some(&p.provider),
            _ => None,
        }
    }
    pub fn used_provider(&self) -> Option<&str> {
        if matches!(self, Self::Transcript(t) if !t.sources.iter().any(|source| source == "speech"))
        {
            return None;
        }
        self.provider()
    }
    pub fn keep(&self) -> bool {
        match self {
            Self::Download(p) | Self::Filter(p) => p.keep,
            Self::Transcript(t) => t.keep,
            Self::Screenshots(s) => s.keep,
            Self::Llm(l) => l.keep,
            Self::Note(_) => false,
        }
    }
    pub fn ai(&self, name: &str) -> Option<crate::llm::Stage> {
        let Self::Llm(l) = self else { return None };
        Some(crate::llm::Stage {
            name: name.into(),
            provider: l.provider.clone(),
            model: l.model.clone(),
            prompt: l.prompt.trim().into(),
            images: l.images.unwrap_or(false),
            batch_size: l.batch_size.unwrap_or(DEFAULT_BATCH_SIZE) as usize,
            chunk_size: l.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE) as usize,
            request: l.request.clone(),
            input: if l.reads == "paragraphs" {
                crate::llm::Input::Paragraphs
            } else {
                crate::llm::Input::Transcript
            },
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub pipeline: Vec<String>,
    #[serde(default)]
    pub providers: BTreeMap<String, Provider>,
    #[serde(default)]
    pub stages: BTreeMap<String, Stage>,
}
impl Default for ConfigFile {
    fn default() -> Self {
        parse(template()).expect("built-in configuration")
    }
}
impl ConfigFile {
    pub fn validate(&self) -> Result<()> {
        for (name, provider) in &self.providers {
            provider.validate(name)?;
        }
        for (name, stage) in &self.stages {
            anyhow::ensure!(
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_')),
                "Stage names may use only letters, digits, \"-\" and \"_\": {name}"
            );
            if let Stage::Llm(l) = stage {
                anyhow::ensure!(
                    matches!(l.reads.as_str(), "paragraphs" | "transcript"),
                    "Stage {name}: reads must be \"paragraphs\" or \"transcript\""
                );
                for (key, present, mode) in [
                    ("images", l.images.is_some(), "paragraphs"),
                    ("batch_size", l.batch_size.is_some(), "paragraphs"),
                    ("chunk_size", l.chunk_size.is_some(), "transcript"),
                ] {
                    anyhow::ensure!(
                        !present || l.reads == mode,
                        "Stage {name}: {key} applies only when reads = \"{mode}\""
                    );
                }
            }
            if let Some(provider) = stage.provider() {
                anyhow::ensure!(!provider.is_empty(), "Stage {name} needs a provider");
                anyhow::ensure!(
                    self.providers.contains_key(provider),
                    "Stage {name}: no provider named {provider} in [providers]"
                );
            }
            match stage {
                Stage::Transcript(t) => {
                    anyhow::ensure!(
                        !t.sources.is_empty()
                            && t.sources
                                .iter()
                                .all(|s| matches!(s.as_str(), "subtitles" | "speech"))
                            && t.sources.len() <= 2
                            && (t.sources.len() == 1 || t.sources[0] != t.sources[1]),
                        "Stage {name}: sources must list subtitles, speech or both, once each"
                    );
                    validate_tuning(&Screenshots::default(), &t.speech)?;
                }
                Stage::Screenshots(s) => validate_tuning(s, &Speech::default())?,
                Stage::Llm(l) => {
                    if l.reads == "paragraphs" {
                        anyhow::ensure!(
                            (1..=100).contains(&l.batch_size.unwrap_or(DEFAULT_BATCH_SIZE)),
                            "Stage {name}: batch_size must be 1 to 100"
                        );
                    } else {
                        anyhow::ensure!(
                            (1000..=1_000_000)
                                .contains(&l.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE)),
                            "Stage {name}: chunk_size must be 1000 to 1000000"
                        );
                    }
                }
                Stage::Download(p) | Stage::Filter(p) => anyhow::ensure!(
                    !p.command.trim().is_empty(),
                    "The adapter command must name an executable."
                ),
                Stage::Note(_) => {}
            }
            if let Some(ai) = stage.ai(name) {
                crate::llm::validate_request(&ai.request)?;
            }
        }
        Ok(())
    }
}

fn validate_tuning(s: &Screenshots, speech: &Speech) -> Result<()> {
    if let Some(region) = &s.region {
        crate::config::Roi::parse(region)?;
    }
    // Reuse the engine's numeric validation for active and inactive stage tables.
    crate::config::validate_tuning(
        s.sensitivity,
        s.frame_interval,
        s.min_gap,
        s.settle,
        speech.max_segment,
        speech.threshold,
        speech.min_silence,
    )
}
pub fn config_path() -> PathBuf {
    crate::config::config_dir().join("config.toml")
}
pub fn load() -> Result<ConfigFile> {
    let path = config_path();
    if !path.is_file() {
        return parse(template());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("Cannot read configuration: {}", path.display()))?;
    parse(&text)
        .with_context(|| format!("Fix the configuration syntax and retry: {}", path.display()))
}
pub fn load_from(path: Option<&Path>) -> Result<ConfigFile> {
    let Some(path) = path else {
        return load();
    };
    let text = if path == Path::new("-") {
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
pub fn parse(text: &str) -> Result<ConfigFile> {
    // Parse errors can quote credentials from the input; report only the format.
    let mut cfg: ConfigFile = if text.trim_start().starts_with('{') {
        serde_json::from_str(text).map_err(|_| anyhow::anyhow!("Invalid JSON configuration"))?
    } else {
        toml::from_str(text).map_err(|_| anyhow::anyhow!("Invalid TOML configuration"))?
    };
    // Fill only the selected mode's defaults, preserving wrong-mode keys for validation.
    for stage in cfg.stages.values_mut() {
        if let Stage::Llm(l) = stage {
            match l.reads.as_str() {
                "paragraphs" => {
                    l.images.get_or_insert(false);
                    l.batch_size.get_or_insert(DEFAULT_BATCH_SIZE);
                }
                "transcript" => {
                    l.chunk_size.get_or_insert(DEFAULT_CHUNK_SIZE);
                }
                _ => {}
            }
        }
    }
    Ok(cfg)
}
pub fn template() -> &'static str {
    include_str!("config.toml")
}
pub fn print_effective(cfg: &ConfigFile) -> Result<()> {
    cfg.validate()?;
    println!("Configuration file: {}", config_path().display());
    let mut effective = cfg.clone();
    for provider in effective.providers.values_mut() {
        if provider.command.is_some() && provider.args.is_none() {
            provider.args = Some(Vec::new());
        }
    }
    print!("{}", toml::to_string_pretty(&effective)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn readme_shows_the_generated_file_with_shortened_prompts() {
        let mut in_prompt = false;
        let shortened: String = template()
            .lines()
            .filter_map(|line| {
                if in_prompt {
                    in_prompt = line != "'''";
                    return None;
                }
                in_prompt = line == "prompt = '''";
                Some(if in_prompt {
                    "prompt = '''…'''\n".into()
                } else {
                    format!("{line}\n")
                })
            })
            .collect();
        assert!(
            include_str!("../README.md")
                .replace("\r\n", "\n")
                .contains(&shortened)
        );
    }

    #[test]
    fn template_is_literal_valid_and_matches_tuning_defaults() {
        let cfg = parse(template()).unwrap();
        crate::options::resolve(String::new(), &Default::default(), &cfg).unwrap();
        let Stage::Transcript(t) = &cfg.stages["transcript"] else {
            panic!()
        };
        assert_eq!(t.language, Transcript::default().language);
        assert_eq!(t.sources, Transcript::default().sources);
        assert_eq!(t.subtitles, Subtitles::default());
        assert_eq!(t.speech.threshold, Speech::default().threshold);
        assert_eq!(t.speech.min_silence, Speech::default().min_silence);
        assert_eq!(t.speech.max_segment, Speech::default().max_segment);
        assert_eq!(
            cfg.stages["screenshots"],
            Stage::Screenshots(Screenshots::default())
        );
        let Stage::Llm(p) = &cfg.stages["proofread"] else {
            panic!()
        };
        assert_eq!(p.batch_size, Some(20));
        let Stage::Llm(top) = &cfg.stages["summary"] else {
            panic!()
        };
        assert_eq!(top.chunk_size, Some(25_000));
        assert_eq!(
            cfg.providers["openai"].concurrency,
            Provider::default().concurrency
        );
        cfg.validate().unwrap();
        assert!(!cfg.stages["download"].keep());
        let uncommented = template()
            .lines()
            .map(|line| {
                match line
                    .strip_prefix("# ")
                    .and_then(|text| text.split_once(" = "))
                {
                    Some((key, value))
                        if key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') =>
                    {
                        if key == "keep" {
                            assert_eq!(value, "false");
                        }
                        &line[2..]
                    }
                    _ => line,
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let explicit = parse(&uncommented).unwrap();
        explicit.validate().unwrap();
        assert_eq!(explicit, cfg);
        for definition in [
            json!({"type":"download","command":"adapter"}),
            json!({"type":"transcript"}),
            json!({"type":"screenshots"}),
            json!({"type":"llm","reads":"paragraphs"}),
            json!({"type":"llm","reads":"transcript"}),
            json!({"type":"filter","command":"adapter"}),
        ] {
            let stage: Stage = serde_json::from_value(definition).unwrap();
            assert!(!stage.keep());
        }
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        assert_eq!(parse(&serialized).unwrap(), cfg);
        assert_eq!(parse(&serde_json::to_string(&cfg).unwrap()).unwrap(), cfg);
    }

    #[test]
    fn provider_errors_are_exact() {
        for (value, expected) in [
            (
                json!({"base_url":"http://localhost:1", "command":"adapter"}),
                "Provider test: set base_url or command, not both",
            ),
            (json!({}), "Provider test: set base_url or command"),
            (
                json!({"base_url":"http://localhost:1", "args":[]}),
                "Provider test: args need a command",
            ),
            (
                json!({"base_url":"https://example.test/v1/chat/completions/"}),
                "Provider test: base_url must be the API root, such as https://api.openai.com/v1",
            ),
            (
                json!({"base_url":"https://example.test/v1/audio/transcriptions"}),
                "Provider test: base_url must be the API root, such as https://api.openai.com/v1",
            ),
            (
                json!({"command":"adapter", "concurrency":0}),
                "Provider test: concurrency must be 1 to 16",
            ),
            (
                json!({"command":"adapter", "concurrency":17}),
                "Provider test: concurrency must be 1 to 16",
            ),
        ] {
            let provider: Provider = serde_json::from_value(value).unwrap();
            assert_eq!(provider.validate("test").unwrap_err().to_string(), expected);
        }
        for base in ["not a URL", "file:///tmp/model", "https://"] {
            let provider: Provider = serde_json::from_value(json!({"base_url":base})).unwrap();
            assert!(provider.validate("test").is_err());
        }
    }

    fn minimal() -> Value {
        json!({"pipeline":["transcript","note"], "providers":{"p":{"base_url":"http://localhost:1/v1"}}, "stages":{"transcript":{"type":"transcript","sources":["subtitles"]},"note":{"type":"note"}}})
    }
    fn error(value: Value) -> String {
        let cfg: ConfigFile = serde_json::from_value(value).unwrap();
        crate::options::resolve(String::new(), &Default::default(), &cfg)
            .unwrap_err()
            .to_string()
    }
    #[test]
    fn stage_validation_includes_inactive_tables() {
        for (stage, expected) in [
            (
                json!({"type":"llm","reads":"paragraphs"}),
                "Stage test needs a provider",
            ),
            (
                json!({"type":"llm","reads":"transcript","provider":"missing"}),
                "Stage test: no provider named missing in [providers]",
            ),
            (
                json!({"type":"transcript","speech":{"provider":"missing"}}),
                "Stage test: no provider named missing in [providers]",
            ),
            (
                json!({"type":"llm","reads":"paragraphs","provider":"p","batch_size":0}),
                "Stage test: batch_size must be 1 to 100",
            ),
            (
                json!({"type":"llm","reads":"paragraphs","provider":"p","batch_size":101}),
                "Stage test: batch_size must be 1 to 100",
            ),
            (
                json!({"type":"llm","reads":"transcript","provider":"p","chunk_size":999}),
                "Stage test: chunk_size must be 1000 to 1000000",
            ),
            (
                json!({"type":"llm","reads":"transcript","provider":"p","chunk_size":1000001}),
                "Stage test: chunk_size must be 1000 to 1000000",
            ),
            (
                json!({"type":"transcript","sources":[]}),
                "Stage test: sources must list subtitles, speech or both, once each",
            ),
            (
                json!({"type":"transcript","sources":["subtitles","subtitles"]}),
                "Stage test: sources must list subtitles, speech or both, once each",
            ),
            (
                json!({"type":"transcript","sources":["unknown"]}),
                "Stage test: sources must list subtitles, speech or both, once each",
            ),
        ] {
            let mut cfg = minimal();
            cfg["stages"]["test"] = stage;
            assert_eq!(error(cfg), expected);
        }
        for stage in [
            json!({"type":"screenshots","sensitivity":0}),
            json!({"type":"transcript","sources":["subtitles"],"speech":{"threshold":2}}),
        ] {
            let mut cfg = minimal();
            cfg["stages"]["test"] = stage;
            let cfg: ConfigFile = serde_json::from_value(cfg).unwrap();
            assert!(cfg.validate().is_err());
        }
    }
    #[test]
    fn llm_reads_and_mode_specific_keys_are_validated() {
        for reads in [
            None,
            Some(json!("")),
            Some(json!("other")),
            Some(json!(false)),
            Some(json!(1)),
            Some(json!({})),
            Some(json!(null)),
        ] {
            let mut cfg = minimal();
            cfg["stages"]["test"] = json!({"type":"llm","provider":"p"});
            if let Some(reads) = reads {
                cfg["stages"]["test"]["reads"] = reads;
            }
            assert_eq!(
                error(cfg),
                "Stage test: reads must be \"paragraphs\" or \"transcript\""
            );
        }
        for (key, value, mode, wrong) in [
            ("images", json!(false), "paragraphs", "transcript"),
            ("batch_size", json!(20), "paragraphs", "transcript"),
            ("chunk_size", json!(25000), "transcript", "paragraphs"),
        ] {
            let mut cfg = minimal();
            cfg["stages"]["test"] = json!({"type":"llm","reads":wrong,"provider":"p"});
            cfg["stages"]["test"][key] = value;
            assert_eq!(
                error(cfg),
                format!("Stage test: {key} applies only when reads = \"{mode}\"")
            );
        }
    }
    #[test]
    fn stage_names_are_validated_in_inactive_tables_too() {
        for name in ["", "../bad", "a.b", "a b", "a/b", "a\\b", "한글"] {
            let mut cfg = minimal();
            cfg["stages"][name] = json!({"type":"note"});
            assert_eq!(
                error(cfg),
                format!("Stage names may use only letters, digits, \"-\" and \"_\": {name}")
            );
        }
        let mut cfg = minimal();
        cfg["stages"]["A-z_09"] = json!({"type":"note"});
        serde_json::from_value::<ConfigFile>(cfg)
            .unwrap()
            .validate()
            .unwrap();
    }
    #[test]
    fn pipeline_rules_and_skip_are_validated_on_the_effective_pipeline() {
        for (names, expected) in [
            (
                vec!["missing", "note"],
                "pipeline: no stage named missing in [stages]",
            ),
            (
                vec!["transcript", "transcript", "note"],
                "pipeline: transcript is listed twice",
            ),
            (
                vec!["note", "transcript"],
                "pipeline: note must come after transcript",
            ),
            (
                vec!["transcript"],
                "pipeline: exactly one note stage must run",
            ),
            (
                vec!["note"],
                "pipeline: a transcript stage must run, or pass --subtitle",
            ),
        ] {
            let mut cfg = minimal();
            cfg["pipeline"] = json!(names);
            assert_eq!(error(cfg), expected);
        }
        let mut cfg = minimal();
        cfg["stages"]["last"] = json!({"type":"filter","command":"adapter"});
        cfg["pipeline"] = json!(["transcript", "note", "last"]);
        assert_eq!(error(cfg), "pipeline: note must come after last");
        for (kind, definition, names) in [
            (
                "download",
                json!({"type":"download","command":"adapter"}),
                vec!["one", "two", "transcript", "note"],
            ),
            (
                "transcript",
                json!({"type":"transcript","sources":["subtitles"]}),
                vec!["one", "two", "note"],
            ),
            (
                "screenshots",
                json!({"type":"screenshots"}),
                vec!["transcript", "one", "two", "note"],
            ),
            (
                "note",
                json!({"type":"note"}),
                vec!["transcript", "one", "two"],
            ),
        ] {
            let mut cfg = minimal();
            cfg["pipeline"] = json!(names);
            cfg["stages"]["one"] = definition.clone();
            cfg["stages"]["two"] = definition;
            assert_eq!(
                error(cfg),
                if kind == "note" {
                    "pipeline: exactly one note stage must run".into()
                } else {
                    format!("pipeline: only one {kind} stage can run")
                }
            );
        }
        let cfg = ConfigFile::default();
        let opts = crate::cli::RunOpts {
            skip: vec!["proofread".into(), "summary".into()],
            ..Default::default()
        };
        let resolved = crate::options::resolve(String::new(), &opts, &cfg).unwrap();
        assert!(
            !resolved
                .stages
                .iter()
                .any(|(name, _)| matches!(name.as_str(), "proofread" | "summary"))
        );
        for (skip, expected) in [
            ("note", "pipeline: exactly one note stage must run"),
            ("unknown", "--skip: no stage named unknown in the pipeline"),
        ] {
            let opts = crate::cli::RunOpts {
                skip: vec![skip.into()],
                ..Default::default()
            };
            assert_eq!(
                crate::options::resolve(String::new(), &opts, &cfg)
                    .unwrap_err()
                    .to_string(),
                expected
            );
        }
        let mut cfg: ConfigFile = serde_json::from_value(minimal()).unwrap();
        cfg.pipeline = vec!["note".into()];
        crate::options::resolve(
            String::new(),
            &crate::cli::RunOpts {
                subtitle: Some("test.srt".into()),
                ..Default::default()
            },
            &cfg,
        )
        .unwrap();
    }
    #[test]
    fn unknown_keys_and_other_stage_types_keys_are_rejected() {
        for kind in [
            "download",
            "transcript",
            "screenshots",
            "llm",
            "filter",
            "note",
        ] {
            let mut cfg = minimal();
            cfg["stages"]["bad"] = json!({"type":kind,"nonsense":true,"command":"adapter"});
            assert!(serde_json::from_value::<ConfigFile>(cfg).is_err(), "{kind}");
        }
        for field in [
            "output",
            "transcript",
            "speech",
            "screenshots",
            "download",
            "services",
            "unknown",
        ] {
            let mut cfg = minimal();
            cfg[field] = json!({});
            assert!(serde_json::from_value::<ConfigFile>(cfg).is_err());
        }
        for field in ["api_key", "adapter", "unknown"] {
            let mut cfg = minimal();
            cfg["providers"]["p"][field] = json!("secret");
            assert!(serde_json::from_value::<ConfigFile>(cfg).is_err());
        }
        for field in ["subtitles", "speech"] {
            let mut cfg = minimal();
            cfg["stages"]["transcript"][field] = json!({"unknown":true});
            assert!(serde_json::from_value::<ConfigFile>(cfg).is_err());
        }
        assert!(serde_json::from_value::<Stage>(json!({"type":"note","keep":true})).is_err());
        let error = serde_json::from_value::<Stage>(json!({"type":"unknown"}))
            .unwrap_err()
            .to_string();
        assert!(error.contains(
            "expected one of `download`, `transcript`, `screenshots`, `llm`, `filter`, `note`"
        ));
        assert!(
            serde_json::from_value::<Stage>(
                json!({"type":"llm","reads":"paragraphs","input":"paragraphs"})
            )
            .is_err()
        );
        for key in ["images", "batch_size", "chunk_size"] {
            let mut stage = json!({"type":"llm","reads":"paragraphs"});
            stage[key] = json!(null);
            assert!(serde_json::from_value::<Stage>(stage).is_err());
        }
        assert!(parse("{}").is_err());
    }
}
