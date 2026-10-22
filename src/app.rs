use crate::cli::{Cli, Command, ConfigCmd};
use crate::{doctor, pipeline, progress, settings};
use clap::Parser;
use tracing_subscriber::EnvFilter;

fn init_logging(verbose: u8, quiet: bool, json: bool) {
    let default = if quiet {
        "error"
    } else if verbose >= 2 {
        "debug"
    } else if verbose == 1 || json {
        "info"
    } else {
        "warn"
    };
    let filter = if quiet {
        EnvFilter::new("error")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| default.into())
    };
    if json {
        // NDJSON mode: logs become {"type":"log",...} lines on stdout; stages emit the other events.
        use tracing_subscriber::prelude::*;
        tracing_subscriber::registry()
            .with(filter)
            .with(progress::json_log_layer())
            .init();
    } else if verbose >= 2 {
        // Debug level: full format with time and target
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .with_target(true)
            .init();
    } else {
        // Default: compact single lines without timestamps
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .with_target(false)
            .without_time()
            .compact()
            .init();
    }
}

pub(super) fn main() -> std::process::ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Error: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let code = runtime.block_on(async {
        if let Err(error) = crate::runtime::install_signal_handlers() {
            eprintln!("Error: {error}");
            return std::process::ExitCode::FAILURE;
        }
        // Keep signal handling responsive while stdin, prompts, or HTTP workers block.
        let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pipeline_running = running.clone();
        let mut work = tokio::task::spawn_blocking(move || main_inner(pipeline_running));
        tokio::select! {
            result = &mut work => result.unwrap_or(std::process::ExitCode::FAILURE),
            _ = crate::runtime::watch_cancelled() => {
                if running.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = work.await;
                } else {
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), work).await;
                }
                std::process::ExitCode::from(130)
            }
        }
    });
    if crate::runtime::is_cancelled() {
        runtime.shutdown_timeout(std::time::Duration::from_millis(100));
        return std::process::ExitCode::from(130);
    }
    code
}

fn main_inner(running: std::sync::Arc<std::sync::atomic::AtomicBool>) -> std::process::ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    let json_requested = args
        .iter()
        .skip(1)
        .take_while(|a| *a != "--")
        .any(|a| a == "--json");
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error) => {
            if error.use_stderr() && json_requested {
                progress::set_json_mode();
                progress::emit(
                    serde_json::json!({"type": "error", "message": format!("Invalid arguments: {}", error)}),
                );
            } else {
                let _ = error.print();
            }
            return std::process::ExitCode::from(error.exit_code() as u8);
        }
    };
    let json = cli.opts.json;
    let quiet = cli.opts.quiet;
    if json {
        progress::set_json_mode();
    }
    progress::set_quiet(quiet);
    init_logging(cli.opts.verbose, quiet, json);
    match run(cli, args.len() == 1, &running) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let message = format!("{error:#}");
            if json {
                progress::emit(serde_json::json!({"type": "error", "message": message}));
            } else {
                eprintln!("Error: {}", message);
            }
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli, no_args: bool, running: &std::sync::atomic::AtomicBool) -> anyhow::Result<()> {
    match cli.command {
        Some(Command::Doctor) => doctor::run(),
        Some(Command::Config { cmd }) => {
            match cmd {
                ConfigCmd::Init { force } => {
                    let path = settings::config_path();
                    if path.is_file() && !force {
                        anyhow::bail!(
                            "{} already exists. Run `notepipe config init --force` to replace it.",
                            path.display()
                        );
                    }
                    if let Some(dir) = path.parent() {
                        std::fs::create_dir_all(dir)?;
                    }
                    std::fs::write(&path, settings::template())?;
                    println!("Created {}", path.display());
                    println!(
                        "Every value in it is in effect; edit it to change how notes are made."
                    );
                }
                ConfigCmd::Path => println!("{}", settings::config_path().display()),
                ConfigCmd::Show => settings::print_effective(&settings::load()?)?,
            }
            Ok(())
        }
        None => {
            let source = match cli.source {
                Some(source) => normalize_source(&source)?,
                None => {
                    anyhow::ensure!(
                        no_args,
                        "Missing video or audio source. Example: notepipe ./lecture.mp4"
                    );
                    println!(
                        "notepipe {} — turn video and audio into Markdown notes\n\n{}\n\nRun `notepipe --help` for every option.",
                        crate::cli::version(),
                        crate::cli::after_help()
                    );
                    return Ok(());
                }
            };
            let creation = if cli.opts.config.is_none() && !settings::config_path().exists() {
                create_default_config()
            } else {
                Ok(false)
            };
            let file = match creation {
                Ok(created) => {
                    if created && !progress::is_quiet() {
                        let message = format!(
                            "Created {}. Speech recognition and the proofread and summary stages use OpenAI with the key in OPENAI_API_KEY; edit the file to change that.",
                            settings::config_path().display()
                        );
                        if progress::is_json() {
                            progress::emit(progress::log_event("info", &message));
                        } else {
                            eprintln!("{message}");
                        }
                    }
                    settings::load_from(cli.opts.config.as_deref())?
                }
                Err(error) => {
                    eprintln!(
                        "Could not create {}: {error}. Continuing with the default settings.",
                        settings::config_path().display()
                    );
                    settings::parse(settings::template())?
                }
            };
            let initial = crate::options::resolve(source.clone(), &cli.opts, &file)?;
            // Fail before probing tools or starting a source adapter.
            initial.validate()?;
            tracing::info!(out = %initial.out_dir.display(), mode = %initial.asr.mode, "start");
            running.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(crate::runtime::block_on(pipeline::run(&initial))??)
        }
    }
}

fn create_default_config() -> std::io::Result<bool> {
    use std::io::Write;
    let path = settings::config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(error),
    };
    if let Err(error) = file.write_all(settings::template().as_bytes()) {
        // A partly written file would fail to parse on every later run.
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok(true)
}

/// Accept local files and complete HTTP(S) URLs.
fn normalize_source(source: &str) -> anyhow::Result<String> {
    if std::path::Path::new(source).is_file() {
        return Ok(std::path::Path::new(source)
            .canonicalize()?
            .to_string_lossy()
            .into_owned());
    }
    let source = source.trim();
    if source.starts_with("http://") || source.starts_with("https://") {
        let url = url::Url::parse(source)
            .map_err(|_| anyhow::anyhow!("Invalid URL; provide a complete video or audio URL"))?;
        anyhow::ensure!(url.host_str().is_some(), "URL is missing a host");
        return Ok(source.to_owned());
    }
    anyhow::bail!(
        "Video or audio file not found or URL not recognized: {}\nCheck the path and quote paths containing spaces.",
        source
    )
}
