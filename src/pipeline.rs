//! Local media to a note folder, with independent stage caches for each source.
use crate::{
    artifact::{self, Document},
    asr, checkpoint,
    config::{self, PipelineConfig, TranscriptSource},
    media, progress, runtime, scene,
    source::{self, MediaMeta},
    subtitle, timeline,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const NO_SCREENSHOTS: &str = "No screenshots retrieved";

fn source_key(source: &str, adapter: &crate::settings::Source) -> Result<String> {
    let path = Path::new(source);
    let identity = if path.is_file() {
        // shortcut: identity is path, size and mtime; a same-size replacement with preserved mtime reuses stale results — hash the file if that becomes a real problem
        let path = path.canonicalize()?;
        let metadata = path.metadata()?;
        serde_json::to_vec(
            &serde_json::json!({"path":path,"size":metadata.len(),"mtime":metadata.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_nanos()}),
        )?
    } else {
        let discovered = if adapter.command.is_empty() {
            Some(source::command(adapter)?.0)
        } else {
            None
        };
        serde_json::to_vec(
            &serde_json::json!({"url":source,"command":adapter.command,"discovered":discovered}),
        )?
    };
    Ok(runtime::digest(&identity)[..16].into())
}

fn screenshot_key(cfg: &PipelineConfig) -> Result<String> {
    // Keep the internal key label so the public rename does not invalidate cached screenshots.
    Ok(runtime::digest(&serde_json::to_vec(
        &serde_json::json!({"similarity":cfg.similarity,"sample_interval":cfg.sample_interval,"cooldown":cfg.cooldown,"slide_mode":cfg.screenshot_mode,"stable_secs":cfg.stable_secs,"roi":cfg.roi}),
    )?))
}

fn read_frames(root: &Path) -> Result<Option<scene::Output>> {
    let marker = root.join("complete.json");
    if !marker.is_file() {
        return Ok(None);
    }
    let output: scene::Output =
        serde_json::from_slice(&std::fs::read(marker)?).context("Screenshot cache is damaged")?;
    if let scene::Output::Frames(frames) = &output
        && (frames.is_empty()
            || frames.iter().any(|frame| {
                !frame.t.is_finite()
                    || frame.t < 0.
                    || !artifact::safe_asset_path(root, &frame.image)
                        .is_ok_and(|path| path.is_file())
            }))
    {
        return Ok(None);
    }
    Ok(Some(output))
}

#[derive(Serialize, Deserialize)]
struct TranscriptCache {
    events: Vec<timeline::TranscriptEvent>,
}
fn read_transcript(path: &Path) -> Result<Option<TranscriptCache>> {
    if !path.is_file() {
        return Ok(None);
    }
    let cache: TranscriptCache =
        serde_json::from_slice(&std::fs::read(path)?).context("Transcript cache is damaged")?;
    if !timeline::has_readable_events(&cache.events) {
        return Ok(None);
    }
    subtitle::validate_events(&cache.events)?;
    Ok(Some(cache))
}

fn selected_transcript(path: &Path) -> Result<(String, Vec<timeline::TranscriptEvent>)> {
    let key = runtime::file_digest(path)?;
    let events = subtitle::parse_subtitle(&subtitle::read_subtitle_text(path)?);
    subtitle::validate_events(&events)?;
    Ok((key, events))
}

pub async fn run(cfg: &PipelineConfig) -> Result<()> {
    let mut warnings = artifact::recover(&cfg.out_root)?;
    let mut cfg = cfg.clone();
    cfg.validate().context("Invalid settings")?;
    if let Some(path) = &cfg.subtitle {
        selected_transcript(path)?;
    }
    if cfg.subtitle.is_none() && cfg.transcript_source == TranscriptSource::Asr {
        cfg.validate_asr()?;
    }
    let local = Path::new(&cfg.url).is_file();
    if local {
        cfg.url = Path::new(&cfg.url)
            .canonicalize()?
            .to_string_lossy()
            .into_owned();
    } else {
        config::ensure_http_url(&cfg.url)?;
    }
    let cache_root = config::cache_dir();
    let work_root = cache_root.join("work");
    std::fs::create_dir_all(&work_root)?;
    let key = source_key(&cfg.url, &cfg.source)?;
    let _lock = runtime::lock_file(&work_root.join(format!("{key}.lock")))?;
    let work = work_root.join(key);
    runtime::check_cancelled()?;
    if cfg.no_resume && work.exists() {
        std::fs::remove_dir_all(&work)?;
    }
    std::fs::create_dir_all(&work)?;
    cfg.out_dir = work.clone();
    let audio = cfg.audio_path();
    let _dispatch = crate::dispatch::install(&work)?;
    if cfg.llm.api_key.trim().is_empty()
        && let Some(key) = config::llm_api_key_from_env()
    {
        cfg.llm.api_key = key;
    }
    let source_cache = work.join("source");
    progress::stage("fetch", "start");
    let mut source_media = None;
    let mut output = if local {
        None
    } else {
        source::Output::read(&source_cache)?
    };
    if !local && output.is_none() {
        source_media = Some(
            source::resolve(&cfg.url, &cfg.source, &source_cache)
                .await?
                .media,
        );
        output = source::Output::read(&source_cache)?;
    }
    if source_media.is_none()
        && let Some(output) = &output
    {
        match output.media_path(&source_cache) {
            Ok(path) => source_media = Some(path),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
            Err(error) => return Err(error),
        }
    }
    let mut meta = if let Some(output) = &output {
        output.metadata(&cfg.url)?
    } else {
        let cached = work.join("meta.json");
        if cached.is_file() {
            serde_json::from_slice(&std::fs::read(cached)?)?
        } else {
            MediaMeta {
                title: Path::new(&cfg.url)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                author: String::new(),
                duration: 0.,
                source: cfg.url.clone(),
                seek_url: None,
            }
        }
    };
    if meta.duration <= 0. && work.join("meta.json").is_file() {
        let saved: MediaMeta = serde_json::from_slice(&std::fs::read(work.join("meta.json"))?)?;
        if saved.source == cfg.url && saved.duration.is_finite() && saved.duration > 0. {
            meta.duration = saved.duration;
        }
    }
    progress::stage("fetch", "done");
    progress::stage("subtitle", "start");
    let selected = if let Some(path) = &cfg.subtitle {
        Some(path.clone())
    } else if cfg.transcript_source == TranscriptSource::Asr {
        None
    } else if let Some(cached) = &output {
        match cached.preferred_subtitle(&source_cache, &cfg.asr.language) {
            Ok(path) => path,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                source_media = Some(
                    source::resolve(&cfg.url, &cfg.source, &source_cache)
                        .await?
                        .media,
                );
                output = source::Output::read(&source_cache)?;
                output
                    .as_ref()
                    .context("Missing completed source output")?
                    .preferred_subtitle(&source_cache, &cfg.asr.language)?
            }
            Err(error) => return Err(error),
        }
    } else {
        subtitle::sidecar_subtitle(Path::new(&cfg.url), &cfg.asr.language)
    };
    if selected.is_none() {
        anyhow::ensure!(
            cfg.transcript_source != TranscriptSource::Subtitle,
            "No subtitles found. Select existing subtitles, or use speech recognition."
        );
        if crate::wizard::is_first_run(&cfg) {
            cfg.asr = crate::wizard::run(&cfg)?;
        }
        cfg.validate_asr()?;
    }
    if cfg.asr.api_key.trim().is_empty()
        && let Some(key) = config::asr_api_key_from_env()
    {
        cfg.asr.api_key = key;
    }
    let selected = selected.as_deref().map(selected_transcript).transpose()?;
    let transcript_key = if let Some((key, _)) = &selected {
        key.clone()
    } else {
        runtime::digest(&serde_json::to_vec(&checkpoint::AsrIdentity::new(
            &cfg.asr,
        ))?)
    };
    let transcript_path = work.join(format!("transcript-{transcript_key}.json"));
    let transcript = read_transcript(&transcript_path)?;
    let asr_root = work.join(format!("asr-{transcript_key}"));
    let frames_root = work.join(format!("frames-{}", screenshot_key(&cfg)?));
    let frames = read_frames(&frames_root)?;
    progress::stage("subtitle", "done");
    let need_media = frames.is_none() || (transcript.is_none() && selected.is_none());
    if need_media {
        crate::error::require_cmd("ffmpeg")?;
        crate::error::require_cmd("ffprobe")?;
        if !local && source_media.is_none() {
            let resolved = source::resolve(&cfg.url, &cfg.source, &source_cache).await?;
            meta = resolved.meta;
            source_media = Some(resolved.media);
        }
    }
    let media_path = if local {
        Some(PathBuf::from(&cfg.url))
    } else {
        source_media.clone()
    };
    if meta.duration <= 0.
        && let Some(path) = &media_path
    {
        meta.duration = media::probe_duration(path).await.unwrap_or(0.);
    }
    checkpoint::atomic_write(&work.join("meta.json"), &serde_json::to_vec(&meta)?)?;
    runtime::check_cancelled()?;
    let mut frame_cfg = cfg.clone();
    frame_cfg.out_dir = frames_root.clone();
    let mut asr_cfg = cfg.clone();
    asr_cfg.out_dir = asr_root.clone();
    let (frames, transcript) = tokio::join!(
        async {
            if let Some(frames) = frames {
                return Ok(frames);
            }
            let frames = scene::run(
                &frame_cfg,
                media_path.as_deref().context("Missing source media")?,
            )
            .await?;
            runtime::check_cancelled()?;
            checkpoint::atomic_write(
                &frames_root.join("complete.json"),
                &serde_json::to_vec(&frames)?,
            )?;
            Ok::<_, anyhow::Error>(frames)
        },
        async {
            let events = if let Some(cache) = transcript {
                return Ok(cache.events);
            } else if let Some((_, events)) = selected {
                events
            } else {
                progress::stage("audio", "start");
                media::extract_audio(
                    media_path.as_deref().context("Missing source media")?,
                    &audio,
                )
                .await?;
                progress::stage("audio", "done");
                runtime::check_cancelled()?;
                progress::stage("transcribe", "start");
                let events = asr::run(&asr_cfg, &audio).await?;
                progress::stage("transcribe", "done");
                anyhow::ensure!(
                    timeline::has_readable_events(&events),
                    "No speech was recognized in the recording"
                );
                events
            };
            subtitle::validate_events(&events)?;
            runtime::check_cancelled()?;
            checkpoint::atomic_write(
                &transcript_path,
                &serde_json::to_vec(&TranscriptCache {
                    events: events.clone(),
                })?,
            )?;
            Ok::<_, anyhow::Error>(events)
        }
    );
    runtime::check_cancelled()?;
    let frames = match frames {
        Ok(scene::Output::Frames(frames)) if !frames.is_empty() => frames,
        Ok(scene::Output::Frames(_)) => {
            warnings.push(NO_SCREENSHOTS.into());
            Vec::new()
        }
        Ok(scene::Output::NoVideo) => Vec::new(),
        Err(error) => {
            warnings.push(format!("Screenshots: {error:#}"));
            Vec::new()
        }
    };
    let speech = transcript?;
    let mut sections = if frames.is_empty() {
        vec![timeline::Section {
            t: 0.,
            end: meta.duration,
            image: String::new(),
            speech,
        }]
    } else {
        timeline::merge(frames, speech, meta.duration)
    };
    timeline::coalesce_sections(&mut sections);
    anyhow::ensure!(
        artifact::has_readable_body(&sections),
        "No readable transcript"
    );
    let mut top = Vec::new();
    let mut adapters = std::collections::HashMap::<Vec<String>, crate::adapter::Adapter>::new();
    for stage in &cfg.llm.stages {
        runtime::check_cancelled()?;
        let mut settings = cfg.llm.for_stage(stage)?;
        if settings.adapter.is_empty() && settings.base_url.trim().is_empty() {
            warnings.push(format!("{}: skipped; no AI service is set. Set base_url and model in [ai], or remove this stage.", stage.name));
            continue;
        }
        if !settings.adapter.is_empty() {
            if !adapters.contains_key(&settings.adapter) {
                // shortcut: the adapter starts even when every response is cached; start it on the first cache miss if startup cost matters.
                let adapter = start_ai_adapter(settings.adapter.clone()).await?;
                adapters.insert(settings.adapter.clone(), adapter);
            }
            settings.base_url = format!("{}/v1", adapters[&settings.adapter].base);
        }
        progress::stage(&stage.name, "start");
        match stage.input {
            crate::llm::Input::Paragraphs => {
                let (processed, report) =
                    paragraphs_with_rollback(sections, &frames_root, &settings, stage).await?;
                sections = processed;
                report_warning(&mut warnings, &stage.name, &report);
            }
            crate::llm::Input::Transcript => {
                let events = sections
                    .iter()
                    .flat_map(|section| section.speech.iter().cloned())
                    .collect::<Vec<_>>();
                match crate::summarize::run(&settings, stage, &events, &meta).await {
                    Ok(text) => top.push(timeline::StageText {
                        stage: stage.name.clone(),
                        text,
                    }),
                    Err(error) => warnings.push(format!("{}: {error:#}", stage.name)),
                }
            }
        }
        progress::stage(&stage.name, "done");
    }
    drop(adapters);
    runtime::check_cancelled()?;
    progress::stage("render", "start");
    if let Some(path) = source_media {
        remove_file(&path, &mut warnings);
    }
    remove_file(&audio, &mut warnings);
    let document = Document {
        schema: 1,
        meta,
        sections,
        top,
        warnings,
    };
    let root = cfg.out_root.clone();
    let (note_dir, mut warnings) =
        tokio::task::spawn_blocking(move || artifact::publish(&root, &frames_root, document))
            .await
            .context("Note publication interrupted")??;
    progress::stage("render", "done");
    if !cfg.on_complete.is_empty() {
        let (argv, directory, count) = (cfg.on_complete.clone(), note_dir.clone(), warnings.len());
        if let Err(error) =
            tokio::task::spawn_blocking(move || on_complete(&argv, &directory, count))
                .await
                .context("Completion hook interrupted")
                .and_then(|result| result)
        {
            warnings.push(format!("on_complete: {error:#}"));
        }
    }
    for warning in &warnings {
        eprintln!("Warning: {warning}");
    }
    progress::done(&note_dir, &warnings);
    if !progress::is_json() && !progress::is_quiet() {
        eprintln!("Notes ready: {}", note_dir.display());
    }
    Ok(())
}

