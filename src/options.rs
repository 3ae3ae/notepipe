//! Apply CLI overrides, then validate file dependencies before source work.
use crate::{
    cli::RunOpts,
    config,
    settings::{self, Kind, StageType},
};
use anyhow::Result;
use std::collections::{BTreeMap, HashSet};

pub fn resolve(
    source: String,
    opts: &RunOpts,
    file: &settings::ConfigFile,
) -> Result<config::PipelineConfig> {
    file.validate()?;
    for name in &opts.skip {
        anyhow::ensure!(
            file.pipeline.contains(name),
            "--skip: no stage named {name} in the pipeline"
        );
    }
    let mut names = HashSet::new();
    let mut stages = Vec::new();
    let mut written = BTreeMap::new();
    let mut absent = BTreeMap::new();
    let mut download = settings::Source::default();
    let mut downloads = 0;
    for name in &file.pipeline {
        let mut stage = file
            .stages
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("pipeline: no stage named {name} in [stages]"))?
            .clone();
        anyhow::ensure!(names.insert(name), "pipeline: {name} is listed twice");
        if let StageType::Download(p) = &stage.kind
            && !opts.skip.contains(name)
        {
            downloads += 1;
            anyhow::ensure!(downloads <= 1, "pipeline: only one download stage can run");
            download.command = p.command.clone();
        }
        for input in &stage.input {
            if input == "source" {
                anyhow::ensure!(
                    matches!(stage.kind, StageType::Download(_)),
                    "Stage {name}: only a download stage reads source"
                );
                continue;
            }
            let kind = *written
                .get(input)
                .ok_or_else(|| anyhow::anyhow!("Stage {name}: no earlier stage writes {input}"))?;
            let (typ, accepted): (&str, &[Kind]) = match &stage.kind {
                StageType::Download(_) => ("download", &[]),
                StageType::Transcript(_) => ("transcript", &[Kind::Media]),
                StageType::Screenshots(_) => ("screenshots", &[Kind::Media]),
                StageType::Document {} => ("document", &[Kind::Transcript, Kind::Images]),
                StageType::Llm(_) if stage.output.ends_with(".json") => ("llm", &[Kind::Document]),
                StageType::Llm(_) => ("llm", &[Kind::Transcript, Kind::Document, Kind::Markdown]),
                StageType::Markdown {} => ("markdown", &[Kind::Document, Kind::Markdown]),
                StageType::Run(_) => (
                    "run",
                    &[
                        Kind::Media,
                        Kind::Transcript,
                        Kind::Images,
                        Kind::Document,
                        Kind::Markdown,
                    ],
                ),
            };
            anyhow::ensure!(
                accepted.contains(&kind),
                "Stage {name} reads {input}, which is {kind}; a {typ} stage reads {}",
                accepted
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" or ")
            );
        }
        let input_kinds = stage
            .input
            .iter()
            .filter_map(|p| written.get(p).copied())
            .collect::<Vec<_>>();
        let (valid_count, rule) = match &stage.kind {
            StageType::Download(_) => (stage.input == ["source"], "a download stage reads source"),
            StageType::Transcript(_) => {
                (stage.input.len() == 1, "a transcript stage reads one input")
            }
            StageType::Screenshots(_) => (
                stage.input.len() == 1,
                "a screenshots stage reads one input",
            ),
            StageType::Llm(_) => (stage.input.len() == 1, "a llm stage reads one input"),
            StageType::Document {} => (
                input_kinds
                    .iter()
                    .filter(|k| **k == Kind::Transcript)
                    .count()
                    == 1
                    && input_kinds.iter().filter(|k| **k == Kind::Images).count() <= 1,
                "a document stage reads one transcript and at most one images directory",
            ),
            StageType::Markdown {} => (
                !stage.input.is_empty(),
                "a markdown stage reads at least one input",
            ),
            StageType::Run(_) => (true, ""),
        };
        anyhow::ensure!(valid_count, "Stage {name}: {rule}");
        if let StageType::Run(p) = &stage.kind {
            anyhow::ensure!(
                p.command.iter().any(|s| s == "{input}")
                    || (stage.input.len() <= 1
                        && !input_kinds
                            .iter()
                            .any(|k| matches!(k, Kind::Media | Kind::Images))),
                "Stage {name}: standard input takes one file; use {{input}} in command for more"
            );
        }
        let skipped = opts.skip.contains(name);
        if !skipped || (matches!(stage.kind, StageType::Llm(_)) && stage.output.ends_with(".json"))
        {
            stage.input.retain(|input| {
                if absent.contains_key(input) {
                    !(matches!(stage.kind, StageType::Markdown {})
                        || (matches!(stage.kind, StageType::Document {})
                            && written[input] == Kind::Images))
                } else {
                    true
                }
            });
            for input in &stage.input {
                if let Some(skipped) = absent.get(input) {
                    anyhow::bail!("--skip {skipped}: {name} needs {input}");
                }
            }
            anyhow::ensure!(
                !matches!(stage.kind, StageType::Markdown {}) || !stage.input.is_empty(),
                "--skip leaves stage {name} with no input"
            );
        }
        if skipped && !(matches!(stage.kind, StageType::Llm(_)) && stage.output.ends_with(".json"))
        {
            absent.insert(stage.output.clone(), name.clone());
        }
        match &mut stage.kind {
            StageType::Transcript(t) => {
                if let Some(language) = &opts.asr_language {
                    t.language = language.clone();
                }
            }
            StageType::Screenshots(s) => {
                if let Some(v) = opts.screenshot_mode {
                    s.mode = v;
                }
                if let Some(v) = opts.similarity {
                    s.sensitivity = v;
                }
                if let Some(v) = opts.sample_interval {
                    s.frame_interval = v;
                }
                if let Some(v) = opts.cooldown {
                    s.min_gap = v;
                }
                if let Some(v) = opts.stable_secs {
                    s.settle = v;
                }
                if let Some(v) = &opts.roi {
                    s.region = Some(v.clone());
                }
            }
            _ => {}
        }
        written.insert(stage.output.clone(), stage.output_kind());
        stages.push((name.clone(), stage));
    }
    for (option, given, matching) in [
        ("--subtitle", opts.subtitle.is_some(), false),
        ("--language", opts.asr_language.is_some(), false),
        ("--screenshot-mode", opts.screenshot_mode.is_some(), true),
        ("--sensitivity", opts.similarity.is_some(), true),
        ("--frame-interval", opts.sample_interval.is_some(), true),
        ("--min-gap", opts.cooldown.is_some(), true),
        ("--settle", opts.stable_secs.is_some(), true),
        ("--region", opts.roi.is_some(), true),
    ] {
        let kind = if matching {
            "screenshots"
        } else {
            "transcript"
        };
        anyhow::ensure!(
            !given
                || stages.iter().any(|(name, s)| !opts.skip.contains(name)
                    && if matching {
                        matches!(s.kind, StageType::Screenshots(_))
                    } else {
                        matches!(s.kind, StageType::Transcript(_))
                    }),
            "{option} needs a {kind} stage in the pipeline"
        );
    }
    let published = stages
        .iter()
        .filter(|(_, s)| !absent.contains_key(&s.output))
        .filter_map(|(_, s)| s.publish.as_deref())
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !published.is_empty(),
        "Nothing is published; set publish on a stage"
    );
    anyhow::ensure!(
        !opts.json || !published.contains(&"-"),
        "Publishing to standard output and --json both use standard output; use one of them"
    );
    if source.starts_with("http://") || source.starts_with("https://") {
        anyhow::ensure!(
            stages
                .iter()
                .any(|(n, s)| !opts.skip.contains(n) && matches!(s.kind, StageType::Download(_))),
            "{}",
            crate::source::ADAPTER_HINT
        );
    }
    let out_root = config::expand_tilde(opts.out.clone().unwrap_or_else(|| file.out.clone()));
    let out_root = std::path::absolute(out_root)?;
    let cfg = config::PipelineConfig {
        url: source,
        source: download,
        transcript_name: None,
        screenshots_name: None,
        out_dir: out_root.clone(),
        out_root,
        similarity: config::DEFAULT_SIMILARITY,
        sample_interval: config::DEFAULT_SAMPLE_INTERVAL,
        cooldown: config::DEFAULT_COOLDOWN,
        screenshot_mode: Default::default(),
        stable_secs: config::DEFAULT_STABLE_SECS,
        roi: None,
        no_resume: opts.no_resume,
        subtitle: opts.subtitle.clone(),
        stages,
        providers: file.providers.clone(),
        asr: Default::default(),
        skip: opts.skip.clone(),
        verbose: opts.verbose > 0,
    };
    for (name, stage) in &cfg.stages {
        stage_config(&cfg, name, stage)?.validate()?;
    }
    Ok(cfg)
}

