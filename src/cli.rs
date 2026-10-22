use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

pub(super) fn after_help() -> String {
    let path = crate::settings::config_path();
    let not_created = if path.exists() {
        ""
    } else {
        " (created on the first run)"
    };
    format!(
        r#"Examples:
  notepipe lecture.mp4                     A note for a local video, in the current folder
  notepipe interview.m4a -o ~/notes        A note for an audio file, under ~/notes
  notepipe talk.mp4 --subtitle talk.srt    Use your own transcript
  notepipe lecture.mp4 --stages summary    Add an AI summary at the top
  notepipe https://youtu.be/VIDEO_ID       A URL (install "notepipe[ytdlp]" first)

Transcripts:
  Subtitles are used when they exist: lecture.srt or lecture.vtt next to the file, or those found by the
  download adapter. Otherwise speech is sent to the service set in [speech]; the first run in a terminal
  asks for it and saves it.

Settings:
  Options override the configuration file, which overrides the built-in defaults.
  File: {}{not_created}
  notepipe config init     Create the file with the default settings
  notepipe config show     Show the settings in effect
  notepipe doctor          Check ffmpeg, adapters and settings

Documentation: https://github.com/3ae3ae/notepipe"#,
        path.display()
    )
}

pub(super) fn version() -> &'static str {
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
    long_about = "Turn video and audio into Markdown notes.\n\nnotepipe writes a note for a recording: the transcript in paragraphs, arranged around screenshots of what was on screen,\noptionally proofread, summarized or extended by an AI model. Each note is a folder with note.md, note.json and images/.",
    args_conflicts_with_subcommands = true,
    after_help = after_help()
)]
pub struct Cli {
    /// A local video or audio file, or a URL (URLs need a download adapter)
    #[arg(value_name = "SOURCE")]
    pub source: Option<String>,

    #[command(flatten)]
    pub opts: RunOpts,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Args, Clone, Debug, Default)]
pub struct RunOpts {
    /// Folder that receives the note folders (default: the current folder; config: output.dir)
    #[arg(short, long, value_name = "DIR", help_heading = "Output")]
    pub out: Option<PathBuf>,

    /// Read settings from this file instead of the configuration file; '-' reads them from standard input
    #[arg(long, value_name = "FILE", help_heading = "Output")]
    pub config: Option<PathBuf>,

    /// Discard this source's cached work and start over
    #[arg(long, help_heading = "Output")]
    pub no_resume: bool,

