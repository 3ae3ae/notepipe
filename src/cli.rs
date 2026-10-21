use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

pub const QUICK_START: &str = r#"Quick start:
  notepipe ./lecture.mp4
  notepipe https://example.com/video
  notepipe ./lecture.mp4 -o ./notes

Transcripts:
  Platform subtitles are used first; speech recognition runs when none exist.
  --transcript-source subtitle  Use subtitles only
  --transcript-source asr       Force speech recognition

Next steps:
  notepipe doctor              Check dependencies and settings
  notepipe config init         Create a configuration template
  notepipe summarize ./notes   Summarize existing notes

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
    /// Video URL or local file
    pub source: Option<String>,

    #[command(flatten)]
    pub opts: RunOpts,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Args, Clone, Debug, Default)]
pub struct RunOpts {
    /// Output root (platform/title/ID)
    #[arg(short, long)]
    #[arg(help_heading = "Input and output")]
    pub out: Option<PathBuf>,

    /// Similarity threshold; higher captures more slides
    #[arg(long)]
    #[arg(help_heading = "Slides")]
    pub similarity: Option<f64>,

    /// Frame sampling interval in seconds
    #[arg(long)]
    #[arg(help_heading = "Slides")]
    pub sample_interval: Option<f64>,

    /// Minimum seconds between screenshots
    #[arg(long)]
    #[arg(help_heading = "Slides")]
    pub cooldown: Option<f64>,

    /// Capture immediately or wait for stable slides
    #[arg(long, value_enum)]
    #[arg(help_heading = "Slides")]
    pub slide_mode: Option<crate::config::SlideMode>,

    /// Seconds to wait for a stable frame
    #[arg(long)]
    #[arg(help_heading = "Slides")]
    pub stable_secs: Option<f64>,

    /// Detect slide changes in a region: left%,top%-right%,bottom%
    #[arg(long)]
    #[arg(help_heading = "Slides")]
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

    /// Output formats, comma-separated: md,html,json
    #[arg(long, value_delimiter = ',', value_enum)]
    #[arg(help_heading = "Input and output")]
    pub formats: Option<Vec<crate::config::OutputFormat>>,

    /// Keep the downloaded video
    #[arg(long)]
    #[arg(help_heading = "Input and output")]
    pub keep_video: bool,

    /// Delete newly downloaded video after success
    #[arg(long, conflicts_with = "keep_video")]
    #[arg(help_heading = "Input and output")]
    pub no_keep_video: bool,

    /// Reuse the previously downloaded video
    #[arg(long)]
    #[arg(help_heading = "Input and output")]
    pub no_download: bool,

    /// Enable AI proofreading for this run
    #[arg(long)]
    #[arg(help_heading = "AI proofreading")]
    pub llm: bool,

    /// Disable AI proofreading for this run
    #[arg(long, conflicts_with = "llm")]
    #[arg(help_heading = "AI proofreading")]
    pub no_llm: bool,

    /// AI proofreading base URL (OpenAI-compatible)
    #[arg(long)]
    #[arg(help_heading = "AI proofreading")]
    pub llm_base_url: Option<String>,

    /// AI proofreading key; prefer NOTEPIPE_LLM_API_KEY to keep it out of shell history
    #[arg(long)]
    #[arg(help_heading = "AI proofreading")]
    pub llm_api_key: Option<String>,

    /// AI proofreading model name
    #[arg(long)]
    #[arg(help_heading = "AI proofreading")]
    pub llm_model: Option<String>,

    /// AI proofreading instructions
    #[arg(long)]
    #[arg(help_heading = "AI proofreading")]
    pub llm_prompt: Option<String>,

    /// Attach slides for proofreading (requires image support)
    #[arg(long, conflicts_with = "no_llm_vision")]
    #[arg(help_heading = "AI proofreading")]
    pub llm_vision: bool,

    /// Disable image-assisted proofreading
    #[arg(long, conflicts_with = "llm_vision")]
    #[arg(help_heading = "AI proofreading")]
    pub no_llm_vision: bool,

    /// Generate a video summary and add it to the notes
    #[arg(long)]
    #[arg(help_heading = "AI proofreading")]
    pub summarize: bool,

    /// Disable video summary for this run
    #[arg(long, conflicts_with = "summarize")]
    #[arg(help_heading = "AI proofreading")]
    pub no_summarize: bool,

    /// Bilingual notes: target language code or name (e.g. en, pt-BR, Arabic)
    #[arg(long, value_name = "LANG", conflicts_with = "no_translate", value_parser = parse_translation_target)]
    #[arg(help_heading = "AI proofreading")]
    pub translate: Option<String>,

    /// Do not translate this run
    #[arg(long)]
    #[arg(help_heading = "AI proofreading")]
    pub no_translate: bool,

    /// Diagnostic logs; -vv for debug details
    #[arg(short, long, action = clap::ArgAction::Count)]
    #[arg(help_heading = "Terminal output")]
    pub verbose: u8,

    /// Continue interrupted transcription (on by default)
    #[arg(long)]
    #[arg(help_heading = "Input and output")]
    pub resume: bool,

    /// Transcribe again from the start
    #[arg(long, conflicts_with = "resume")]
    #[arg(help_heading = "Input and output")]
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
    /// Execute a saved task from stdin JSON
    #[command(hide = true)]
    RunTask,
    /// Check tools, speech backends and settings
    Doctor,
    /// Manage the configuration file
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Add AI summaries to existing notes
    Summarize(SummarizeArgs),
}

#[derive(Args)]
pub struct SummarizeArgs {
    /// A note directory or a folder of notes
    #[arg(required = true)]
    pub dirs: Vec<PathBuf>,
    /// Regenerate existing summaries
    #[arg(long)]
    pub force: bool,
    /// Also save each summary as Markdown in this directory
    #[arg(short = 'o', long)]
    pub out: Option<PathBuf>,
    /// Quiet mode: report errors only
    #[arg(short, long)]
    pub quiet: bool,
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

fn parse_translation_target(value: &str) -> Result<String, String> {
    crate::language::target(value)
        .map(str::to_owned)
        .map_err(str::to_owned)
}
