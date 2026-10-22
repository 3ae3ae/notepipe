//! Apply CLI overrides, then validate the effective pipeline before source work.
use crate::{
    cli::RunOpts,
    config,
    settings::{self, Stage},
};
use anyhow::Result;

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
    let mut names = std::collections::HashSet::new();
    let mut stages = Vec::new();
    for name in file
        .pipeline
        .iter()
        .filter(|name| !opts.skip.contains(name))
    {
        let stage = file
            .stages
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("pipeline: no stage named {name} in [stages]"))?;
        anyhow::ensure!(names.insert(name), "pipeline: {name} is listed twice");
        stages.push((name.clone(), stage.clone()));
    }
    let mut download_name = None;
    let mut transcript_name = None;
    let mut screenshots_name = None;
    let mut note_name = None;
    let mut transcript = settings::Transcript::default();
    let mut screenshots = settings::Screenshots::default();
    let mut download = settings::Source::default();
    let mut out_root = std::path::PathBuf::from(config::DEFAULT_OUT_DIR);
    let violation = stages
        .iter()
        .enumerate()
        .flat_map(|(index, earlier)| {
            stages[index + 1..]
                .iter()
                .filter(move |later| earlier.1.rank() > later.1.rank())
                .map(move |later| (earlier, later))
        })
        .min_by_key(|(_, later)| later.1.rank());
    if let Some(((name, _), (other, _))) = violation {
        anyhow::bail!("pipeline: {name} must come after {other}");
    }
    for (name, stage) in &stages {
        let slot = match stage {
            Stage::Download(p) => {
                download.command = p.argv();
                Some((&mut download_name, "download"))
            }
            Stage::Transcript(t) => {
                transcript = t.clone();
                Some((&mut transcript_name, "transcript"))
            }
            Stage::Screenshots(s) => {
                screenshots = s.clone();
                Some((&mut screenshots_name, "screenshots"))
            }
            Stage::Note(n) => {
                anyhow::ensure!(
                    note_name.is_none(),
                    "pipeline: exactly one note stage must run"
                );
                note_name = Some(name.clone());
                out_root = n.dir.clone();
                None
            }
            _ => None,
        };
        if let Some((slot, kind)) = slot {
            anyhow::ensure!(slot.is_none(), "pipeline: only one {kind} stage can run");
            *slot = Some(name.clone());
        }
    }
    let note_name =
        note_name.ok_or_else(|| anyhow::anyhow!("pipeline: exactly one note stage must run"))?;
    anyhow::ensure!(
        transcript_name.is_some() || opts.subtitle.is_some(),
        "pipeline: a transcript stage must run, or pass --subtitle"
    );
    for (option, given) in [
        ("--screenshot-mode", opts.screenshot_mode.is_some()),
        ("--sensitivity", opts.similarity.is_some()),
        ("--frame-interval", opts.sample_interval.is_some()),
        ("--min-gap", opts.cooldown.is_some()),
        ("--settle", opts.stable_secs.is_some()),
        ("--region", opts.roi.is_some()),
    ] {
        anyhow::ensure!(
            !given || screenshots_name.is_some(),
            "{option} needs a screenshots stage in the pipeline"
        );
    }
    if let Some(language) = &opts.asr_language {
        transcript.language = language.clone();
    }
    let speech = &transcript.speech;
    let provider = file.providers.get(&speech.provider);
    let asr = settings::Asr {
        mode: speech.api,
        model: speech.model.clone(),
        language: if transcript.language == "auto" {
            String::new()
        } else {
            transcript.language.clone()
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
    let out_root = config::expand_tilde(opts.out.clone().unwrap_or(out_root));
    let cfg = config::PipelineConfig {
        url: source,
        download_name,
        transcript_name,
        screenshots_name,
        note_name,
        transcript,
        source: download,
        out_dir: out_root.clone(),
        out_root,
        similarity: opts.similarity.unwrap_or(screenshots.sensitivity),
        sample_interval: opts.sample_interval.unwrap_or(screenshots.frame_interval),
        cooldown: opts.cooldown.unwrap_or(screenshots.min_gap),
        screenshot_mode: opts.screenshot_mode.unwrap_or(screenshots.mode),
        stable_secs: opts.stable_secs.unwrap_or(screenshots.settle),
        roi: opts
            .roi
            .as_ref()
            .or(screenshots.region.as_ref())
            .map(|r| config::Roi::parse(r))
            .transpose()?,
        no_resume: opts.no_resume,
        subtitle: opts.subtitle.clone(),
        stages,
        providers: file.providers.clone(),
        asr,
    };
    cfg.validate()?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overrides_reach_the_named_stages_and_speech_cache_identity() {
        let file = settings::ConfigFile::default();
        let opts = RunOpts {
            out: Some("notes".into()),
            no_resume: true,
            asr_language: Some("ko".into()),
            screenshot_mode: Some(config::ScreenshotMode::First),
            similarity: Some(0.7),
            sample_interval: Some(0.5),
            cooldown: Some(3.0),
            stable_secs: Some(0.4),
            roi: Some("10%,0%-100%,100%".into()),
            ..Default::default()
        };
        let cfg = resolve("video".into(), &opts, &file).unwrap();
        assert_eq!(cfg.out_root, std::path::Path::new("notes"));
        assert!(cfg.no_resume);
        assert_eq!(cfg.transcript.language, "ko");
        assert_eq!(crate::checkpoint::AsrIdentity::new(&cfg.asr).language, "ko");
        assert_eq!(
            (
                cfg.similarity,
                cfg.sample_interval,
                cfg.cooldown,
                cfg.stable_secs
            ),
            (0.7, 0.5, 3.0, 0.4)
        );
        assert_eq!(cfg.screenshot_mode, config::ScreenshotMode::First);
        assert!(cfg.roi.is_some());
        let cfg = resolve(
            String::new(),
            &RunOpts {
                asr_language: Some("auto".into()),
                ..Default::default()
            },
            &file,
        )
        .unwrap();
        assert!(cfg.asr.language.is_empty());
    }
}
