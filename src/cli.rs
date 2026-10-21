use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

pub const QUICK_START: &str = r#"Quick start:
  notepipe ./lecture.mp4
  notepipe ./recording.mp3
  notepipe https://example.com/video
  notepipe ./lecture.mp4 -o ./notes

Transcripts:
  Platform subtitles are used first; speech recognition runs when none exist.
  --transcript-source subtitle  Use subtitles only
  --transcript-source asr       Force speech recognition

Next steps:
  notepipe doctor              Check dependencies and settings
  notepipe config init         Create a configuration template

The first conversion in a terminal offers setup; scripts should pass options explicitly.
Use <command> --help for details.
"#;

fn version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| match option_env!("NOTEPIPE_COMMIT_HASH") {
        Some(hash) => format!("{} ({hash})", env!("CARGO_PKG_VERSION")),
        None => env!("CARGO_PKG_VERSION").into(),
    })
}

#[derive(Parser)]
#[command(
    name = "notepipe",
    version = version(),
    about = "Turn video and audio into Markdown notes",
    args_conflicts_with_subcommands = true,
    after_help = QUICK_START
)]
pub struct Cli {
    /// Video or audio URL, or local file
    pub source: Option<String>,

    #[command(flatten)]
    pub opts: RunOpts,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Args, Clone, Debug, Default)]
pub struct RunOpts {
    /// Output root for note folders (default: current directory)
    #[arg(short, long)]
    #[arg(help_heading = "Input and output")]
    pub out: Option<PathBuf>,

    /// Similarity threshold; higher captures more screenshots
    #[arg(long)]
    #[arg(help_heading = "Screenshots")]
    pub similarity: Option<f64>,

    /// Frame sampling interval in seconds
    #[arg(long)]
    #[arg(help_heading = "Screenshots")]
    pub sample_interval: Option<f64>,

    /// Minimum seconds between screenshots
    #[arg(long)]
    #[arg(help_heading = "Screenshots")]
    pub cooldown: Option<f64>,

    /// Capture immediately or wait for a stable frame
    #[arg(long, value_enum)]
    #[arg(help_heading = "Screenshots")]
    pub screenshot_mode: Option<crate::config::ScreenshotMode>,

    /// Seconds to wait for a stable frame
    #[arg(long)]
    #[arg(help_heading = "Screenshots")]
    pub stable_secs: Option<f64>,

    /// Detect screenshot changes in a region: left%,top%-right%,bottom%
    #[arg(long)]
    #[arg(help_heading = "Screenshots")]
    pub roi: Option<String>,

    /// Speech service base URL or full endpoint
    #[arg(long)]
    #[arg(help_heading = "Speech recognition")]
    pub asr_base_url: Option<String>,

    /// Cloud API key; prefer NOTEPIPE_ASR_API_KEY to keep it out of shell history
    #[arg(long)]
    #[arg(help_heading = "Speech recognition")]
    pub asr_api_key: Option<String>,

    /// Speech service model; omitted when empty
    #[arg(long)]
    #[arg(help_heading = "Speech recognition")]
    pub asr_model: Option<String>,

    /// HTTP dialect: transcriptions, chat
    #[arg(long, value_enum)]
    #[arg(help_heading = "Speech recognition")]
    pub asr_mode: Option<crate::settings::AsrMode>,

    /// Speech language code; empty means automatic detection
    #[arg(long, help_heading = "Speech recognition")]
    pub asr_language: Option<String>,

    /// Transcript source: auto, subtitle, asr
    #[arg(long, value_enum)]
    #[arg(help_heading = "Input and output")]
    pub transcript_source: Option<crate::config::TranscriptSource>,

    /// Maximum speech segment length in seconds
    #[arg(long)]
    #[arg(help_heading = "Speech recognition")]
    pub max_speech: Option<f32>,

    /// Read TOML or JSON configuration from a file, or '-' for stdin
    #[arg(long, value_name = "FILE", help_heading = "Input and output")]
    pub config: Option<PathBuf>,

    /// Use this SRT/VTT transcript; skip subtitle discovery and speech recognition
    #[arg(long, value_name = "FILE", help_heading = "Input and output")]
    pub subtitle: Option<PathBuf>,

    /// LLM connection base URL (OpenAI-compatible)
    #[arg(long)]
    #[arg(help_heading = "LLM stages")]
    pub llm_base_url: Option<String>,

    /// LLM connection key; prefer NOTEPIPE_LLM_API_KEY to keep it out of shell history
    #[arg(long)]
    #[arg(help_heading = "LLM stages")]
    pub llm_api_key: Option<String>,

    /// LLM connection model name
    #[arg(long)]
    #[arg(help_heading = "LLM stages")]
    pub llm_model: Option<String>,

    /// Override the selected proofread stage's prompt
    #[arg(long)]
    #[arg(help_heading = "LLM stages")]
    pub llm_prompt: Option<String>,

    /// Ordered built-in or configured LLM stage names, comma-separated
    #[arg(
        long,
        value_delimiter = ',',
        conflicts_with = "no_stages",
        help_heading = "LLM stages"
    )]
    pub stages: Option<Vec<String>>,

    /// Run without LLM stages
    #[arg(long, help_heading = "LLM stages")]
    pub no_stages: bool,

    /// Diagnostic logs; -vv for debug details
    #[arg(short, long, action = clap::ArgAction::Count)]
    #[arg(help_heading = "Terminal output")]
    pub verbose: u8,

    /// Clear this source's cached work and start again
    #[arg(long, help_heading = "Input and output")]
    pub no_resume: bool,

    /// Quiet mode: report errors only
    #[arg(short, long)]
    #[arg(help_heading = "Terminal output")]
    pub quiet: bool,

    /// Stream NDJSON progress without prompts
    #[arg(long)]
    #[arg(help_heading = "Terminal output")]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Check tools, speech backends and settings
    Doctor,
    /// Manage the configuration file
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
}

#[derive(Subcommand)]
pub enum ConfigCmd {
    /// Create a template; --force replaces existing settings
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Show the config path and file settings
    Show,
}