async fn start_ai_adapter(argv: Vec<String>) -> Result<crate::adapter::Adapter> {
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _cancel_on_drop = runtime::CancelOnDrop(cancelled.clone());
    tokio::task::spawn_blocking(move || {
        let check_control = || {
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(runtime::Cancelled.into());
            }
            runtime::check_cancelled()
        };
        check_control()?;
        crate::adapter::Adapter::start_with_control(&argv, &check_control)
    })
    .await?
}

fn on_complete(argv: &[String], note_dir: &Path, warnings: usize) -> Result<()> {
    let program = crate::adapter::resolve_executable(&argv[0])?.canonicalize()?;
    let output = runtime::run_bounded(
        "on_complete",
        std::process::Command::new(program)
            .args(&argv[1..])
            .current_dir(note_dir)
            .env("NOTEPIPE_NOTE_DIR", note_dir)
            .env("NOTEPIPE_MARKDOWN", note_dir.join("note.md"))
            .env("NOTEPIPE_JSON", note_dir.join("note.json"))
            .env("NOTEPIPE_WARNINGS", warnings.to_string()),
        std::time::Duration::from_secs(10 * 60),
    )?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        crate::error::cmd_error("on_complete", output.status.code(), &output.stderr)
    );
    Ok(())
}

fn remove_file(path: &Path, warnings: &mut Vec<String>) {
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        warnings.push(format!(
            "Could not remove cached media {}: {error}",
            path.display()
        ));
    }
}

