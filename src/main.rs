use clap::Parser;
use notepipe::cli::{Cli, Command, ConfigCmd};
use notepipe::{doctor, pipeline, progress, settings};
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
        // NDJSON 模式：stdout 只出 {"type":"log",...} 行（其余事件由各阶段显式 emit）
        use tracing_subscriber::prelude::*;
        tracing_subscriber::registry()
            .with(filter)
            .with(progress::json_log_layer())
            .init();
    } else if verbose >= 2 {
        // 调试档：完整格式（时间 + target）
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .with_target(true)
            .init();
    } else {
        // 默认档：个人 CLI 时间戳是纯噪音，compact 单行
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .with_target(false)
            .without_time()
            .compact()
            .init();
    }
}

fn main() -> std::process::ExitCode {
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
        if let Err(error) = notepipe::runtime::install_signal_handlers() {
            eprintln!("Error: {error}");
            return std::process::ExitCode::FAILURE;
        }
        // Keep signal handling responsive while stdin, prompts, or HTTP workers block.
        let mut work = tokio::task::spawn_blocking(main_inner);
        tokio::select! {
            result = &mut work => result.unwrap_or(std::process::ExitCode::FAILURE),
            _ = notepipe::runtime::watch_cancelled() => {
                let _ = tokio::time::timeout(std::time::Duration::from_secs(5), work).await;
                std::process::ExitCode::from(130)
            }
        }
    });
    if notepipe::runtime::is_cancelled() {
        runtime.shutdown_timeout(std::time::Duration::from_millis(100));
        return std::process::ExitCode::from(130);
    }
    code
}

fn main_inner() -> std::process::ExitCode {
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
    match run(cli, args.len() == 1) {
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

fn run(cli: Cli, no_args: bool) -> anyhow::Result<()> {
    match cli.command {
        Some(Command::Doctor) => doctor::run(),
        Some(Command::Config { cmd }) => {
            match cmd {
                ConfigCmd::Init { force } => {
                    let path = settings::config_path();
                    if path.is_file() && !force {
                        anyhow::bail!(
                            "Configuration already exists: {}. Use --force instead",
                            path.display()
                        );
                    }
                    if let Some(dir) = path.parent() {
                        std::fs::create_dir_all(dir)?;
                    }
                    std::fs::write(&path, settings::template())?;
                    println!("Configuration template created: {}", path.display());
                }
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
                        "Missing video source. Example: notepipe ./lecture.mp4"
                    );
                    println!("notepipe — Turn videos into illustrated notes\n");
                    println!("{}", notepipe::cli::QUICK_START);
                    println!("All options: notepipe --help");
                    return Ok(());
                }
            };
            let file = settings::load_from(cli.opts.config.as_deref())?;
            let initial = notepipe::options::resolve(source.clone(), &cli.opts, &file)?;
            // Fail before probing tools or starting a source adapter.
            initial.validate()?;
            tracing::info!(out = %initial.out_dir.display(), mode = %initial.asr.mode, "start");
            Ok(notepipe::runtime::block_on(pipeline::run(&initial))??)
        }
    }
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
            .map_err(|_| anyhow::anyhow!("Invalid URL; provide a complete video URL"))?;
        anyhow::ensure!(url.host_str().is_some(), "URL is missing a host");
        return Ok(source.to_owned());
    }
    anyhow::bail!(
        "Video file not found or URL not recognized: {}\nCheck the path and quote paths containing spaces.",
        source
    )
}
