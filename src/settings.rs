//! The configuration file (~/.config/notepipe/config.toml).
//! Precedence: command-line options, then the file, then built-in defaults.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Output {
    pub dir: Option<PathBuf>,
    pub on_complete: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Transcript {
    pub source: Option<crate::config::TranscriptSource>,
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
pub struct Speech {
    #[serde(rename = "api")]
    pub mode: AsrMode,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub concurrency: usize,
    #[serde(rename = "adapter")]
    pub command: Vec<String>,
    pub detection: Detection,
}

impl Default for Speech {
    fn default() -> Self {
        Self::from(&Asr::default())
    }
}

impl From<&Asr> for Speech {
    fn from(asr: &Asr) -> Self {
        Self {
            mode: asr.mode,
            base_url: asr.base_url.clone(),
            api_key: asr.api_key.clone(),
            model: asr.model.clone(),
            concurrency: asr.concurrency,
            command: asr.command.clone(),
            detection: Detection {
                threshold: asr.vad_threshold,
                min_silence: asr.min_silence,
                max_segment: asr.max_speech,
            },
        }
    }
}

impl Speech {
    pub fn is_configured(&self) -> bool {
        !self.command.is_empty() || !self.base_url.trim().is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ConfigFile {
    pub output: Output,
    pub transcript: Transcript,
    pub speech: Speech,
    pub screenshots: Screenshots,
    pub ai: crate::llm::LlmSettings,
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
        serde_json::to_vec_pretty(cfg)?
    } else {
        toml::to_string_pretty(cfg)?.into_bytes()
    };
    crate::checkpoint::atomic_write(&p, &bytes)?;
    Ok(p)
}

/// The commented template written by `config init`; its values come from the built-in defaults, so they cannot drift.
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

const TEMPLATE: &str = r#"# notepipe configuration
#
# These settings apply to every run. Command-line options override them, and anything not set
# here uses the built-in default shown next to it. Lines starting with # are ignored: remove the
# # in front of a setting to change it.
#
#   notepipe config show    the settings in effect
#   notepipe config path    where this file is
#   notepipe --help         the matching command-line options
#
# Reference: https://github.com/3ae3ae/notepipe/blob/main/docs/configuration.md


# ── Output ────────────────────────────────────────────────────────────────────────────────
[output]

# Folder that receives the notes. Each note is a folder named after the recording's title,
# holding note.md, note.json and images/. "~" stands for your home folder.
#dir = "{OUT}"

# A program to run after each note is written, as a list of arguments (no shell). It runs in the
# note folder and receives NOTEPIPE_NOTE_DIR, NOTEPIPE_MARKDOWN, NOTEPIPE_JSON and
# NOTEPIPE_WARNINGS. For example, to copy every note into another folder:
#   on_complete = ["sh", "-c", "cp -R \"$NOTEPIPE_NOTE_DIR\" ~/Documents/notes/"]
#on_complete = []


# ── Transcript ────────────────────────────────────────────────────────────────────────────
[transcript]

# Where the transcript comes from:
#   "auto"       subtitles when there are any, otherwise speech recognition
#   "subtitles"  subtitles only; fail when there are none
#   "speech"     speech recognition only, even when subtitles exist
# Subtitles are lecture.srt or lecture.vtt next to a local lecture.mp4, or those a download
# adapter finds.
#source = "{TRANSCRIPT_SOURCE}"

# The spoken language, as a code such as "en", "ko" or "ja". It is passed to speech recognition
# and chooses between subtitles in several languages. Empty: detect automatically.
#language = ""


# ── Speech recognition ────────────────────────────────────────────────────────────────────
# Used when the transcript comes from speech. Set either base_url, for a service, or adapter,
# for a local engine that notepipe starts.
[speech]

# Address of an OpenAI-compatible speech service, for example:
#   OpenAI    https://api.openai.com/v1         model "gpt-4o-mini-transcribe"
#   Groq      https://api.groq.com/openai/v1    model "whisper-large-v3-turbo"
#   Mistral   https://api.mistral.ai/v1         model "voxtral-mini-latest"
#   a server of your own, such as http://localhost:8000/v1
#base_url = ""

# API key, sent as "Authorization: Bearer <key>". Leave it empty to read the
# NOTEPIPE_SPEECH_API_KEY environment variable instead, which keeps the key out of this file.
#api_key = ""

# Model name. Sent only when set.
#model = ""

# How audio is sent:
#   "transcriptions"  POST /audio/transcriptions, the OpenAI speech-to-text API
#   "chat"            audio input to /chat/completions, for audio-capable chat models
#api = "transcriptions"

# A local engine instead of base_url: the adapter's command and its arguments. notepipe starts
# it for the run and stops it afterwards. Install an official adapter as an extra, for example
# `uv tool install "notepipe[llama]"`, and then:
#   adapter = ["notepipe-asr-llama", "-hf", "ggml-org/Qwen3-ASR-1.7B-GGUF:Q8_0"]
#adapter = []

# How many requests run at the same time, 1 to 16. Local engines usually want 1.
#concurrency = {ASR_CONCURRENCY}

# How speech is found and cut into pieces before it is sent. The defaults suit most recordings.
[speech.detection]

# How sure the detector must be that a sound is speech, from 0 to 1. Lower keeps quieter speech.
#threshold = {VAD_THRESHOLD}

# Pauses shorter than this many seconds do not split speech.
#min_silence = {MIN_SILENCE}

# The longest piece of speech sent in one request, 1 to 600 seconds.
#max_segment = {MAX_SPEECH}


# ── Screenshots ───────────────────────────────────────────────────────────────────────────
# A screenshot is taken when the picture changes. Audio files have none.
[screenshots]

# "stable" waits until the picture stops changing, which suits animated slides;
# "first" takes the screenshot as soon as the picture changes.
#mode = "{SCREENSHOT_MODE}"

# How small a change triggers a screenshot, greater than 0 and at most 1. Higher takes more.
#sensitivity = {SIMILARITY}

# Seconds between the frames that are compared, at least 0.2.
#frame_interval = {SAMPLE_INTERVAL}

# The least time between two screenshots, in seconds.
#min_gap = {COOLDOWN}

# In "stable" mode, how long the picture must stay still, in seconds.
#settle = {STABLE_SECS}

# Watch only part of the frame, as "left%,top%-right%,bottom%". For example,
# "40%,0%-100%,100%" ignores the left 40%, where a speaker's video might be. Empty: the whole frame.
#region = ""


# ── AI stages ─────────────────────────────────────────────────────────────────────────────
# Optional steps that send the note to a language model, run in the order of the [[ai.stages]]
# blocks below. None run unless you add a block here or pass --stages.
[ai]

# Address of an OpenAI-compatible chat service, used by every stage, for example:
#   OpenAI       https://api.openai.com/v1
#   Anthropic    https://api.anthropic.com/v1
#   Gemini       https://generativelanguage.googleapis.com/v1beta/openai
#   OpenRouter   https://openrouter.ai/api/v1
#   Ollama       http://localhost:11434/v1
#base_url = "{LLM_BASE_URL}"

# A program that serves the AI stages instead of base_url: for example an adapter that calls a
# different API, such as the OpenAI Responses API, or one that uses a subscription you already have.
# notepipe starts it for the run and stops it afterwards. Stages that set their own base_url still
# go to that address. With an adapter, model is optional.
#   adapter = ["my-ai-adapter", "--option"]
#adapter = []

# API key. Leave it empty to read the NOTEPIPE_AI_API_KEY environment variable instead.
#api_key = ""

# The model for every stage that does not name its own.
#model = "{LLM_MODEL}"

# How many requests each stage runs at the same time, 1 to 16.
#concurrency = {LLM_CONCURRENCY}

# Extra fields added to every request, for options your service supports, for example:
#   request = { temperature = 0.2, reasoning_effort = "low" }
# notepipe sets model, messages, stream and response_format itself; they cannot be changed here.
#request = {}

# Each [[ai.stages]] block adds one stage. Two stages are built in and need only their name:
#
#   [[ai.stages]]
#   name = "proofread"     # corrects recognition errors and removes filler in each paragraph
#
#   [[ai.stages]]
#   name = "summary"       # adds a summary at the top of the note
#
# Every prompt is yours to change: a built-in stage takes a prompt that replaces its own, and any
# stage can also set its own base_url, api_key, model or request. A stage of your own needs a
# name, a prompt, and what it reads and writes:
#
#   [[ai.stages]]
#   name = "translate"
#   input = "paragraphs"   # "paragraphs": each paragraph; "transcript": the whole transcript
#   output = "below"       # paragraphs: "replace" or "below"; transcript: "top"
#   prompt = "Translate each item into Korean. Keep names, code and formulas unchanged."
#
# For paragraph stages, images = true also sends the screenshot shown during each paragraph, for
# models that accept images.


# ── Downloads ─────────────────────────────────────────────────────────────────────────────
# URLs are fetched by a download adapter, such as notepipe-source-ytdlp
# (`uv tool install "notepipe[ytdlp]"`).
[download]

# The adapter's command and its arguments. Empty: use the notepipe-source-* program that is
# installed, if there is exactly one. For the yt-dlp adapter, arguments after "--" go to yt-dlp:
#   adapter = ["notepipe-source-ytdlp", "--", "--cookies-from-browser", "firefox"]
#adapter = []
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
    println!("[speech]");
    println!("  {:<17}: {}", "base_url", r.asr.base_url);
    println!("  {:<17}: {}", "api_key", hidden(&r.asr.api_key));
    println!("  {:<17}: {}", "model", r.asr.model);
    println!("  {:<17}: {}", "api", r.asr.mode);
    println!("  {:<17}: {:?}", "adapter", r.asr.command);
    println!("  {:<17}: {}", "concurrency", r.asr.concurrency);
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
    println!("[ai]");
    println!("  {:<17}: {}", "base_url", cfg.ai.base_url);
    println!("  {:<17}: {:?}", "adapter", cfg.ai.adapter);
    println!("  {:<17}: {}", "api_key", hidden(&cfg.ai.api_key));
    println!("  {:<17}: {}", "model", cfg.ai.model);
    println!(
        "  {:<17}: {}",
        "concurrency",
        cfg.ai.concurrency.clamp(1, 16)
    );
    println!(
        "  {:<17}: {:?}",
        "request keys",
        cfg.ai.request.keys().collect::<Vec<_>>()
    );
    for stage in &cfg.ai.stages {
        let effective = cfg.ai.for_stage(stage)?;
        println!("[[ai.stages]]");
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
    println!("[download]");
    println!("  {:<17}: {:?}", "adapter", r.source.command);
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn old_sections_are_rejected() {
        assert!(super::parse("[asr]\nbase_url = 'https://example.test/v1'\n").is_err());
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
        cfg.speech.base_url = "https://example.test/v1".into();
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
        assert_eq!(from_template.sample_interval, built_in.sample_interval);
        assert_eq!(from_template.cooldown, built_in.cooldown);
        assert_eq!(from_template.screenshot_mode, built_in.screenshot_mode);
        assert_eq!(from_template.roi, built_in.roi);
        assert_eq!(from_template.transcript_source, built_in.transcript_source);
        assert_eq!(file.transcript.language, built_in.asr.language);
        assert_eq!(from_template.source, built_in.source);
        assert_eq!(from_template.asr, built_in.asr);
        assert_eq!(file.speech, super::Speech::default());
        assert_eq!(file.ai.adapter, super::ConfigFile::default().ai.adapter);
    }
}