pub fn stage_config(
    cfg: &config::PipelineConfig,
    name: &str,
    stage: &settings::Stage,
) -> Result<config::PipelineConfig> {
    let mut cfg = cfg.clone();
    match &stage.kind {
        StageType::Transcript(t) => {
            cfg.transcript_name = Some(name.into());
            let speech = &t.speech;
            let provider = cfg.providers.get(&speech.provider);
            cfg.asr = settings::Asr {
                mode: speech.api,
                model: speech.model.clone(),
                language: if t.language == "auto" {
                    String::new()
                } else {
                    t.language.clone()
                },
                base_url: provider
                    .and_then(|p| p.base_url.clone())
                    .unwrap_or_default(),
                concurrency: provider.map_or(4, |p| p.concurrency as usize),
                command: provider.map_or_else(Vec::new, |p| p.argv()),
                max_speech: speech.max_segment,
                vad_threshold: speech.threshold,
                min_silence: speech.min_silence,
                ..Default::default()
            };
        }
        StageType::Screenshots(s) => {
            cfg.screenshots_name = Some(name.into());
            cfg.similarity = s.sensitivity;
            cfg.sample_interval = s.frame_interval;
            cfg.cooldown = s.min_gap;
            cfg.screenshot_mode = s.mode;
            cfg.stable_secs = s.settle;
            cfg.roi = s
                .region
                .as_ref()
                .map(|r| config::Roi::parse(r))
                .transpose()?;
        }
        _ => {}
    }
    Ok(cfg)
}
