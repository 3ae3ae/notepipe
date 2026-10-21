use anyhow::Context;
use clap::{CommandFactory, FromArgMatches};
use notepipe::cli::{Cli, Command, ConfigCmd, LlmCmd};
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
    let mut args: Vec<_> = std::env::args_os().collect();
    let lang_arg = notepipe::cli::take_lang_arg(&mut args);
    // 近似扫描（先于 clap 解析，解析失败也要走对输出模式）：把值恰为 "--json"/
    // "run-task" 的其他参数值（如 --llm-prompt "--json"）误判为 NDJSON 是可接受的
    // 代价——它只影响解析失败时的错误格式，不影响成功路径的行为。
    let json_requested = args
        .iter()
        .skip(1)
        .take_while(|a| *a != "--")
        .any(|a| a == "--json" || a == "run-task");
    let requested = lang_arg
        .as_deref()
        .and_then(notepipe::i18n::Lang::from_code)
        .unwrap_or_else(notepipe::i18n::Lang::detect);
    notepipe::i18n::set_lang(requested);
    let command = notepipe::cli::localize_command(Cli::command());
    let cli = match command.try_get_matches_from(&args) {
        Ok(matches) => match Cli::from_arg_matches(&matches) {
            Ok(mut cli) => {
                cli.lang = lang_arg.clone();
                cli
            }
            Err(error) => {
                let rendered = notepipe::cli::localize_rendered(&error.render().to_string());
                eprint!("{rendered}");
                return std::process::ExitCode::from(error.exit_code() as u8);
            }
        },
        Err(error) => {
            if error.use_stderr() && json_requested {
                progress::set_json_mode();
                progress::emit(
                    serde_json::json!({"type": "error", "message": notepipe::tr!("参数无效：{}", error)}),
                );
            } else if notepipe::i18n::lang() == notepipe::i18n::Lang::En {
                let _ = error.print();
            } else {
                let rendered = notepipe::cli::localize_rendered(&error.render().to_string());
                print!("{rendered}");
            }
            return std::process::ExitCode::from(error.exit_code() as u8);
        }
    };
    // Machine protocol messages stay in Simplified Chinese for stable consumers.
    if matches!(cli.command, Some(Command::RunTask)) {
        notepipe::i18n::set_lang(notepipe::i18n::Lang::ZhHans);
    } else if let Some(value) = &cli.lang {
        match notepipe::i18n::Lang::from_code(value) {
            Some(lang) => notepipe::i18n::set_lang(lang),
            None if value == "auto" => {
                notepipe::i18n::init_from_env();
            }
            None => {
                eprintln!(
                    "{}",
                    notepipe::tr!(
                        "未知的语言：{}。可用：auto、en、zh-Hans、zh-Hant、ja",
                        value
                    )
                );
                return std::process::ExitCode::FAILURE;
            }
        }
    }
    let json = cli.opts.json || matches!(&cli.command, Some(Command::RunTask));
    let quiet =
        cli.opts.quiet || matches!(&cli.command, Some(Command::Summarize(args)) if args.quiet);
    if json || matches!(cli.command, Some(Command::RunTask)) {
        // Machine consumers parse message text; keep the protocol's source language.
        notepipe::i18n::set_lang(notepipe::i18n::Lang::ZhHans);
    }
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
                notepipe::eprintln_tr!("错误：{}", message);
            }
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli, no_args: bool) -> anyhow::Result<()> {
    if let Some(platform) = cli.logout {
        return notepipe::login::logout(platform);
    }
    if let Some(platform) = cli.login {
        anyhow::ensure!(
            cli.source.as_deref().is_none_or(config::looks_like_source),
            notepipe::tr!(
                "--login 后只能跟视频链接或本地视频路径 / After --login, provide only a video URL or local file path"
            )
        );
        notepipe::login::login(platform)?;
        if cli.source.is_none() {
            return Ok(());
        }
    }
    match cli.command {
        Some(Command::RunTask) => {
            notepipe::dispatch::watch_parent();
            let request = notepipe::execution::Request::read(std::io::stdin().lock())?;
            Ok(notepipe::runtime::block_on(notepipe::execution::run(
                request,
            ))??)
        }
        Some(Command::Llm { cmd }) => {
            match cmd {
                LlmCmd::Setup {
                    base_url,
                    api_key,
                    model,
                    disable_hint,
                } => {
                    let cfg = llm::setup_interactive(
                        settings::load()?,
                        base_url,
                        api_key,
                        model,
                        disable_hint,
                    )?;
                    let path = settings::save(&cfg)?;
                    println!("已保存并启用 / Saved and enabled: {}", path.display());
                    match llm::test_connection(&cfg.llm) {
                        Ok(()) => notepipe::println_tr!("连接测试通过 / Connection test passed."),
                        Err(e) => anyhow::bail!(
                            "配置已保存，但连接测试失败；请检查服务地址、密钥和模型 / Settings saved, but connection test failed; check endpoint, key and model: {e:#}"
                        ),
                    }
                }
                LlmCmd::Status => llm::print_status(&settings::load()?),
                LlmCmd::Disable => {
                    let mut cfg = settings::load()?;
                    cfg.llm.enabled = false;
                    let path = settings::save(&cfg)?;
                    println!(
                        "已关闭 AI 校对，凭据保留 / AI proofreading disabled; credentials kept: {}",
                        path.display()
                    );
                }
            }
            Ok(())
        }
        Some(Command::Doctor) => doctor::run(),
        Some(Command::Config { cmd }) => {
            match cmd {
                ConfigCmd::Init { force } => {
                    let path = settings::config_path();
                    if path.is_file() && !force {
                        anyhow::bail!(
                            "{}",
                            notepipe::tr!("配置已存在：{}。使用 --force 替换", path.display())
                        );
                    }
                    if let Some(dir) = path.parent() {
                        std::fs::create_dir_all(dir)?;
                    }
                    std::fs::write(&path, settings::template())?;
                    println!(
                        "已生成配置模板 / Configuration template created: {}",
                        path.display()
                    );
                }
                ConfigCmd::Show => settings::print_effective(&settings::load()?)?,
            }
            Ok(())
        }
        Some(Command::Summarize(args)) => {
            let file = settings::load()?;
            llm::validate(&file.llm)?;
            let mut targets: Vec<std::path::PathBuf> = vec![];
            for dir in &args.dirs {
                collect_targets(dir, &mut targets, 0);
            }
            if targets.is_empty() {
                anyhow::bail!(
                    "未找到笔记：{0} / No notes found in: {0}",
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
        Some(Command::Remove(args)) => {
            let mut cfg = settings::load()?;
            let mut cleared: Vec<String> = vec![];
            // 只列出确实非空、本次真正清掉的字段（空配置不冒充「已清除」）
            for (name, value) in [
                ("llm.api_key", &cfg.llm.api_key),
                ("llm.base_url", &cfg.llm.base_url),
                ("llm.model", &cfg.llm.model),
            ] {
                if !value.is_empty() {
                    cleared.push(name.to_string());
                }
            }
            if notepipe::login::codex::remove_credentials()? {
                cleared.push("codex".to_string());
            }
            cfg.llm = notepipe::llm::LlmSettings::default();
            if args.asr {
                if !cfg.asr.api_key.is_empty() {
                    cleared.push("asr.api_key".to_string());
                }
                cfg.asr.api_key.clear();
            }
            let path = settings::save(&cfg)?;
            if cleared.is_empty() {
                println!(
                    "没有需要清除的 API 配置 / No saved API settings to clear: {}",
                    path.display()
                );
            } else {
                println!(
                    "已清除 API 配置 / Cleared API settings ({}): {}",
                    cleared.join(", "),
                    path.display()
                );
            }
            if !args.asr {
                notepipe::println_tr!(
                    "云端语音识别密钥未清除；如需清除：notepipe remove --asr / Cloud speech key kept; to clear it: notepipe remove --asr"
                );
            }
            Ok(())
        }
        None => {
            let source = match cli.source {
                Some(source) => normalize_source(&source)?,
                None => {
                    anyhow::ensure!(
                        no_args,
                        notepipe::tr!("缺少视频来源。示例：notepipe ./lecture.mp4")
                    );
                    println!("{}\n", notepipe::tr!("notepipe — 将视频转换为图文笔记"));
                    println!("{}", notepipe::tr!(notepipe::cli::QUICK_START));
                    notepipe::println_tr!("全部参数 / All options: notepipe --help");
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
        let version = manifest.parent().context(notepipe::tr!(
            "笔记清单位置无效 / Invalid note manifest location"
        ))?;
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
            tracing::warn!(
                "跳过无法读取的笔记 / Skipped unreadable note {}: {error:#}",
                dir.display()
            );
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
            println!(
                "已有摘要，笔记保持不变 / Summary already exists: {}",
                dir.display()
            );
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
        .context(notepipe::tr!(
            "笔记版本位置无效 / Invalid note version location"
        ))?
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
            println!(
                "摘要已保存 / Summary already saved: {}",
                target.version_dir().display()
            );
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
            println!("已导出摘要 / Summary exported: {}", file.display());
        }
    }
    if !progress::is_quiet() {
        println!(
            "摘要已作为新版笔记保存 / Summary saved as a new note version: {}",
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
        let url = url::Url::parse(source).map_err(|_| {
            anyhow::anyhow!(notepipe::tr!(
                "链接无效；请提供完整的视频链接 / Invalid URL; provide a complete video URL"
            ))
        })?;
        anyhow::ensure!(
            url.host_str().is_some(),
            notepipe::tr!("链接缺少主机名 / URL is missing a host")
        );
        return Ok(source.to_owned());
    }
    if config::known_video_site(source).is_some() {
        return Ok(format!("https://{source}"));
    }
    anyhow::bail!(
        "{}\n{}",
        notepipe::tr!("找不到视频文件或无法识别链接：{}", source),
        notepipe::tr!("请检查路径；含空格的路径需加引号")
    )
}
