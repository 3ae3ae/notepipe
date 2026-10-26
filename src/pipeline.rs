//! File dependencies schedule independent stages; publication happens after all succeed.
use crate::{
    artifact::{self, Document, Publication},
    asr, checkpoint,
    config::{self, PipelineConfig},
    media, progress, runtime, scene,
    settings::{self, Kind, Stage, StageType},
    source::{self, MediaMeta},
    subtitle, timeline,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

pub const NO_SCREENSHOTS: &str = "No screenshots retrieved";

fn source_key(source: &str, adapter: &settings::Source) -> Result<String> {
    let path = Path::new(source);
    let identity = if path.is_file() {
        // shortcut: path, size and mtime miss same-size replacements preserving mtime; hash media if needed.
        let path = path.canonicalize()?;
        let metadata = path.metadata()?;
        serde_json::to_vec(
            &serde_json::json!({"path":path,"size":metadata.len(),"mtime":metadata.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_nanos()}),
        )?
    } else {
        serde_json::to_vec(&serde_json::json!({"url":source,"command":adapter.command}))?
    };
    Ok(runtime::digest(&identity)[..16].into())
}
fn screenshot_key(cfg: &PipelineConfig) -> Result<String> {
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
            || frames.iter().any(|f| {
                !f.t.is_finite()
                    || f.t < 0.
                    || !artifact::safe_asset_path(root, &f.image).is_ok_and(|p| p.is_file())
            }))
    {
        return Ok(None);
    }
    Ok(Some(output))
}
#[derive(Serialize, Deserialize)]
struct Transcript {
    language: String,
    events: Vec<timeline::TranscriptEvent>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}