fn report_warning(warnings: &mut Vec<String>, stage: &str, report: &crate::llm::StageReport) {
    warnings.extend(report.warnings.iter().cloned());
    if report.failed > 0 {
        warnings.push(format!(
            "{stage} incomplete ({}/{} failed); original text retained{}",
            report.failed,
            report.attempted,
            report
                .note
                .as_ref()
                .map(|note| format!(": {note}"))
                .unwrap_or_default()
        ));
    }
}

async fn paragraphs_with_rollback(
    mut sections: Vec<timeline::Section>,
    frames_root: &Path,
    settings: &crate::llm::LlmSettings,
    stage: &crate::llm::Stage,
) -> Result<(Vec<timeline::Section>, crate::llm::StageReport)> {
    let original = sections.clone();
    let (settings, stage, root) = (settings.clone(), stage.clone(), frames_root.to_owned());
    let (mut sections, mut report) = tokio::task::spawn_blocking(move || {
        crate::llm::paragraphs(&mut sections, &root, &settings, &stage)
            .map(|report| (sections, report))
    })
    .await
    .context("Paragraphs stage interrupted")??;
    if !artifact::has_readable_body(&sections) {
        sections = original;
        report.failed = report.attempted;
    }
    Ok((sections, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn dropping_ai_adapter_startup_stops_its_blocking_process() {
        use std::time::{Duration, Instant};
        let _dispatch = runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        let Some(python) = runtime::which("python3").or_else(|| runtime::which("python")) else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let argv = vec![
            python.display().to_string(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/asr_adapter.py").into(),
            "--record".into(),
            root.path().display().to_string(),
            "--behavior".into(),
            "never".into(),
        ];
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let task = tokio::spawn(start_ai_adapter(argv));
            let start = Instant::now();
            while !root.path().join("pid").is_file() && start.elapsed() < Duration::from_secs(30) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            task.abort();
            assert!(matches!(task.await, Err(error) if error.is_cancelled()));
            let pid: i32 = std::fs::read_to_string(root.path().join("pid"))
                .unwrap()
                .parse()
                .unwrap();
            let start = Instant::now();
            while unsafe { libc::kill(pid, 0) } == 0 && start.elapsed() < Duration::from_secs(30) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert_eq!(
                unsafe { libc::kill(pid, 0) },
                -1,
                "AI adapter survived cancellation"
            );
        });
    }

    #[test]
    fn source_and_stage_keys_ignore_unrelated_settings_but_track_inputs() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("media");
        std::fs::write(&path, "first").unwrap();
        let mut adapter = crate::settings::Source {
            command: vec!["adapter-one".into()],
        };
        let before = source_key(path.to_str().unwrap(), &adapter).unwrap();
        std::fs::write(&path, "changed length").unwrap();
        assert_ne!(
            source_key(path.to_str().unwrap(), &adapter).unwrap(),
            before
        );
        let url = "https://example.test/media";
        let first = source_key(url, &adapter).unwrap();
        adapter.command.push("--different-setting".into());
        assert_ne!(source_key(url, &adapter).unwrap(), first);
        adapter.command = vec!["adapter-two".into()];
        assert_ne!(source_key(url, &adapter).unwrap(), first);
        let mut cfg = crate::options::resolve(
            "https://example.test/media".into(),
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
        let first = screenshot_key(&cfg).unwrap();
        cfg.llm
            .stages
            .push(serde_json::from_value::<crate::llm::Stage>(serde_json::json!({"name":"summary","input":"transcript","output":"top","prompt":"CUSTOM"})).unwrap());
        cfg.asr.model = "different".into();
        assert_eq!(screenshot_key(&cfg).unwrap(), first);
        cfg.similarity = 0.5;
        assert_ne!(screenshot_key(&cfg).unwrap(), first);
    }
    #[test]
    fn empty_transcripts_and_missing_screenshots_are_cache_misses() {
        let root = tempfile::tempdir().unwrap();
        let transcript = root.path().join("transcript.json");
        checkpoint::atomic_write(&transcript, br#"{"events":[]}"#).unwrap();
        assert!(read_transcript(&transcript).unwrap().is_none());
        checkpoint::atomic_write(
            &root.path().join("complete.json"),
            br#"[{"t":0,"image":"frames/missing.jpg"}]"#,
        )
        .unwrap();
        assert!(read_frames(root.path()).unwrap().is_none());
        checkpoint::atomic_write(&root.path().join("complete.json"), b"[]").unwrap();
        assert!(read_frames(root.path()).unwrap().is_none());
        checkpoint::atomic_write(&root.path().join("complete.json"), b"null").unwrap();
        assert!(matches!(
            read_frames(root.path()).unwrap(),
            Some(scene::Output::NoVideo)
        ));
    }
}
