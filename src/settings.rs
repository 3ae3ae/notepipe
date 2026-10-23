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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asr {
    #[serde(skip)]
    pub limit: Option<std::sync::Arc<crate::runtime::RequestLimit>>,
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
            limit: None,
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
#[serde(deny_unknown_fields)]
pub struct Source {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub command: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Provider {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    pub api_key_env: String,
    pub concurrency: i64,
}
impl Default for Provider {
    fn default() -> Self {
        Self {
            base_url: None,
            command: None,
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
            validate_command(command)?;
        }
        Ok(())
    }
    pub fn argv(&self) -> Vec<String> {
        self.command.clone().unwrap_or_default()
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
fn validate_command(command: &[String]) -> Result<()> {
    anyhow::ensure!(
        command.first().is_some_and(|p| !p.trim().is_empty()),
        "The adapter command must name an executable."
    );
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Subtitles {
    pub automatic: bool,
}
impl Default for Subtitles {
    fn default() -> Self {
        Self { automatic: true }
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
const DEFAULT_BATCH_SIZE: i64 = 20;
const DEFAULT_CHUNK_SIZE: i64 = 25_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Llm {
    pub provider: String,
    pub model: String,
    pub prompt: String,
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub images: Option<bool>,
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub batch_size: Option<i64>,
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub chunk_size: Option<i64>,
    pub request: serde_json::Map<String, serde_json::Value>,
}
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produces: Option<Kind>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum StageType {
    Download(Source),
    Transcript(Transcript),
    Screenshots(Screenshots),
    Document {},
    Llm(Llm),
    Markdown {},
    Run(Run),
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Media,
    Transcript,
    Images,
    Document,
    Markdown,
    File,
}
impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Media => "media",
            Self::Transcript => "transcript",
            Self::Images => "images",
            Self::Document => "document",
            Self::Markdown => "markdown",
            Self::File => "file",
        })
    }
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Stage {
    pub input: Vec<String>,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish: Option<String>,
    #[serde(flatten)]
    pub kind: StageType,
}
impl<'de> Deserialize<'de> for Stage {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error;
        let mut value = serde_json::Map::<String, serde_json::Value>::deserialize(d)?;
        let input = match value.remove("input") {
            None => Vec::new(),
            Some(serde_json::Value::String(input)) => vec![input],
            Some(input) => serde_json::from_value(input).map_err(D::Error::custom)?,
        };
        let output = serde_json::from_value(
            value
                .remove("output")
                .ok_or_else(|| D::Error::missing_field("output"))?,
        )
        .map_err(D::Error::custom)?;
        let publish = value
            .remove("publish")
            .map(serde_json::from_value)
            .transpose()
            .map_err(D::Error::custom)?;
        let kind = serde_json::from_value(value.into()).map_err(D::Error::custom)?;
        Ok(Self {
            input,
            output,
            publish,
            kind,
        })
    }
}
pub fn plain_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && name != "source" && !name.contains(['/', '\\'])
}
impl Stage {
    pub fn output_kind(&self) -> Kind {
        match &self.kind {
            StageType::Download(_) => Kind::Media,
            StageType::Transcript(_) => Kind::Transcript,
            StageType::Screenshots(_) => Kind::Images,
            StageType::Document {} => Kind::Document,
            StageType::Llm(_) if self.output.ends_with(".json") => Kind::Document,
            StageType::Llm(_) | StageType::Markdown {} => Kind::Markdown,
            StageType::Run(r) => r.produces.unwrap_or(Kind::File),
        }
    }
    pub fn used_provider(&self) -> Option<&str> {
        match &self.kind {
            StageType::Transcript(t) if t.sources.iter().any(|s| s == "speech") => {
                Some(&t.speech.provider)
            }
            StageType::Llm(l) => Some(&l.provider),
            _ => None,
        }
    }
    pub fn ai(&self, name: &str) -> Option<crate::llm::Stage> {
        let StageType::Llm(l) = &self.kind else {
            return None;
        };
        Some(crate::llm::Stage {
            name: name.into(),
            provider: l.provider.clone(),
            model: l.model.clone(),
            prompt: l.prompt.trim().into(),
            images: l.images.unwrap_or(false),
            batch_size: l.batch_size.unwrap_or(DEFAULT_BATCH_SIZE) as usize,
            chunk_size: l.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE) as usize,
            request: l.request.clone(),
            input: if self.output.ends_with(".json") {
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
    #[serde(default = "default_out")]
    pub out: PathBuf,
    #[serde(default)]
    pub providers: BTreeMap<String, Provider>,
    #[serde(default)]
    pub stages: BTreeMap<String, Stage>,
}
fn default_out() -> PathBuf {
    crate::config::DEFAULT_OUT_DIR.into()
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
        let mut outputs = BTreeMap::new();
        let mut published = BTreeMap::new();
        for (name, stage) in &self.stages {
            anyhow::ensure!(
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_')),
                "Stage names may use only letters, digits, \"-\" and \"_\": {name}"
            );
            anyhow::ensure!(
                plain_name(&stage.output),
                "Stage {name}: output must be a plain name in the work folder, such as transcript.json"
            );
            if let Some(a) = outputs.insert(stage.output.to_lowercase(), name) {
                anyhow::bail!("Stages {a} and {name} both write {}", stage.output);
            }
            if let Some(publish) = &stage.publish {
                anyhow::ensure!(
                    publish == "-" || plain_name(publish),
                    "Stage {name}: publish must be a plain file name in the note folder, or - for standard output"
                );
                if let Some(a) = published.insert(publish.to_lowercase(), name) {
                    anyhow::ensure!(
                        publish != "-",
                        "Only one stage can publish to standard output"
                    );
                    anyhow::bail!("Stages {a} and {name} both publish {publish}");
                }
                anyhow::ensure!(
                    !matches!(stage.kind, StageType::Download(_)),
                    "Stage {name}: a download output cannot be published"
                );
            }
            if let StageType::Llm(l) = &stage.kind {
                anyhow::ensure!(
                    stage.output.ends_with(".json") || stage.output.ends_with(".md"),
                    "Stage {name}: an llm output ends in .json (rewrite each paragraph) or .md (one answer)"
                );
                for (key, present, ext) in [
                    ("images", l.images.is_some(), ".json"),
                    ("batch_size", l.batch_size.is_some(), ".json"),
                    ("chunk_size", l.chunk_size.is_some(), ".md"),
                ] {
                    anyhow::ensure!(
                        !present || stage.output.ends_with(ext),
                        "Stage {name}: {key} applies only to an llm output ending in {ext}"
                    );
                }
                anyhow::ensure!(
                    (1..=100).contains(&l.batch_size.unwrap_or(DEFAULT_BATCH_SIZE)),
                    "Stage {name}: batch_size must be 1 to 100"
                );
                anyhow::ensure!(
                    (1000..=1_000_000).contains(&l.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE)),
                    "Stage {name}: chunk_size must be 1000 to 1000000"
                );
                crate::llm::validate_request(&l.request)?;
            }
            let provider = match &stage.kind {
                StageType::Transcript(t) if !t.speech.provider.is_empty() => {
                    Some(t.speech.provider.as_str())
                }
                _ => stage.used_provider(),
            };
            if let Some(provider) = provider {
                anyhow::ensure!(!provider.is_empty(), "Stage {name} needs a provider");
                anyhow::ensure!(
                    self.providers.contains_key(provider),
                    "Stage {name}: no provider named {provider} in [providers]"
                );
            }
            match &stage.kind {
                StageType::Transcript(t) => {
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
                StageType::Screenshots(s) => validate_tuning(s, &Speech::default())?,
                StageType::Download(p) => {
                    if let Some(command) = &p.command {
                        validate_command(command)?;
                    }
                    anyhow::ensure!(
                        stage.input == ["source"],
                        "Stage {name}: a download stage reads source"
                    );
                }
                StageType::Run(p) => {
                    validate_command(&p.command)?;
                    anyhow::ensure!(
                        p.produces.is_none_or(|k| matches!(
                            k,
                            Kind::Transcript | Kind::Document | Kind::Markdown
                        )),
                        "Stage {name}: produces must be transcript, document or markdown"
                    );
                    for arg in &p.command {
                        for placeholder in ["{input}", "{output}"] {
                            anyhow::ensure!(
                                !arg.contains(placeholder) || arg == placeholder,
                                "Stage {name}: a {placeholder} placeholder must be a whole argument"
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}
fn validate_tuning(s: &Screenshots, speech: &Speech) -> Result<()> {
    if let Some(region) = &s.region {
        crate::config::Roi::parse(region)?;
    }
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
    load_from(Some(&path))
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
    parse_at(&text, path)
}
pub fn parse(text: &str) -> Result<ConfigFile> {
    parse_at(text, Path::new("-"))
}
fn parse_at(text: &str, path: &Path) -> Result<ConfigFile> {
    let mut cfg: ConfigFile = if text.trim_start().starts_with('{') {
        serde_json::from_str(text).map_err(|error: serde_json::Error| {
            let message = error.to_string();
            let suffix = format!(" at line {} column {}", error.line(), error.column());
            let message = message.strip_suffix(&suffix).unwrap_or(&message);
            anyhow::anyhow!(
                "Fix the configuration and retry: {}: line {}: {message}",
                path.display(),
                error.line()
            )
        })?
    } else {
        toml::from_str(text).map_err(|error: toml::de::Error| {
            let line = error.span().map_or(1, |span| {
                text.as_bytes()[..span.start]
                    .iter()
                    .filter(|b| **b == b'\n')
                    .count()
                    + 1
            });
            anyhow::anyhow!(
                "Fix the configuration and retry: {}: line {line}: {}",
                path.display(),
                error.message()
            )
        })?
    };
    for stage in cfg.stages.values_mut() {
        if let StageType::Llm(l) = &mut stage.kind {
            if stage.output.ends_with(".json") {
                l.images.get_or_insert(false);
                l.batch_size.get_or_insert(DEFAULT_BATCH_SIZE);
            } else if stage.output.ends_with(".md") {
                l.chunk_size.get_or_insert(DEFAULT_CHUNK_SIZE);
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
    print!("{}", toml::to_string_pretty(cfg)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    fn resolve(value: &Value, skip: &[&str]) -> Result<crate::config::PipelineConfig> {
        let cfg = parse(&value.to_string())?;
        crate::options::resolve(
            "local".into(),
            &crate::cli::RunOpts {
                skip: skip.iter().map(|s| (*s).into()).collect(),
                ..Default::default()
            },
            &cfg,
        )
    }
    fn config() -> Value {
        serde_json::to_value(ConfigFile::default()).unwrap()
    }
    #[test]
    fn template_matches_commented_defaults_and_round_trips() {
        let cfg = parse(template()).unwrap();
        cfg.validate().unwrap();
        crate::options::resolve("local".into(), &Default::default(), &cfg).unwrap();
        let uncommented = template()
            .lines()
            .map(
                |line| match line.strip_prefix("# ").and_then(|s| s.split_once(" = ")) {
                    Some((key, _))
                        if key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') =>
                    {
                        &line[2..]
                    }
                    _ => line,
                },
            )
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(parse(&uncommented).unwrap(), cfg);
        assert_eq!(parse(&toml::to_string_pretty(&cfg).unwrap()).unwrap(), cfg);
        assert_eq!(parse(&serde_json::to_string(&cfg).unwrap()).unwrap(), cfg);
    }
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
    fn download_settings_allow_no_command_and_validate_present_commands() {
        assert!(Source::default().command.is_none());
        let settings: Source = serde_json::from_value(json!({})).unwrap();
        assert_eq!(settings, Source::default());
        assert_eq!(serde_json::to_value(settings).unwrap(), json!({}));
        for value in [
            json!({"command":null}),
            json!({"command":["unused"],"unknown":true}),
        ] {
            assert!(serde_json::from_value::<Source>(value).is_err());
        }
        assert!(
            serde_json::from_value::<Stage>(
                json!({"type":"download","input":"source","output":"media"})
            )
            .is_ok()
        );
        for command in [json!([]), json!([""]), json!(["   "])] {
            let mut cfg = config();
            cfg["stages"]["download"]["command"] = command;
            assert_eq!(
                resolve(&cfg, &[]).unwrap_err().to_string(),
                "The adapter command must name an executable."
            );
        }
        let value = json!({"command":["unused","argument"]});
        let settings: Source = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(settings).unwrap(), value);
    }
    #[test]
    fn output_and_publication_names_are_plain_and_unique() {
        for key in ["output", "publish"] {
            for name in ["", ".hidden", "../outside", "a/b", "a\\b", "source"] {
                let mut cfg = config();
                cfg["stages"]["summary"][key] = json!(name);
                let expected = if key == "output" {
                    "Stage summary: output must be a plain name in the work folder, such as transcript.json"
                } else {
                    "Stage summary: publish must be a plain file name in the note folder, or - for standard output"
                };
                assert_eq!(resolve(&cfg, &[]).unwrap_err().to_string(), expected);
            }
        }
        for (key, value, expected) in [
            (
                "output",
                "draft.json",
                "Stages draft and summary both write draft.json",
            ),
            (
                "publish",
                "note.json",
                "Stages proofread and summary both publish note.json",
            ),
        ] {
            let mut cfg = config();
            cfg["stages"]["summary"][key] = json!(value);
            assert_eq!(resolve(&cfg, &[]).unwrap_err().to_string(), expected);
        }
        let mut cfg = config();
        cfg["stages"]["summary"]["publish"] = json!("-");
        cfg["stages"]["proofread"]["publish"] = json!("-");
        assert_eq!(
            resolve(&cfg, &[]).unwrap_err().to_string(),
            "Only one stage can publish to standard output"
        );
        let mut cfg = config();
        cfg["stages"]["download"]["publish"] = json!("media");
        assert_eq!(
            resolve(&cfg, &[]).unwrap_err().to_string(),
            "Stage download: a download output cannot be published"
        );
    }
    #[test]
    fn output_and_publication_names_compare_unicode_lowercase() {
        for (key, verb) in [("output", "write"), ("publish", "publish")] {
            for (a, b) in [("A.md", "a.md"), ("Ä.md", "ä.md"), ("İ.md", "i\u{307}.md")] {
                let mut cfg = json!({"pipeline":["a","b"],"stages":{
                    "a":{"type":"run","output":"first","publish":"first","command":["unused"]},
                    "b":{"type":"run","output":"second","publish":"second","command":["unused"]}
                }});
                cfg["stages"]["a"][key] = json!(a);
                cfg["stages"]["b"][key] = json!(b);
                assert_eq!(
                    resolve(&cfg, &[]).unwrap_err().to_string(),
                    format!("Stages a and b both {verb} {b}")
                );
            }
        }
    }
    #[test]
    fn inputs_must_exist_earlier_and_have_an_accepted_kind() {
        for (input, expected) in [
            (
                "missing.json",
                "Stage transcript: no earlier stage writes missing.json",
            ),
            (
                "source",
                "Stage transcript: only a download stage reads source",
            ),
            (
                "draft.json",
                "Stage transcript: no earlier stage writes draft.json",
            ),
        ] {
            let mut cfg = config();
            cfg["stages"]["transcript"]["input"] = json!(input);
            assert_eq!(resolve(&cfg, &[]).unwrap_err().to_string(), expected);
        }
        for (stage, input, expected) in [
            (
                "summary",
                "media",
                "Stage summary reads media, which is media; a llm stage reads transcript or document or markdown",
            ),
            (
                "proofread",
                "transcript.json",
                "Stage proofread reads transcript.json, which is transcript; a llm stage reads document",
            ),
            (
                "markdown",
                "images",
                "Stage markdown reads images, which is images; a markdown stage reads document or markdown",
            ),
        ] {
            let mut cfg = config();
            cfg["stages"][stage]["input"] = json!(input);
            assert_eq!(resolve(&cfg, &[]).unwrap_err().to_string(), expected);
        }
    }
    #[test]
    fn llm_extension_and_mode_keys_are_checked() {
        let mut cfg = config();
        cfg["stages"]["proofread"]["output"] = json!("bad.txt");
        assert_eq!(
            resolve(&cfg, &[]).unwrap_err().to_string(),
            "Stage proofread: an llm output ends in .json (rewrite each paragraph) or .md (one answer)"
        );
        for (stage, key, value, ext) in [
            ("proofread", "chunk_size", json!(25000), ".md"),
            ("summary", "images", json!(false), ".json"),
            ("summary", "batch_size", json!(20), ".json"),
        ] {
            let mut cfg = config();
            cfg["stages"][stage][key] = value;
            assert_eq!(
                resolve(&cfg, &[]).unwrap_err().to_string(),
                format!("Stage {stage}: {key} applies only to an llm output ending in {ext}")
            );
        }
    }
    #[test]
    fn skips_pass_documents_through_omit_optional_inputs_and_reject_required_inputs() {
        let cfg = config();
        let resolved = resolve(&cfg, &["proofread", "summary", "screenshots"]).unwrap();
        assert_eq!(
            resolved
                .stages
                .iter()
                .find(|(n, _)| n == "draft")
                .unwrap()
                .1
                .input,
            ["transcript.json"]
        );
        assert_eq!(
            resolved
                .stages
                .iter()
                .find(|(n, _)| n == "markdown")
                .unwrap()
                .1
                .input,
            ["proofread.json"]
        );
        for (skip, expected) in [
            (
                "transcript",
                "--skip transcript: draft needs transcript.json",
            ),
            ("download", "--skip download: transcript needs media"),
            ("draft", "--skip draft: proofread needs draft.json"),
        ] {
            assert_eq!(resolve(&cfg, &[skip]).unwrap_err().to_string(), expected);
        }
        let mut cfg = config();
        cfg["stages"]["proofread"]
            .as_object_mut()
            .unwrap()
            .remove("publish");
        cfg["stages"]["markdown"]
            .as_object_mut()
            .unwrap()
            .remove("publish");
        assert_eq!(
            resolve(&cfg, &[]).unwrap_err().to_string(),
            "Nothing is published; set publish on a stage"
        );
    }
    #[test]
    fn run_placeholders_and_stdin_shape_are_validated() {
        for placeholder in ["{input}", "{output}"] {
            let mut cfg = config();
            cfg["stages"]["custom"] = json!({"type":"run","output":"file","command":["program",format!("prefix{placeholder}")]});
            assert_eq!(
                resolve(&cfg, &[]).unwrap_err().to_string(),
                format!("Stage custom: a {placeholder} placeholder must be a whole argument")
            );
        }
        for input in [
            json!(["transcript.json", "draft.json"]),
            json!("media"),
            json!("images"),
        ] {
            let mut cfg = config();
            cfg["pipeline"]
                .as_array_mut()
                .unwrap()
                .push(json!("custom"));
            cfg["stages"]["custom"] =
                json!({"type":"run","input":input,"output":"custom","command":["program"]});
            assert_eq!(
                resolve(&cfg, &[]).unwrap_err().to_string(),
                "Stage custom: standard input takes one file; use {input} in command for more"
            );
        }
    }
    #[test]
    fn unknown_keys_and_removed_keys_are_errors_everywhere() {
        for (table, key) in [
            ("", "unexpected"),
            ("providers.openai", "args"),
            ("stages.download", "args"),
            ("stages.proofread", "reads"),
            ("stages.summary", "keep"),
            ("stages.transcript.subtitles", "sidecar"),
            ("stages.transcript.speech", "unknown"),
            ("stages.markdown", "model"),
        ] {
            let mut cfg = config();
            let mut target = &mut cfg;
            for part in table.split('.').filter(|p| !p.is_empty()) {
                target = &mut target[part];
            }
            target[key] = json!("unknown");
            assert!(parse(&cfg.to_string()).is_err(), "{table}.{key}");
        }
    }
    #[test]
    fn cli_stdout_conflict_and_matching_stage_overrides_are_checked() {
        let mut cfg = ConfigFile::default();
        cfg.stages.get_mut("markdown").unwrap().publish = Some("-".into());
        let opts = crate::cli::RunOpts {
            json: true,
            ..Default::default()
        };
        assert_eq!(
            crate::options::resolve("local".into(), &opts, &cfg)
                .unwrap_err()
                .to_string(),
            "Publishing to standard output and --json both use standard output; use one of them"
        );
        let opts = crate::cli::RunOpts {
            asr_language: Some("ko".into()),
            similarity: Some(0.6),
            ..Default::default()
        };
        let resolved = crate::options::resolve("local".into(), &opts, &cfg).unwrap();
        let speech =
            crate::options::stage_config(&resolved, "transcript", &cfg.stages["transcript"])
                .unwrap();
        // Overrides live in resolved stages, not the original config.
        assert!(speech.asr.language.is_empty());
        let stage = &resolved
            .stages
            .iter()
            .find(|(n, _)| n == "transcript")
            .unwrap()
            .1;
        assert_eq!(
            crate::options::stage_config(&resolved, "transcript", stage)
                .unwrap()
                .asr
                .language,
            "ko"
        );
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn providers_and_inactive_tables_still_validate_their_settings() {
        for (provider, expected) in [
            (json!({}), "Provider p: set base_url or command"),
            (
                json!({"base_url":"http://localhost/v1","command":["tool"]}),
                "Provider p: set base_url or command, not both",
            ),
            (
                json!({"command":[]}),
                "The adapter command must name an executable.",
            ),
            (
                json!({"command":["tool"],"concurrency":0}),
                "Provider p: concurrency must be 1 to 16",
            ),
            (
                json!({"command":["tool"],"concurrency":17}),
                "Provider p: concurrency must be 1 to 16",
            ),
            (
                json!({"base_url":"http://localhost/v1/chat/completions"}),
                "Provider p: base_url must be the API root, such as https://api.openai.com/v1",
            ),
        ] {
            let mut cfg = ConfigFile::default();
            cfg.providers
                .insert("p".into(), serde_json::from_value(provider).unwrap());
            assert_eq!(cfg.validate().unwrap_err().to_string(), expected);
        }
        for (key, value) in [
            ("batch_size", json!(0)),
            ("batch_size", json!(101)),
            ("chunk_size", json!(999)),
            ("chunk_size", json!(1_000_001)),
            ("request", json!({"messages":[]})),
        ] {
            let mut cfg = serde_json::to_value(ConfigFile::default()).unwrap();
            let stage = if key == "batch_size" {
                "proofread"
            } else {
                "summary"
            };
            cfg["stages"][stage][key] = value;
            assert!(parse(&cfg.to_string()).unwrap().validate().is_err());
        }
        for name in ["", "../bad", "a.b", "a b", "a/b", "a\\b", "한글"] {
            let mut cfg = ConfigFile::default();
            let stage = cfg.stages["markdown"].clone();
            cfg.stages.insert(
                name.into(),
                Stage {
                    output: "unused".into(),
                    publish: None,
                    ..stage
                },
            );
            assert_eq!(
                cfg.validate().unwrap_err().to_string(),
                format!("Stage names may use only letters, digits, \"-\" and \"_\": {name}")
            );
        }
        for typ in ["filter", "note", "unknown"] {
            assert!(
                parse(
                    &json!({"pipeline":[],"stages":{"old":{"type":typ,"output":"old"}}})
                        .to_string()
                )
                .is_err()
            );
        }
        assert!(parse("{}").is_err());
    }
    #[test]
    fn input_errors_describe_each_stage_rule_and_empty_skip_result() {
        for (stage, input, rule) in [
            ("download", json!([]), "a download stage reads source"),
            (
                "transcript",
                json!([]),
                "a transcript stage reads one input",
            ),
            (
                "transcript",
                json!(["media", "media"]),
                "a transcript stage reads one input",
            ),
            (
                "screenshots",
                json!([]),
                "a screenshots stage reads one input",
            ),
            (
                "screenshots",
                json!(["media", "media"]),
                "a screenshots stage reads one input",
            ),
            ("proofread", json!([]), "a llm stage reads one input"),
            (
                "summary",
                json!(["draft.json", "draft.json"]),
                "a llm stage reads one input",
            ),
            (
                "draft",
                json!([]),
                "a document stage reads one transcript and at most one images directory",
            ),
            (
                "draft",
                json!(["transcript.json", "transcript.json"]),
                "a document stage reads one transcript and at most one images directory",
            ),
            (
                "draft",
                json!(["transcript.json", "images", "images"]),
                "a document stage reads one transcript and at most one images directory",
            ),
            (
                "markdown",
                json!([]),
                "a markdown stage reads at least one input",
            ),
        ] {
            let mut cfg = serde_json::to_value(ConfigFile::default()).unwrap();
            cfg["stages"][stage]["input"] = input;
            let cfg = parse(&cfg.to_string()).unwrap();
            assert_eq!(
                crate::options::resolve("local".into(), &Default::default(), &cfg)
                    .unwrap_err()
                    .to_string(),
                format!("Stage {stage}: {rule}")
            );
        }
        let mut cfg = ConfigFile::default();
        cfg.stages.get_mut("markdown").unwrap().input = vec!["summary.md".into()];
        let opts = crate::cli::RunOpts {
            skip: vec!["summary".into()],
            ..Default::default()
        };
        assert_eq!(
            crate::options::resolve("local".into(), &opts, &cfg)
                .unwrap_err()
                .to_string(),
            "--skip leaves stage markdown with no input"
        );
    }
    #[test]
    fn pipeline_names_cardinalities_and_outputs_are_validated() {
        let mut cfg = ConfigFile::default();
        cfg.stages.get_mut("download").unwrap().input = vec!["other".into()];
        assert_eq!(
            cfg.validate().unwrap_err().to_string(),
            "Stage download: a download stage reads source"
        );

        for (pipeline, expected) in [
            (
                vec!["missing"],
                "pipeline: no stage named missing in [stages]",
            ),
            (
                vec!["download", "download"],
                "pipeline: download is listed twice",
            ),
        ] {
            let cfg = ConfigFile {
                pipeline: pipeline.into_iter().map(str::to_owned).collect(),
                ..Default::default()
            };
            assert_eq!(
                crate::options::resolve("local".into(), &Default::default(), &cfg)
                    .unwrap_err()
                    .to_string(),
                expected
            );
        }
        for input in [json!([]), json!(["proofread.json", "proofread.json"])] {
            let mut cfg = serde_json::to_value(ConfigFile::default()).unwrap();
            cfg["stages"]["summary"]["input"] = input;
            let cfg = parse(&cfg.to_string()).unwrap();
            assert_eq!(
                crate::options::resolve("local".into(), &Default::default(), &cfg)
                    .unwrap_err()
                    .to_string(),
                "Stage summary: a llm stage reads one input"
            );
        }
        let mut cfg = serde_json::to_value(ConfigFile::default()).unwrap();
        cfg["stages"]["custom"] =
            json!({"type":"run","output":"custom","produces":"images","command":["tool"]});
        assert_eq!(
            parse(&cfg.to_string())
                .unwrap()
                .validate()
                .unwrap_err()
                .to_string(),
            "Stage custom: produces must be transcript, document or markdown"
        );
    }
}
