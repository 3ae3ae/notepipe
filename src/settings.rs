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
    pub language: String,
    pub sources: Vec<String>,
    pub subtitles: Subtitles,
    pub speech: Speech,
}
impl Default for Transcript {
    fn default() -> Self {
        Self {
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
            mode: Default::default(),
            sensitivity: crate::config::DEFAULT_SIMILARITY,
            frame_interval: crate::config::DEFAULT_SAMPLE_INTERVAL,
            min_gap: crate::config::DEFAULT_COOLDOWN,
            settle: crate::config::DEFAULT_STABLE_SECS,
            region: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Paragraphs {
    pub provider: String,
    pub model: String,
    pub prompt: String,
    pub images: bool,
    pub batch_size: i64,
    pub request: serde_json::Map<String, serde_json::Value>,
}
impl Default for Paragraphs {
    fn default() -> Self {
        Self {
            provider: String::new(),
            model: String::new(),
            prompt: String::new(),
            images: false,
            batch_size: 20,
            request: Default::default(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Top {
    pub provider: String,
    pub model: String,
    pub prompt: String,
    pub chunk_size: i64,
    pub request: serde_json::Map<String, serde_json::Value>,
}
impl Default for Top {
    fn default() -> Self {
        Self {
            provider: String::new(),
            model: String::new(),
            prompt: String::new(),
            chunk_size: 25_000,
            request: Default::default(),
        }
    }
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
    Rewrite(Paragraphs),
    AddBelow(Paragraphs),
    AddTop(Top),
    Filter(Program),
    Note(Note),
    Hook(Program),
}
impl Stage {
    pub fn rank(&self) -> u8 {
        match self {
            Self::Download(_) => 0,
            Self::Transcript(_) | Self::Screenshots(_) => 1,
            Self::Rewrite(_) | Self::AddBelow(_) | Self::AddTop(_) | Self::Filter(_) => 2,
            Self::Note(_) => 3,
            Self::Hook(_) => 4,
        }
    }
    pub fn provider(&self) -> Option<&str> {
        match self {
            Self::Transcript(t)
                if t.sources.iter().any(|s| s == "speech") || !t.speech.provider.is_empty() =>
            {
                Some(&t.speech.provider)
            }
            Self::Rewrite(p) | Self::AddBelow(p) => Some(&p.provider),
            Self::AddTop(t) => Some(&t.provider),
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
    pub fn ai(&self, name: &str) -> Option<crate::llm::Stage> {
        use crate::llm::{Input, Output};
        let (provider, model, prompt, images, batch_size, chunk_size, request, input, output) =
            match self {
                Self::Rewrite(p) | Self::AddBelow(p) => (
                    &p.provider,
                    &p.model,
                    &p.prompt,
                    p.images,
                    p.batch_size as usize,
                    25_000,
                    &p.request,
                    Input::Paragraphs,
                    if matches!(self, Self::Rewrite(_)) {
                        Output::Replace
                    } else {
                        Output::Below
                    },
                ),
                Self::AddTop(t) => (
                    &t.provider,
                    &t.model,
                    &t.prompt,
                    false,
                    20,
                    t.chunk_size as usize,
                    &t.request,
                    Input::Transcript,
                    Output::Top,
                ),
                _ => return None,
            };
        Some(crate::llm::Stage {
            name: name.into(),
            provider: provider.clone(),
            model: model.clone(),
            prompt: prompt.trim().into(),
            images,
            batch_size,
            chunk_size,
            request: request.clone(),
            input,
            output,
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
                Stage::Rewrite(p) | Stage::AddBelow(p) => anyhow::ensure!(
                    (1..=100).contains(&p.batch_size),
                    "Stage {name}: batch_size must be 1 to 100"
                ),
                Stage::AddTop(t) => anyhow::ensure!(
                    (1000..=1_000_000).contains(&t.chunk_size),
                    "Stage {name}: chunk_size must be 1000 to 1000000"
                ),
                Stage::Download(p) | Stage::Filter(p) | Stage::Hook(p) => anyhow::ensure!(
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
    if text.trim_start().starts_with('{') {
        serde_json::from_str(text).map_err(|_| anyhow::anyhow!("Invalid JSON configuration"))
    } else {
        toml::from_str(text).map_err(|_| anyhow::anyhow!("Invalid TOML configuration"))
    }
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
        let Stage::Rewrite(p) = &cfg.stages["proofread"] else {
            panic!()
        };
        assert_eq!(p.batch_size, Paragraphs::default().batch_size);
        let Stage::AddTop(top) = &cfg.stages["summary"] else {
            panic!()
        };
        assert_eq!(top.chunk_size, Top::default().chunk_size);
        assert_eq!(
            cfg.providers["openai"].concurrency,
            Provider::default().concurrency
        );
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
            (json!({"type":"rewrite"}), "Stage test needs a provider"),
            (
                json!({"type":"add-top","provider":"missing"}),
                "Stage test: no provider named missing in [providers]",
            ),
            (
                json!({"type":"transcript","speech":{"provider":"missing"}}),
                "Stage test: no provider named missing in [providers]",
            ),
            (
                json!({"type":"rewrite","provider":"p","batch_size":0}),
                "Stage test: batch_size must be 1 to 100",
            ),
            (
                json!({"type":"add-below","provider":"p","batch_size":101}),
                "Stage test: batch_size must be 1 to 100",
            ),
            (
                json!({"type":"add-top","provider":"p","chunk_size":999}),
                "Stage test: chunk_size must be 1000 to 1000000",
            ),
            (
                json!({"type":"add-top","provider":"p","chunk_size":1000001}),
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
            "rewrite",
            "add-below",
            "add-top",
            "filter",
            "note",
            "hook",
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
        for stage in [
            json!({"type":"add-top","images":true}),
            json!({"type":"rewrite","chunk_size":1000}),
            json!({"type":"add-top","batch_size":1}),
            json!({"type":"rewrite","input":"paragraphs"}),
        ] {
            assert!(serde_json::from_value::<Stage>(stage).is_err());
        }
        assert!(parse("{}").is_err());
    }
}
