use anyhow::Context;
use clap::Parser;
use notepipe::cli::{Cli, Command, ConfigCmd};
use notepipe::{config, doctor, llm, pipeline, progress, settings};
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
    let args: Vec<_> = std::env::args_os().collect();
    // 近似扫描（先于 clap 解析，解析失败也要走对输出模式）：把值恰为 "--json"/
    // "run-task" 的其他参数值（如 --llm-prompt "--json"）误判为 NDJSON 是可接受的
    // 代价——它只影响解析失败时的错误格式，不影响成功路径的行为。
    let json_requested = args
        .iter()
        .skip(1)
        .take_while(|a| *a != "--")
        .any(|a| a == "--json" || a == "run-task");
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
    let json = cli.opts.json || matches!(&cli.command, Some(Command::RunTask));
    let quiet =
        cli.opts.quiet || matches!(&cli.command, Some(Command::Summarize(args)) if args.quiet);
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
        Some(Command::RunTask) => {
            notepipe::dispatch::watch_parent();
            let request = notepipe::execution::Request::read(std::io::stdin().lock())?;
            Ok(notepipe::runtime::block_on(notepipe::execution::run(
                request,
            ))??)
        }
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
        Some(Command::Summarize(args)) => {
            let mut file = settings::load()?;
            if file.llm.api_key.trim().is_empty()
                && let Some(key) = config::llm_api_key_from_env()
            {
                file.llm.api_key = key;
            }
            llm::validate(&file.llm)?;
            let mut targets: Vec<std::path::PathBuf> = vec![];
            for dir in &args.dirs {
                collect_targets(dir, &mut targets, 0);
            }
            if targets.is_empty() {
                anyhow::bail!(
                    "No notes found in: {0}",
                    args.dirs
                        .iter()
                        .map(|d| d.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            for dir in &targets {
                summarize_note(&file, dir, args.force, args.out.as_deref())?;
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
            let file = settings::load()?;
            let initial = notepipe::options::resolve(source.clone(), &cli.opts, &file)?;
            // Fail before probing tools; run-task validates its supplied config independently.
            initial.validate()?;
            if initial.llm.enabled {
                llm::validate(&initial.llm)?;
            }
            if !std::path::Path::new(&source).is_file() {
                notepipe::error::require_ytdlp()?;
            }
            tracing::info!(out = %initial.out_dir.display(), mode = %initial.asr.mode, "start");
            Ok(notepipe::runtime::block_on(pipeline::run(&initial))??)
        }
    }
}

/// 笔记版本目录：课程目录取 current.json 指向的版本，版本目录直接使用。
fn note_version_dir(dir: &std::path::Path) -> anyhow::Result<Option<std::path::PathBuf>> {
    if dir.join("current.json").is_file() {
        let current: notepipe::artifact::CurrentVersion =
            serde_json::from_slice(&std::fs::read(dir.join("current.json"))?)?;
        let manifest = notepipe::artifact::safe_asset_path(dir, &current.manifest)?;
        let version = manifest
            .parent()
            .context("Invalid note manifest location")?;
        return Ok(Some(version.to_path_buf()));
    }
    Ok(dir
        .join("manifest.json")
        .is_file()
        .then(|| dir.to_path_buf()))
}

/// 递归收集笔记（可传单份笔记或存放多份笔记的上级目录）；单条目读取失败只告警并跳过。
fn collect_targets(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
    match note_version_dir(dir) {
        Ok(Some(version)) => return out.push(version),
        Ok(None) => {}
        Err(error) => {
            tracing::warn!("Skipped unreadable note {}: {error:#}", dir.display());
            return;
        }
    }
    if depth >= 5 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && !entry.file_name().to_string_lossy().starts_with('.') {
            collect_targets(&path, out, depth + 1);
        }
    }
}

/// `summarize` 子命令：为一份笔记补写 AI 摘要，保存为新版笔记（幂等，--force 重新生成）。
fn summarize_note(
    file: &settings::ConfigFile,
    dir: &std::path::Path,
    force: bool,
    out: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    use notepipe::{artifact, execution};
    let base = artifact::read_manifest(&dir.join("manifest.json"))?;
    artifact::validate_version(dir, &base)?;
    let document: artifact::Document =
        serde_json::from_slice(&std::fs::read(dir.join(&base.document))?)?;
    if document.summary.is_some() && !force {
        if !progress::is_quiet() {
            println!("Summary already exists: {}", dir.display());
        }
        return Ok(());
    }
    let mut settings = file.llm.clone();
    settings.api_key.clear();
    let mut task_id = format!(
        "summary-{}",
        execution::digest(&serde_json::to_vec(
            &serde_json::json!({"base_version":base.version_id,"settings":settings})
        )?)
    );
    if force {
        task_id.push_str(&format!(
            "-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
    }
    let note_dir = dir
        .parent()
        .and_then(|p| p.parent())
        .context("Invalid note version location")?
        .to_path_buf();
    let work = note_dir.join(".work").join(&task_id);
    execution::bind_work_dir(
        &work,
        &serde_json::json!({"source_id":base.source_id,"task_id":task_id,"settings":settings,"base_version":base.version_id}),
    )?;
    let _lock = notepipe::runtime::lock_file(&work.join(".task.lock"))?;
    let _dispatch = notepipe::dispatch::install(&work, None, &Default::default())?;
    let target = artifact::Target {
        task_id: task_id.clone(),
        note_id: base.note_id.clone(),
        source_id: base.source_id.clone(),
        version_id: task_id,
        note_dir,
    };
    if artifact::published(&target)?.is_some() {
        if !progress::is_quiet() {
            println!("Summary already saved: {}", target.version_dir().display());
        }
        return Ok(());
    }
    let speech = pipeline::all_speech(&document.sections);
    let summary = notepipe::runtime::block_on(notepipe::summarize::summarize(
        &file.llm,
        &speech,
        &document.meta,
    ))??;
    let mut outcomes = base.outcomes;
    outcomes.summary = artifact::Outcome::succeeded();
    outcomes.exports.clear();
    notepipe::runtime::block_on(artifact::publish(
        &target,
        dir,
        &document.meta,
        &document.sections,
        Some(&summary),
        &[],
        outcomes,
    ))??;
    if let Some(out) = out {
        let file = out.join(format!(
            "{}.summary.md",
            notepipe::summarize::sanitize_filename(&document.meta.title)
        ));
        let text = notepipe::summarize::render_standalone_md(&document.meta.title, &summary);
        notepipe::checkpoint::atomic_write(&file, text.as_bytes())?;
        if !progress::is_quiet() {
            println!("Summary exported: {}", file.display());
        }
    }
    if !progress::is_quiet() {
        println!(
            "Summary saved as a new note version: {}",
            target.version_dir().display()
        );
    }
    Ok(())
}

/// Accept existing files and normalize URLs pasted without a scheme.
fn normalize_source(source: &str) -> anyhow::Result<String> {
    if std::path::Path::new(source).is_file() {
        return Ok(source.to_owned());
    }
    let source = source.trim();
    if source.starts_with("http://") || source.starts_with("https://") {
        let url = url::Url::parse(source)
            .map_err(|_| anyhow::anyhow!("Invalid URL; provide a complete video URL"))?;
        anyhow::ensure!(url.host_str().is_some(), "URL is missing a host");
        return Ok(source.to_owned());
    }
    if config::known_video_site(source).is_some() {
        return Ok(format!("https://{source}"));
    }
    anyhow::bail!(
        "Video file not found or URL not recognized: {}\nCheck the path and quote paths containing spaces.",
        source
    )
}