    /// Where the transcript comes from (default: auto; config: transcript.source)
    #[arg(
        long = "transcript",
        value_enum,
        value_name = "SOURCE",
        help_heading = "Transcript"
    )]
    pub transcript_source: Option<crate::config::TranscriptSource>,

    /// Use this SRT or VTT file as the transcript
    #[arg(long, value_name = "FILE", help_heading = "Transcript")]
    pub subtitle: Option<PathBuf>,

    /// Spoken language, such as en or ko; also picks the subtitle language (default: detect; config: transcript.language)
    #[arg(long = "language", value_name = "CODE", help_heading = "Transcript")]
    pub asr_language: Option<String>,

    /// OpenAI-compatible speech service, such as https://api.openai.com/v1 (config: speech.base_url)
    #[arg(
        long = "speech-base-url",
        value_name = "URL",
        help_heading = "Speech recognition"
    )]
    pub asr_base_url: Option<String>,

    /// API key for the speech service; NOTEPIPE_SPEECH_API_KEY keeps it out of shell history (config: speech.api_key)
    #[arg(
        long = "speech-api-key",
        value_name = "KEY",
        help_heading = "Speech recognition"
    )]
    pub asr_api_key: Option<String>,

    /// Speech model, such as gpt-4o-mini-transcribe (config: speech.model)
    #[arg(
        long = "speech-model",
        value_name = "NAME",
        help_heading = "Speech recognition"
    )]
    pub asr_model: Option<String>,

    /// How audio is sent to the service (default: transcriptions; config: speech.api)
    #[arg(
        long = "speech-api",
        value_enum,
        value_name = "API",
        help_heading = "Speech recognition"
    )]
    pub asr_mode: Option<crate::settings::AsrMode>,

    /// Longest piece of speech sent in one request, 1 to 600 (default: 20; config: speech.detection.max_segment)
    #[arg(
        long = "max-segment",
        value_name = "SECONDS",
        help_heading = "Speech recognition"
    )]
    pub max_speech: Option<f32>,

    /// When to take a screenshot after the picture changes (default: stable; config: screenshots.mode)
    #[arg(long, value_enum, value_name = "MODE", help_heading = "Screenshots")]
    pub screenshot_mode: Option<crate::config::ScreenshotMode>,

    /// How small a change triggers a screenshot; higher takes more (default: 0.85; config: screenshots.sensitivity)
    #[arg(long = "sensitivity", value_name = "0-1", help_heading = "Screenshots")]
    pub similarity: Option<f64>,

    /// Seconds between the frames that are compared, at least 0.2 (default: 1; config: screenshots.frame_interval)
    #[arg(
        long = "frame-interval",
        value_name = "SECONDS",
        help_heading = "Screenshots"
    )]
    pub sample_interval: Option<f64>,

    /// Least time between two screenshots (default: 10; config: screenshots.min_gap)
    #[arg(long = "min-gap", value_name = "SECONDS", help_heading = "Screenshots")]
    pub cooldown: Option<f64>,

    /// In stable mode, how long the picture must stay still (default: 0.8; config: screenshots.settle)
    #[arg(long = "settle", value_name = "SECONDS", help_heading = "Screenshots")]
    pub stable_secs: Option<f64>,

    /// Watch only part of the frame, as left%,top%-right%,bottom% (config: screenshots.region)
    ///
    /// For example, 40%,0%-100%,100% ignores the left 40% of the frame, where a speaker's video might be.
    #[arg(long = "region", value_name = "AREA", help_heading = "Screenshots")]
    pub roi: Option<String>,

    /// Stages to run, in order, such as proofread,summary (default: the [[stages]] in the configuration)
    #[arg(
        long,
        value_name = "NAMES",
        value_delimiter = ',',
        conflicts_with = "no_stages",
        help_heading = "Stages"
    )]
    pub stages: Option<Vec<String>>,

    /// Run no stages
    #[arg(long, help_heading = "Stages")]
    pub no_stages: bool,

    /// OpenAI-compatible chat service used by every stage (config: ai.base_url)
    #[arg(long = "ai-base-url", value_name = "URL", help_heading = "Stages")]
    pub llm_base_url: Option<String>,

    /// API key for the AI service; NOTEPIPE_AI_API_KEY keeps it out of shell history (config: ai.api_key)
    #[arg(long = "ai-api-key", value_name = "KEY", help_heading = "Stages")]
    pub llm_api_key: Option<String>,

    /// Model for every stage that does not set its own (config: ai.model)
    #[arg(long = "ai-model", value_name = "NAME", help_heading = "Stages")]
    pub llm_model: Option<String>,

    /// Show more detail; -vv for debugging
    #[arg(short, long, action = clap::ArgAction::Count, help_heading = "Terminal")]
    pub verbose: u8,

    /// Show errors only
    #[arg(short, long, help_heading = "Terminal")]
    pub quiet: bool,

    /// Print progress as JSON lines for other programs, and never prompt
    #[arg(long, help_heading = "Terminal")]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Check that ffmpeg, adapters and settings are ready
    Doctor,
    /// Create, find and show the configuration file
    #[command(arg_required_else_help = true)]
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
}

#[derive(Subcommand)]
pub enum ConfigCmd {
    /// Create the configuration file with the default settings
    Init {
        /// Replace an existing configuration file
        #[arg(long)]
        force: bool,
    },
    /// Print where the configuration file is
    Path,
    /// Show the settings in effect, from the file and the built-in defaults
    Show,
}
