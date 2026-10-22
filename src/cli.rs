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
  notepipe lecture.mp4                           A note for a local video, in the current folder
  notepipe interview.m4a -o ~/notes              A note for an audio file, under ~/notes
  notepipe talk.mp4 --subtitle talk.srt          Use your own transcript
  notepipe lecture.mp4 --skip proofread,summary  Notes without proofreading or a summary
  notepipe https://youtu.be/VIDEO_ID             A URL (install "notepipe[ytdlp]" first)

Transcripts:
  The transcript stage tries its sources in order: subtitles next to the file or from the download, then
  speech recognition through its provider. --subtitle FILE uses your own transcript instead.
  See notepipe config show for the pipeline in effect.

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
    long_about = "Turn video and audio into Markdown notes.\n\nnotepipe writes a note for a recording by running the stages in its configuration file. With the default settings,\nthe note is a folder with note.md, note.json and images/: a summary, then the proofread transcript in paragraphs around\nscreenshots of what was on screen.",
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
    /// Folder that receives the note folders (default: the current folder; config: out)
    #[arg(short, long, value_name = "DIR", help_heading = "Output")]
    pub out: Option<PathBuf>,

    /// Read settings from this file instead of the configuration file; '-' reads them from standard input
    #[arg(long, value_name = "FILE", help_heading = "Output")]
    pub config: Option<PathBuf>,

    /// Discard this source's cached work and start over
    #[arg(long, help_heading = "Output")]
    pub no_resume: bool,

    /// Use this SRT or VTT file as the transcript
    #[arg(long, value_name = "FILE", help_heading = "Transcript")]
    pub subtitle: Option<PathBuf>,

    /// Spoken language, such as en or ko; also picks the subtitle language (default: auto; config: the transcript stage's language)
    #[arg(long = "language", value_name = "CODE", help_heading = "Transcript")]
    pub asr_language: Option<String>,

    /// When to take a screenshot after the picture changes (default: stable; config: the screenshots stage's mode)
    #[arg(long, value_enum, value_name = "MODE", help_heading = "Screenshots")]
    pub screenshot_mode: Option<crate::config::ScreenshotMode>,

    /// How small a change triggers a screenshot; higher takes more (default: 0.85; config: the screenshots stage's sensitivity)
    #[arg(long = "sensitivity", value_name = "0-1", help_heading = "Screenshots")]
    pub similarity: Option<f64>,

    /// Seconds between the frames that are compared, at least 0.2 (default: 1; config: the screenshots stage's frame_interval)
    #[arg(
        long = "frame-interval",
        value_name = "SECONDS",
        help_heading = "Screenshots"
    )]
    pub sample_interval: Option<f64>,

    /// Least time between two screenshots (default: 10; config: the screenshots stage's min_gap)
    #[arg(long = "min-gap", value_name = "SECONDS", help_heading = "Screenshots")]
    pub cooldown: Option<f64>,

    /// In stable mode, how long the picture must stay still (default: 0.8; config: the screenshots stage's settle)
    #[arg(long = "settle", value_name = "SECONDS", help_heading = "Screenshots")]
    pub stable_secs: Option<f64>,

    /// Watch only part of the frame, as left%,top%-right%,bottom% (config: the screenshots stage's region)
    ///
    /// For example, 40%,0%-100%,100% ignores the left 40% of the frame, where a speaker's video might be.
    #[arg(long = "region", value_name = "AREA", help_heading = "Screenshots")]
    pub roi: Option<String>,

    /// Leave these stages out of the pipeline for this run, such as proofread,summary
    #[arg(
        long,
        value_name = "NAMES",
        value_delimiter = ',',
        help_heading = "Stages"
    )]
    pub skip: Vec<String>,

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