impl Transcript {
    fn read(path: &Path) -> Result<Self> {
        let transcript: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        subtitle::validate_events(&transcript.events)?;
        Ok(transcript)
    }
}
fn selected_transcript(path: &Path) -> Result<(String, Vec<timeline::TranscriptEvent>)> {
    let key = runtime::file_digest(path)?;
    let events = subtitle::parse_subtitle(&subtitle::read_subtitle_text(path)?);
    Ok((key, events))
}
struct Provider {
    definition: settings::Provider,
    key: String,
    limit: Arc<runtime::RequestLimit>,
    state: tokio::sync::Mutex<(usize, Option<crate::adapter::Adapter>)>,
}
impl Provider {
    async fn base(&self, name: &str) -> Result<String> {
        let mut state = self.state.lock().await;
        if let Some(base) = &self.definition.base_url {
            return Ok(base.clone());
        }
        if state.1.is_none() {
            state.1 = Some(start_ai_adapter(self.definition.argv(), name.into()).await?);
        }
        Ok(format!("{}/v1", state.1.as_ref().unwrap().base))
    }
    async fn finished(&self) {
        let mut state = self.state.lock().await;
        state.0 -= 1;
        if state.0 == 0 {
            state.1.take();
        }
    }
}
struct Run {
    cfg: PipelineConfig,
    work: PathBuf,
    providers: BTreeMap<String, Provider>,
    cache_locks: std::sync::Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
}
pub async fn run(cfg: &PipelineConfig) -> Result<()> {
    let mut providers = BTreeMap::new();
    for (name, stage) in &cfg.stages {
        if cfg.skip.contains(name)
            || (cfg.subtitle.is_some() && matches!(stage.kind, StageType::Transcript(_)))
        {
            continue;
        }
        if let Some(provider) = stage.used_provider() {
            if !providers.contains_key(provider) {
                let definition = cfg.providers[provider].clone();
                let key = definition.key(provider)?;
                providers.insert(
                    provider.to_owned(),
                    Provider {
                        limit: Arc::new(runtime::RequestLimit::new(
                            definition.concurrency as usize,
                        )),
                        definition,
                        key,
                        state: tokio::sync::Mutex::new((0, None)),
                    },
                );
            }
            providers.get_mut(provider).unwrap().state.get_mut().0 += 1;
        }
    }
    if let Some(path) = &cfg.subtitle {
        subtitle::validate_events(&selected_transcript(path)?.1)?;
    }
    let mut warnings = if cfg.stages.iter().any(|(name, s)| {
        (!cfg.skip.contains(name) || s.llm_writes_document())
            && s.publish.as_deref().is_some_and(|p| p != "-")
    }) {
        artifact::recover(&cfg.out_root)?
    } else {
        Vec::new()
    };
    let work_root = config::cache_dir().join("work");
    std::fs::create_dir_all(&work_root)?;
    let key = source_key(&cfg.url, &cfg.source)?;
    let _lock = runtime::lock_file(&work_root.join(format!("{key}.lock")))?;
    let work = work_root.join(key);
    if cfg.no_resume && work.exists() {
        std::fs::remove_dir_all(&work)?;
    }
    std::fs::create_dir_all(work.join(".cache"))?;
    let work = work.canonicalize()?;
    if cfg.verbose && !progress::is_quiet() {
        let message = format!("Work folder: {}", work.display());
        if progress::is_json() {
            progress::emit(progress::log_event("info", &message));
        } else {
            eprintln!("{message}");
        }
    }
    let _dispatch = crate::dispatch::install(&work.join(".cache"))?;
    let run = Arc::new(Run {
        cfg: cfg.clone(),
        work: work.clone(),
        providers,
        cache_locks: Default::default(),
    });
    let result = execute(run.clone(), &mut warnings).await;
    if result.is_err() {
        for provider in run.providers.values() {
            provider.state.lock().await.1.take();
        }
    }
    runtime::finish_blocking().await;
    cleanup_media(cfg, &work, &mut warnings)?;
    runtime::check_cancelled()?;
    let (note_dir, files, stdout) = result?;
    if let Some(path) = stdout {
        use std::io::Write;
        std::io::copy(
            &mut std::fs::File::open(path)?,
            &mut std::io::stdout().lock(),
        )?;
        std::io::stdout().flush()?;
    }
    for warning in &warnings {
        eprintln!("Warning: {warning}");
    }
    progress::done(&work, note_dir.as_deref(), &files, &warnings);
    if !progress::is_json()
        && !progress::is_quiet()
        && let Some(note_dir) = note_dir
    {
        eprintln!("Notes ready: {}", note_dir.display());
    }
    Ok(())
}
async fn execute(
    run: Arc<Run>,
    warnings: &mut Vec<String>,
) -> Result<(Option<PathBuf>, BTreeMap<String, String>, Option<PathBuf>)> {
    let mut pending = run.cfg.stages.clone();
    let download = run
        .cfg
        .stages
        .iter()
        .find(|(name, stage)| {
            !run.cfg.skip.contains(name) && matches!(stage.kind, StageType::Download(_))
        })
        .map(|(_, stage)| stage.output.as_str());
    let mut ready = HashSet::new();
    ready.insert("source".to_owned());
    let mut jobs = tokio::task::JoinSet::new();
    let mut failure = None;
    while !pending.is_empty() || !jobs.is_empty() {
        if failure.is_none() {
            let mut index = 0;
            while index < pending.len() {
                let (name, stage) = &pending[index];
                let skipped = run.cfg.skip.contains(name);
                let passthrough = skipped && stage.llm_writes_document();
                if (skipped && !passthrough)
                    || ((matches!(stage.kind, StageType::Download(_))
                        || download.is_none_or(|output| ready.contains(output)))
                        && stage.input.iter().all(|input| ready.contains(input)))
                {
                    let (name, stage) = pending.remove(index);
                    let run = run.clone();
                    jobs.spawn(async move {
                        let result = if skipped && !passthrough {
                            Ok(Vec::new())
                        } else {
                            run.stage(&name, &stage, passthrough).await
                        };
                        if !skipped
                            && !(run.cfg.subtitle.is_some()
                                && matches!(stage.kind, StageType::Transcript(_)))
                            && let Some(provider) = stage.used_provider()
                        {
                            run.providers[provider].finished().await;
                        }
                        (stage.output, result)
                    });
                } else {
                    index += 1;
                }
            }
        }
        let result = tokio::select! {
            result = jobs.join_next() => result,
            error = runtime::watch_cancelled(), if failure.is_none() => {
                failure = Some(error);
                jobs.abort_all();
                continue;
            }
        };
        if let Some(result) = result {
            let (output, result) = match result {
                Ok(result) => result,
                Err(_) if failure.is_some() => continue,
                Err(error) => return Err(error.into()),
            };
            match result {
                Ok(stage_warnings) => {
                    ready.insert(output.clone());
                    warnings.extend(stage_warnings);
                }
                Err(error) if failure.is_none() => {
                    failure = Some(error);
                    jobs.abort_all();
                }
                Err(_) => {}
            }
        } else {
            break;
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    runtime::check_cancelled()?;
    let mut outputs = Vec::new();
    let mut stdout = None;
    for (name, stage) in &run.cfg.stages {
        if run.cfg.skip.contains(name) && !stage.llm_writes_document() {
            continue;
        }
        if let Some(publish) = &stage.publish {
            if publish == "-" {
                stdout = Some(run.work.join(&stage.output));
            }
            outputs.push(Publication {
                stage: name.clone(),
                output: stage.output.clone(),
                name: publish.clone(),
                images: run.images(stage)?,
            });
        }
    }
    if let Some(path) = &stdout {
        anyhow::ensure!(path.is_file(), "Standard output takes a file");
        std::fs::File::open(path)?;
    }
    let meta = run.meta()?;
    let (note, files) = artifact::publish(&run.cfg.out_root, &run.work, &meta, &outputs, warnings)?;
    Ok((note, files, stdout))
}
impl Run {
    fn cache_lock(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        runtime::lock_recover(&self.cache_locks)
            .entry(key.into())
            .or_default()
            .clone()
    }
    fn meta(&self) -> Result<MediaMeta> {
        for (name, stage) in &self.cfg.stages {
            if matches!(stage.kind, StageType::Download(_)) && !self.cfg.skip.contains(name) {
                return source::Output::info(&self.work.join(&stage.output))?
                    .metadata(&self.cfg.url);
            }
        }
        Ok(MediaMeta {
            extra: Default::default(),
            title: Path::new(&self.cfg.url)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            author: String::new(),
            duration: 0.,
            source: self.cfg.url.clone(),
            seek_url: None,
        })
    }
    async fn stage(&self, name: &str, stage: &Stage, passthrough: bool) -> Result<Vec<String>> {
        runtime::check_cancelled()?;
        if !passthrough {
            progress::stage(name, "start");
        }
        let staging = tempfile::Builder::new()
            .prefix(".stage-")
            .tempdir_in(&self.work)?;
        let target = staging.path().join(&stage.output);
        let mut warnings = Vec::new();
        let inputs = stage
            .input
            .iter()
            .map(|p| self.work.join(p))
            .collect::<Vec<_>>();
        if passthrough {
            artifact::copy_path(&inputs[0], &target)?;
            Document::read(&target, &self.work)?;
        } else {
            match &stage.kind {
                StageType::Download(_) => self.download(name, &target).await?,
                StageType::Transcript(t) => {
                    self.transcript(name, stage, t, &inputs[0], &target).await?
                }
                StageType::Screenshots(_) => {
                    warnings = self.screenshots(name, stage, &inputs[0], &target).await?
                }
                StageType::Document {} => {
                    let transcript = stage
                        .input
                        .iter()
                        .position(|p| self.kind(p) == Kind::Transcript)
                        .unwrap();
                    let transcript = Transcript::read(&inputs[transcript])?;
                    let mut frames = Vec::new();
                    if let Some(index) = stage
                        .input
                        .iter()
                        .position(|p| self.kind(p) == Kind::Images)
                    {
                        frames = serde_json::from_slice::<Vec<timeline::FrameEvent>>(
                            &std::fs::read(inputs[index].join("images.json"))?,
                        )?;
                        for frame in &mut frames {
                            frame.image = format!("{}/{}", stage.input[index], frame.image);
                        }
                    }
                    let meta = self.meta()?;
                    let mut sections = if frames.is_empty() {
                        vec![timeline::Section {
                            extra: Default::default(),
                            t: 0.,
                            end: meta.duration,
                            image: String::new(),
                            speech: transcript.events,
                        }]
                    } else {
                        timeline::merge(frames, transcript.events, meta.duration)
                    };
                    timeline::coalesce_sections(&mut sections);
                    anyhow::ensure!(
                        artifact::has_readable_body(&sections),
                        "No readable transcript"
                    );
                    let document = Document {
                        schema: 3,
                        meta,
                        sections,
                        extra: transcript.extra,
                    };
                    write_json(&target, &document)?;
                }
                StageType::Llm(_) => {
                    let ai = stage.ai(name).unwrap();
                    let provider = &self.providers[&ai.provider];
                    let mut settings = crate::llm::LlmSettings::for_stage(
                        &provider.definition,
                        &ai,
                        provider.key.clone(),
                    );
                    settings.base_url = provider.base(name).await?;
                    settings.limit = Some(provider.limit.clone());
                    let _cancel = runtime::CancelOnDrop(settings.cancelled.clone());
                    if stage.llm_writes_document() {
                        let mut document = Document::read(&inputs[0], &self.work)?;
                        let work = self.work.clone();
                        let (document, stage_warnings) =
                            crate::runtime::spawn_blocking(move || {
                                let warnings = crate::llm::paragraphs(
                                    &mut document.sections,
                                    &work,
                                    &settings,
                                    &ai,
                                )?;
                                anyhow::ensure!(
                                    artifact::has_readable_body(&document.sections),
                                    "Stage {} would remove every paragraph",
                                    ai.name
                                );
                                Ok::<_, anyhow::Error>((document, warnings))
                            })
                            .await??;
                        warnings = stage_warnings;
                        write_json(&target, &document)?;
                    } else {
                        let text = match self.kind(&stage.input[0]) {
                            Kind::Markdown => {
                                crate::summarize::markdown(
                                    &settings,
                                    &ai,
                                    &std::fs::read_to_string(&inputs[0])?,
                                )
                                .await?
                            }
                            kind => {
                                let events = if kind == Kind::Transcript {
                                    Transcript::read(&inputs[0])?.events
                                } else {
                                    Document::read(&inputs[0], &self.work)?
                                        .sections
                                        .into_iter()
                                        .flat_map(|s| s.speech)
                                        .collect()
                                };
                                crate::summarize::run(&settings, &ai, &events, &self.meta()?)
                                    .await?
                            }
                        };
                        std::fs::write(&target, text)?;
                    }
                }
                StageType::Markdown {} => {
                    let mut text = Vec::new();
                    for (input, path) in stage.input.iter().zip(&inputs) {
                        if self.kind(input) == Kind::Document {
                            let document = Document::read(path, &self.work)?;
                            text.push(crate::render::document(&document.sections));
                        } else {
                            text.push(std::fs::read_to_string(path)?);
                        }
                    }
                    std::fs::write(&target, crate::render::markdown(&text))?;
                }
                StageType::Run(program) => {
                    let (name, program, inputs, target, work) = (
                        name.to_owned(),
                        program.clone(),
                        inputs,
                        target.clone(),
                        self.work.clone(),
                    );
                    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let _cancel = runtime::CancelOnDrop(cancelled.clone());
                    crate::runtime::spawn_blocking(move || {
                        run_program(&name, &program, &inputs, &target, &work, &cancelled)
                    })
                    .await??;
                }
            }
        }
        runtime::check_cancelled()?;
        let output = self.work.join(&stage.output);
        if target.is_dir() || output.symlink_metadata().is_ok_and(|m| m.is_dir()) {
            replace_output(&target, &output)?;
        } else {
            std::fs::rename(&target, &output)?;
        }
        if !passthrough {
            progress::stage(name, "done");
        }
        Ok(warnings)
    }
    fn images(&self, stage: &Stage) -> Result<Vec<String>> {
        if stage.output_kind() == Kind::Document {
            return Ok(Document::read(&self.work.join(&stage.output), &self.work)?.images());
        }
        let mut images = Vec::new();
        if stage.output_kind() == Kind::Markdown {
            let text = std::fs::read_to_string(self.work.join(&stage.output))?;
            for input in &stage.input {
                let input = &self
                    .cfg
                    .stages
                    .iter()
                    .find(|(_, s)| &s.output == input)
                    .unwrap()
                    .1;
                images.extend(
                    self.images(input)?
                        .into_iter()
                        .filter(|image| text.contains(image)),
                );
            }
        }
        images.sort();
        images.dedup();
        Ok(images)
    }
    fn kind(&self, output: &str) -> Kind {
        self.cfg
            .stages
            .iter()
            .find(|(_, s)| s.output == output)
            .unwrap()
            .1
            .output_kind()
    }
    fn subtitle(
        &self,
        info: &source::Output,
        media: &Path,
        t: &settings::Transcript,
    ) -> Result<Option<(PathBuf, String)>> {
        if let Some(path) = &self.cfg.subtitle {
            return Ok(Some((
                path.clone(),
                if t.language == "auto" {
                    String::new()
                } else {
                    t.language.clone()
                },
            )));
        }
        let mut tracks = info.subtitle_tracks(media, &self.cfg.url)?;
        tracks.retain(|track| {
            t.subtitles.automatic || track.kind != subtitle::SubtitleKind::Automatic
        });
        subtitle::sort_tracks(
            &mut tracks,
            if t.language == "auto" {
                ""
            } else {
                &t.language
            },
            info.language.as_deref(),
        );
        Ok(tracks
            .into_iter()
            .next()
            .map(|track| (track.path, track.language.unwrap_or_default())))
    }
    async fn download(&self, name: &str, target: &Path) -> Result<()> {
        std::fs::create_dir(target)?;
        if Path::new(&self.cfg.url).is_file() {
            let path = Path::new(&self.cfg.url).canonicalize()?;
            let tracks = subtitle::sidecar_tracks(&path)
                .into_iter()
                .map(|t| serde_json::json!({"path":t.path,"language":t.language,"automatic":false}))
                .collect::<Vec<_>>();
            let duration = media::probe_duration(&path).await.unwrap_or(0.);
            return write_json(
                &target.join("info.json"),
                &serde_json::json!({"media":path,"title":path.file_stem().unwrap_or_default().to_string_lossy(),"duration":duration,"subtitles":tracks}),
            );
        }
        let cache = self.work.join(".cache/download");
        let cached = source::Output::read(&cache)?;
        let mut need_media = cached.is_none();
        if let Some(info) = &cached {
            match info.tracks(&cache.join("files")) {
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
                {
                    need_media = true
                }
                result => {
                    result?;
                }
            }
            for (stage_name, stage) in &self.cfg.stages {
                if need_media {
                    break;
                }
                if self.cfg.skip.contains(stage_name) {
                    continue;
                }
                let cfg = crate::options::stage_config(&self.cfg, stage_name, stage)?;
                match &stage.kind {
                    StageType::Screenshots(_) => {
                        need_media |= read_frames(
                            &self
                                .work
                                .join(format!(".cache/frames-{}", screenshot_key(&cfg)?)),
                        )?
                        .is_none()
                    }
                    StageType::Transcript(t) => {
                        let selected =
                            self.subtitle(info, &cache.join("files"), t)?
                                .filter(|(path, _)| {
                                    selected_transcript(path).is_ok_and(|(_, events)| {
                                        timeline::has_readable_events(&events)
                                    })
                                });
                        let subtitles_first = self.cfg.subtitle.is_some()
                            || t.sources.first().is_some_and(|s| s == "subtitles");
                        if !(subtitles_first && selected.is_some()) {
                            let key = runtime::digest(&serde_json::to_vec(
                                &checkpoint::AsrIdentity::new(&cfg.asr),
                            )?);
                            need_media |= Transcript::read(
                                &self.work.join(format!(".cache/transcript-{key}.json")),
                            )
                            .is_err();
                        }
                    }
                    StageType::Run(_)
                        if stage.input.iter().any(|p| self.kind(p) == Kind::Media) =>
                    {
                        need_media = true
                    }
                    _ => {}
                }
            }
        }
        if need_media {
            let resolved = source::resolve(&self.cfg.url, &self.cfg.source, &cache, name).await?;
            tracing::debug!(media = %resolved.media.display(), title = %resolved.meta.title, "download ready");
        }
        if cache.join("files").is_dir() {
            artifact::copy_path(&cache.join("files"), target)?;
        }
        artifact::copy_path(&cache.join("complete.json"), &target.join("info.json"))?;
        Ok(())
    }
    async fn transcript(
        &self,
        name: &str,
        stage: &Stage,
        t: &settings::Transcript,
        input: &Path,
        target: &Path,
    ) -> Result<()> {
        let info = source::Output::info(input)?;
        let selected = if self.cfg.subtitle.is_some() {
            self.subtitle(&info, input, t)?
        } else if t.sources.iter().any(|s| s == "subtitles") {
            progress::stage_label(
                &format!("{name}/subtitles"),
                "start",
                "Looking for subtitles",
            );
            let selected = self.subtitle(&info, input, t)?;
            progress::stage_label(
                &format!("{name}/subtitles"),
                "done",
                "Looking for subtitles",
            );
            selected
        } else {
            None
        };
        let mut cfg = crate::options::stage_config(&self.cfg, name, stage)?;
        for source in if self.cfg.subtitle.is_some() {
            vec!["subtitles".to_owned()]
        } else {
            t.sources.clone()
        } {
            let (key, language, selected_events) = if source == "subtitles" {
                let Some((path, language)) = &selected else {
                    continue;
                };
                let (key, events) = selected_transcript(path)?;
                if !timeline::has_readable_events(&events) && self.cfg.subtitle.is_none() {
                    continue;
                }
                subtitle::validate_events(&events)?;
                (key, language.clone(), Some(events))
            } else {
                (
                    runtime::digest(&serde_json::to_vec(&checkpoint::AsrIdentity::new(
                        &cfg.asr,
                    ))?),
                    cfg.asr.language.clone(),
                    None,
                )
            };
            let lock = self.cache_lock(&format!("transcript-{key}"));
            let _lock = lock.lock().await;
            let cache = self.work.join(format!(".cache/transcript-{key}.json"));
            if cache.is_file()
                && let Ok(mut transcript) = Transcript::read(&cache)
            {
                transcript.language = language;
                write_json(target, &transcript)?;
                return Ok(());
            }
            let events = if let Some(events) = selected_events {
                events
            } else {
                let audio = self.work.join(format!(".cache/audio-{name}.wav"));
                crate::error::require_cmd("ffmpeg")?;
                crate::error::require_cmd("ffprobe")?;
                progress::stage_label(&format!("{name}/audio"), "start", "Extracting audio");
                media::extract_audio(&info.file(input, &self.cfg.url)?, &audio).await?;
                progress::stage_label(&format!("{name}/audio"), "done", "Extracting audio");
                cfg.out_dir = self.work.join(format!(".cache/asr-{key}"));
                let provider = &self.providers[&t.speech.provider];
                cfg.asr.base_url = provider.base(name).await?;
                cfg.asr.api_key = provider.key.clone();
                cfg.asr.command.clear();
                cfg.asr.limit = Some(provider.limit.clone());
                progress::stage_label(&format!("{name}/speech"), "start", "Transcribing speech");
                let events = asr::run(&cfg, &audio).await?;
                progress::stage_label(&format!("{name}/speech"), "done", "Transcribing speech");
                events
            };
            if !timeline::has_readable_events(&events) {
                continue;
            }
            subtitle::validate_events(&events)?;
            let transcript = Transcript {
                events,
                language,
                extra: Default::default(),
            };
            checkpoint::atomic_write(&cache, &serde_json::to_vec(&transcript)?)?;
            return write_json(target, &transcript);
        }
        if t.sources.iter().any(|s| s == "speech") {
            anyhow::bail!("No speech was recognized in the recording");
        }
        anyhow::bail!("No subtitles found. Select existing subtitles, or use speech recognition.")
    }
    async fn screenshots(
        &self,
        name: &str,
        stage: &Stage,
        input: &Path,
        target: &Path,
    ) -> Result<Vec<String>> {
        let mut cfg = crate::options::stage_config(&self.cfg, name, stage)?;
        let cache = self
            .work
            .join(format!(".cache/frames-{}", screenshot_key(&cfg)?));
        let lock = self.cache_lock(&format!("frames-{}", screenshot_key(&cfg)?));
        let _lock = lock.lock().await;
        cfg.out_dir = cache.clone();
        let mut warnings = Vec::new();
        let frames = match read_frames(&cache)? {
            Some(output) => output,
            None => {
                crate::error::require_cmd("ffmpeg")?;
                crate::error::require_cmd("ffprobe")?;
                let info = source::Output::info(input)?;
                match scene::run(&cfg, &info.file(input, &self.cfg.url)?).await {
                    Ok(output) => {
                        checkpoint::atomic_write(
                            &cache.join("complete.json"),
                            &serde_json::to_vec(&output)?,
                        )?;
                        output
                    }
                    Err(error) => {
                        warnings.push(format!("Screenshots: {error:#}"));
                        scene::Output::NoVideo
                    }
                }
            }
        };
        std::fs::create_dir(target)?;
        let mut images = Vec::new();
        if let scene::Output::Frames(frames) = frames {
            if frames.is_empty() {
                warnings.push(NO_SCREENSHOTS.into());
            }
            for (i, frame) in frames.iter().enumerate() {
                let image = format!("{:04}.jpg", i + 1);
                artifact::copy_path(
                    &artifact::safe_asset_path(&cache, &frame.image)?,
                    &target.join(&image),
                )?;
                images.push(timeline::FrameEvent { t: frame.t, image });
            }
        }
        write_json(&target.join("images.json"), &images)?;
        Ok(warnings)
    }
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    std::fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
fn replace_output(source: &Path, target: &Path) -> Result<()> {
    let old = tempfile::tempdir_in(target.parent().unwrap())?;
    let backup = old.path().join("old");
    if target.symlink_metadata().is_ok() {
        std::fs::rename(target, &backup)?;
    }
    if let Err(error) = std::fs::rename(source, target) {
        if backup.symlink_metadata().is_ok() {
            std::fs::rename(backup, target)?;
        }
        return Err(error.into());
    }
    Ok(())
}
fn run_program(
    name: &str,
    program: &settings::Run,
    inputs: &[PathBuf],
    target: &Path,
    work: &Path,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<()> {
    use std::{
        io::{Read, Seek},
        process::{Command, Stdio},
        time::Duration,
    };
    let mut args = Vec::new();
    for arg in &program.command {
        match arg.as_str() {
            "{input}" => args.extend(inputs.iter().map(|p| p.as_os_str().to_owned())),
            "{output}" => args.push(target.as_os_str().to_owned()),
            _ => args.push(arg.into()),
        }
    }
    let (executable, args) = args
        .split_first()
        .context("The adapter command must name an executable.")?;
    let executable = crate::adapter::resolve_executable(&executable.to_string_lossy())?;
    let mut command = Command::new(executable);
    command.args(args);
    if !program.command.iter().any(|s| s == "{input}")
        && let Some(input) = inputs.first()
    {
        command.stdin(std::fs::File::open(input)?);
    } else {
        command.stdin(Stdio::null());
    }
    let has_output = program.command.iter().any(|s| s == "{output}");
    let mut stdout = tempfile::tempfile()?;
    command.stdout(stdout.try_clone()?);
    command.stderr(Stdio::piped());
    let mut child = runtime::ManagedChild::spawn_tree("run", &mut command)?;
    let stderr = runtime::StderrTail::default();
    let drain = stderr.drain(
        child.take_stderr().context("Missing program stderr pipe")?,
        "run",
    );
    let status = loop {
        anyhow::ensure!(
            !cancelled.load(std::sync::atomic::Ordering::Relaxed),
            "Cancelled"
        );
        runtime::check_cancelled()?;
        if let Some(status) = child.try_wait() {
            break status;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    child.stop();
    let _ = drain.join();
    let mut apply = || -> Result<()> {
        anyhow::ensure!(
            status.success(),
            "{}",
            crate::error::cmd_error(name, status.code(), &stderr.tail())
        );
        if !has_output {
            stdout.rewind()?;
            std::io::copy(&mut stdout, &mut std::fs::File::create(target)?)?;
        }
        anyhow::ensure!(
            target
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.is_file()),
            "Stage {name}: the program did not write {}",
            target.file_name().unwrap().to_string_lossy()
        );
        match program.produces {
            Some(Kind::Document) => {
                Document::read(target, work)?;
                Ok(())
            }
            Some(Kind::Transcript) => {
                Transcript::read(target)?;
                Ok(())
            }
            Some(Kind::Markdown) => {
                let mut text = String::new();
                std::fs::File::open(target)?.read_to_string(&mut text)?;
                Ok(())
            }
            _ => Ok(()),
        }
    };
    apply().map_err(|error| stderr.attach(error, name))
}
async fn start_ai_adapter(argv: Vec<String>, stage: String) -> Result<crate::adapter::Adapter> {
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _cancel_on_drop = runtime::CancelOnDrop(cancelled.clone());
    crate::runtime::spawn_blocking(move || {
        let check = || {
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(runtime::Cancelled.into());
            }
            runtime::check_cancelled()
        };
        check()?;
        crate::adapter::Adapter::start_with_control(&argv, &stage, &check)
    })
    .await?
}
fn cleanup_media(cfg: &PipelineConfig, work: &Path, warnings: &mut Vec<String>) -> Result<()> {
    if work
        .join(".cache")
        .symlink_metadata()
        .is_ok_and(|m| m.is_symlink())
    {
        remove_temporary(work, Path::new(".cache"), false, warnings);
        return Ok(());
    }
    let audio_cleanup = (|| -> std::io::Result<()> {
        for entry in std::fs::read_dir(work.join(".cache"))? {
            let name = entry?.file_name();
            if name
                .to_str()
                .is_some_and(|s| s.starts_with("audio-") && s.ends_with(".wav"))
            {
                remove_temporary(work, &Path::new(".cache").join(name), false, warnings);
            }
        }
        Ok(())
    })();
    if let Err(error) = audio_cleanup
        && error.kind() != std::io::ErrorKind::NotFound
    {
        warnings.push(format!(
            "Could not remove cached media {}: {error}",
            work.join(".cache").display()
        ));
    }
    // Downloaded media is temporary even on failure; local files are never removed.
    if !Path::new(&cfg.url).is_file() {
        if work
            .join(".cache/download")
            .symlink_metadata()
            .is_ok_and(|m| m.is_symlink())
        {
            remove_temporary(work, Path::new(".cache/download"), false, warnings);
            return Ok(());
        }
        if let Some(output) = source::Output::read(&work.join(".cache/download"))? {
            remove_temporary(
                work,
                &Path::new(".cache/download/files").join(&output.media),
                false,
                warnings,
            );
            for (_, stage) in &cfg.stages {
                if matches!(stage.kind, StageType::Download(_)) {
                    remove_temporary(
                        work,
                        &Path::new(&stage.output).join(&output.media),
                        false,
                        warnings,
                    );
                }
            }
        } else {
            remove_temporary(work, Path::new(".cache/download/files"), true, warnings);
        }
    }
    Ok(())
}
fn remove_temporary(root: &Path, relative: &Path, recursive: bool, warnings: &mut Vec<String>) {
    let remove = || -> std::io::Result<()> {
        let mut path = root.to_owned();
        if path.symlink_metadata()?.file_type().is_symlink() {
            return std::fs::remove_dir_all(path);
        }
        for component in relative.components() {
            if !matches!(component, std::path::Component::Normal(_)) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Invalid note asset path",
                ));
            }
            path.push(component);
            if path.symlink_metadata()?.file_type().is_symlink() {
                return std::fs::remove_dir_all(path);
            }
        }
        if recursive {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        }
    };
    if let Err(error) = remove()
        && error.kind() != std::io::ErrorKind::NotFound
    {
        warnings.push(format!(
            "Could not remove cached media {}: {error}",
            root.join(relative).display()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn incomplete_download_cleanup_removes_files_and_leaves_link_targets() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        let files = work.join(".cache/download/files");
        std::fs::create_dir_all(&files).unwrap();
        let outside = root.path().join("outside.wav");
        std::fs::write(&outside, "user data").unwrap();
        std::fs::write(files.join("partial.wav"), "partial download").unwrap();
        symlink(&outside, files.join("link.wav")).unwrap();
        let mut file = settings::ConfigFile::default();
        file.stages.get_mut("download").unwrap().kind = StageType::Download(settings::Source {
            command: Some(vec!["unused".into()]),
        });
        let cfg = crate::options::resolve(
            "https://example.test/media".into(),
            &Default::default(),
            &file,
        )
        .unwrap();
        let mut warnings = Vec::new();
        cleanup_media(&cfg, &work, &mut warnings).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(!files.exists());
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "user data");
    }
    #[cfg(unix)]
    #[test]
    fn audio_cleanup_covers_old_stage_names_and_unlinks_symlinks() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        std::fs::create_dir_all(work.join(".cache")).unwrap();
        let source = root.path().join("local.wav");
        std::fs::write(&source, "local media").unwrap();
        let cfg = crate::options::resolve(
            source.to_string_lossy().into(),
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
        std::fs::write(work.join(".cache/audio-old-stage.wav"), "audio").unwrap();
        symlink(&source, work.join(".cache/audio-transcript.wav")).unwrap();
        let mut warnings = Vec::new();
        cleanup_media(&cfg, &work, &mut warnings).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(std::fs::read_dir(work.join(".cache")).unwrap().count(), 0);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "local media");
        std::fs::remove_dir(work.join(".cache")).unwrap();
        symlink(root.path(), work.join(".cache")).unwrap();
        cleanup_media(&cfg, &work, &mut warnings).unwrap();
        assert!(work.join(".cache").symlink_metadata().is_err());
        assert!(source.is_file());
        cleanup_media(&cfg, &work, &mut warnings).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        std::fs::write(work.join(".cache"), "not a directory").unwrap();
        cleanup_media(&cfg, &work, &mut warnings).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("Could not remove cached media "));
    }
    #[cfg(unix)]
    #[test]
    fn cleanup_unlinks_directory_and_file_links_without_touching_their_targets() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let victim = outside.join("victim.wav");
        std::fs::write(&victim, "USER DATA").unwrap();
        for (index, link) in [
            ".cache",
            ".cache/download/files/nested",
            "media/nested",
            "victim.wav",
        ]
        .iter()
        .enumerate()
        {
            let work = root.path().join(format!("work-{index}"));
            std::fs::create_dir(&work).unwrap();
            let link = work.join(link);
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            let target = if index == 3 { &victim } else { &outside };
            symlink(target, &link).unwrap();
            let relative = if index == 0 {
                PathBuf::from(".cache/victim.wav")
            } else if index == 3 {
                PathBuf::from("victim.wav")
            } else {
                link.strip_prefix(&work).unwrap().join("victim.wav")
            };
            let mut warnings = Vec::new();
            remove_temporary(&work, &relative, false, &mut warnings);
            assert!(warnings.is_empty(), "{warnings:?}");
            assert!(link.symlink_metadata().is_err());
            assert_eq!(std::fs::read_to_string(&victim).unwrap(), "USER DATA");
            std::fs::write(work.join("ordinary.wav"), "temporary").unwrap();
            remove_temporary(&work, Path::new("ordinary.wav"), false, &mut warnings);
            assert!(!work.join("ordinary.wav").exists());
        }
    }

    #[test]
    fn cache_keys_track_their_inputs_without_tracking_later_stages() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("local");
        std::fs::write(&path, "original").unwrap();
        let mut source = settings::Source {
            command: Some(vec!["adapter".into()]),
        };
        let key = source_key(path.to_str().unwrap(), &source).unwrap();
        std::fs::write(&path, "changed content").unwrap();
        assert_ne!(key, source_key(path.to_str().unwrap(), &source).unwrap());
        let url = "https://example.test/video";
        let key = source_key(url, &source).unwrap();
        assert_eq!(
            key,
            runtime::digest(
                &serde_json::to_vec(&serde_json::json!({"url":url,"command":["adapter"]})).unwrap()
            )[..16]
        );
        source.command.as_mut().unwrap().push("--option".into());
        assert_ne!(key, source_key(url, &source).unwrap());
        let mut file = settings::ConfigFile::default();
        file.stages.get_mut("download").unwrap().kind = StageType::Download(source);
        let cfg = crate::options::resolve(url.into(), &Default::default(), &file).unwrap();
        let stage = &cfg
            .stages
            .iter()
            .find(|(n, _)| n == "screenshots")
            .unwrap()
            .1;
        let mut cfg = crate::options::stage_config(&cfg, "screenshots", stage).unwrap();
        let key = screenshot_key(&cfg).unwrap();
        cfg.asr.model = "changed".into();
        assert_eq!(key, screenshot_key(&cfg).unwrap());
        cfg.similarity = 0.5;
        assert_ne!(key, screenshot_key(&cfg).unwrap());
    }
    #[test]
    fn empty_transcripts_and_missing_screenshots_are_cache_misses() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("transcript.json");
        std::fs::write(&path, r#"{"language":"","events":[]}"#).unwrap();
        assert!(Transcript::read(&path).is_err());
        for value in [r#"[{"t":0,"image":"missing.jpg"}]"#, "[]"] {
            std::fs::write(root.path().join("complete.json"), value).unwrap();
            assert!(read_frames(root.path()).unwrap().is_none());
        }
        std::fs::write(root.path().join("complete.json"), "null").unwrap();
        assert!(matches!(
            read_frames(root.path()).unwrap(),
            Some(scene::Output::NoVideo)
        ));
    }
}
