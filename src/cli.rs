use crate::i18n::t;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

pub const QUICK_START: &str = r#"Quick start:
  notepipe ./lecture.mp4
  notepipe https://www.youtube.com/watch?v=VIDEO_ID
  notepipe ./lecture.mp4 -o ./notes

Transcripts:
  Platform subtitles are used first; speech recognition runs when none exist.
  --transcript-source subtitle  Use subtitles only
  --transcript-source asr       Force speech recognition

Next steps:
  notepipe doctor              Check dependencies and settings
  notepipe config init         Create a configuration template
  notepipe llm setup           Set up optional AI proofreading
  notepipe summarize ./notes   Summarize existing notes

The first conversion in a terminal offers setup; scripts should pass options explicitly.
Use <command> --help for details.
"#;

#[derive(Parser)]
#[command(
    name = "notepipe",
    version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("NOTEPIPE_COMMIT_HASH"), ")"),
    about = crate::tr!("Turn video and audio into Markdown notes"),
    args_conflicts_with_subcommands = true,
    after_help = QUICK_START
)]
pub struct Cli {
    /// Interface language.
    #[arg(long, global = true, value_name = "LANG", help = crate::tr!("Interface language: {}", language_choices()))]
    pub lang: Option<String>,

    /// Log in (ollama local, codex subscription), optionally then process a video
    #[arg(long, value_enum, conflicts_with_all = ["logout", "json"])]
    pub login: Option<LoginPlatform>,

    /// Remove saved login credentials
    #[arg(long, value_enum, conflicts_with_all = ["login", "source", "json"])]
    pub logout: Option<LoginPlatform>,

    /// Video URL or local file
    pub source: Option<String>,

    #[command(flatten)]
    pub opts: RunOpts,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum LoginPlatform {
    Ollama,
    Codex,
}

#[derive(Args, Clone, Debug, Default)]
pub struct RunOpts {
    /// Output root (platform/title/ID)
    #[arg(short, long)]
    #[arg(help_heading = "Input and output")]
    pub out: Option<PathBuf>,

    /// Cookie file (Netscape format; uses a private copy)
    #[arg(long, value_name = "FILE", help_heading = "Input and output")]
    pub cookies: Option<PathBuf>,

    /// Browser cookies (e.g. chrome, firefox:default-release)
    #[arg(long, value_name = "BROWSER", help_heading = "Input and output")]
    pub cookies_from_browser: Option<String>,

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

    /// Maximum download height (1080 recommended)
    #[arg(long)]
    #[arg(help_heading = "Slides")]
    pub max_height: Option<u32>,

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

    /// HTTP dialect: transcriptions, chat, xai, meta
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

    /// AI proofreading key (prefer llm setup to keep it out of shell history)
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

    /// Hide the optional AI proofreading tip
    #[arg(long)]
    #[arg(help_heading = "AI proofreading")]
    pub no_llm_hint: bool,

    /// Generate a video summary and add it to the notes
    #[arg(long)]
    #[arg(help_heading = "AI proofreading")]
    pub summarize: bool,

    /// Disable video summary for this run
    #[arg(long, conflicts_with = "summarize")]
    #[arg(help_heading = "AI proofreading")]
    pub no_summarize: bool,

    /// Bilingual notes: translate the text into this language (zh-Hans, zh-Hant, en, ja, ko, fr, de, es)
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
    /// Configure optional AI proofreading
    Llm {
        #[command(subcommand)]
        cmd: LlmCmd,
    },
    /// Check tools, speech backends and settings
    Doctor,
    /// Manage the configuration file
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Add AI summaries to existing notes
    Summarize(SummarizeArgs),
    /// Clear the AI proofreading endpoint, key, model and Codex login
    Remove(RemoveArgs),
}

#[derive(Args)]
pub struct RemoveArgs {
    /// Also clear the cloud speech API key
    #[arg(long)]
    pub asr: bool,
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
pub enum LlmCmd {
    /// Set up AI proofreading; prompts in a terminal
    Setup {
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        model: Option<String>,
        /// Hide the completion tip
        #[arg(long)]
        disable_hint: bool,
    },
    /// Show settings with keys hidden
    Status,
    /// Disable AI proofreading and keep credentials
    Disable,
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

pub fn language_choices() -> String {
    std::iter::once("auto")
        .chain(crate::i18n::languages().map(|lang| lang.code()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Removes `--lang` from the argument list before clap parses it.
///
/// `--lang` is a global option, but clap's `args_conflicts_with_subcommands`
/// would otherwise reject `notepipe --lang en doctor`. Keeping the flag in the
/// command definition documents it in `--help`; taking it out here lets it
/// appear before or after a subcommand.
pub fn take_lang_arg(args: &mut Vec<std::ffi::OsString>) -> Option<String> {
    let mut value = None;
    let mut index = 0;
    let mut literal = false;
    while index < args.len() {
        let arg = args[index].to_string_lossy().into_owned();
        if literal {
            index += 1;
            continue;
        }
        if arg == "--" {
            literal = true;
            index += 1;
            continue;
        }
        if arg == "--lang" {
            if index + 1 < args.len() {
                value = Some(args[index + 1].to_string_lossy().into_owned());
                args.drain(index..index + 2);
                continue;
            }
            // Leave a value-less flag for clap to report the missing value.
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--lang=") {
            value = Some(rest.to_owned());
            args.remove(index);
            continue;
        }
        index += 1;
    }
    value
}

/// Replaces clap's compile-time help and about texts with catalog translations.
///
/// The catalog is keyed by the original text, so the compiled doc comments stay
/// the single source of truth and a missing entry falls back to the source.
pub fn localize_command(command: clap::Command) -> clap::Command {
    use clap::Command;

    fn localized(value: Option<&clap::builder::StyledStr>) -> Option<String> {
        value.map(|value| t(&value.to_string()).to_owned())
    }

    fn about(command: Command) -> Command {
        let about = localized(command.get_about());
        let long_about = localized(command.get_long_about());
        let before = localized(command.get_before_help());
        let after = localized(command.get_after_help());
        let mut command = command;
        if let Some(value) = about {
            command = command.about(value);
        }
        if let Some(value) = long_about {
            command = command.long_about(value);
        }
        if let Some(value) = before {
            command = command.before_help(value);
        }
        if let Some(value) = after {
            command = command.after_help(value);
        }
        command
    }

    fn args(command: Command) -> Command {
        command.mut_args(|arg| {
            let help = localized(arg.get_help());
            let long_help = localized(arg.get_long_help());
            let heading = arg.get_help_heading().map(|heading| {
                // clap's heading type only accepts static strings; this runs once per process.
                Box::leak(t(heading).to_owned().into_boxed_str()) as &'static str
            });
            let mut arg = arg;
            if let Some(value) = help {
                arg = arg.help(value);
            }
            if let Some(value) = long_help {
                arg = arg.long_help(value);
            }
            if let Some(value) = heading {
                arg = arg.help_heading(value);
            }
            arg
        })
    }

    let command = about(command);
    let command = args(command);
    command.mut_subcommands(localize_command)
}

/// Localizes clap's own section headings in rendered help and error text.
///
/// Clap has no runtime translations for these strings; replacing whole lines
/// keeps the argument help itself (already localized) untouched.
pub fn localize_rendered(text: &str) -> String {
    let lang = crate::i18n::lang();
    if lang == crate::i18n::Lang::source() {
        return text.to_owned();
    }
    let (usage, options, commands, arguments, more) = (
        crate::tr!("Usage:"),
        crate::tr!("Options:"),
        crate::tr!("Command:"),
        crate::tr!("Parameters:"),
        crate::tr!("For more information, run --help."),
    );
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        let localized = if trimmed.starts_with("Usage:") {
            Some(line.replacen("Usage:", usage, 1))
        } else if trimmed.starts_with("Options:") {
            Some(line.replacen("Options:", options, 1))
        } else if trimmed.starts_with("Commands:") {
            Some(line.replacen("Commands:", commands, 1))
        } else if trimmed.starts_with("Arguments:") {
            Some(line.replacen("Arguments:", arguments, 1))
        } else if trimmed.starts_with("For more information, try '--help'.") {
            Some(line.replacen("For more information, try '--help'.", more, 1))
        } else if line.contains("Print help") {
            Some(line.replacen("Print help", crate::tr!("show help"), 1))
        } else if line.contains("Print version") {
            Some(line.replacen("Print version", crate::tr!("show version"), 1))
        } else if line.contains("Print this message or the help of the given subcommand(s)") {
            Some(line.replacen(
                "Print this message or the help of the given subcommand(s)",
                crate::tr!("Show help for this message or subcommand"),
                1,
            ))
        } else {
            None
        };
        out.push_str(localized.as_deref().unwrap_or(line));
        out.push('\n');
    }
    if !text.ends_with('\n') {
        out.pop();
    }
    out
}

fn parse_translation_target(value: &str) -> Result<String, String> {
    crate::language::target(value)
        .map(str::to_owned)
        .ok_or_else(|| {
            crate::tr!(
                "Supported languages: {}",
                crate::language::TARGETS.join(", ")
            )
        })
}
