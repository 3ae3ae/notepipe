//! Merge file settings with CLI overrides.
use crate::cli::RunOpts;
use crate::{config, llm, settings};

fn resolve_stages(opts: &RunOpts, file: &settings::ConfigFile) -> anyhow::Result<Vec<llm::Stage>> {
    let mut stages = file.stages.clone();
    if opts.no_stages {
        stages.clear();
    } else if let Some(names) = &opts.stages {
        stages = names
            .iter()
            .map(|name| {
                file.stages
                    .iter()
                    .find(|stage| stage.name == *name)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("Unknown stage: {name}; the configuration has no [[stages]] block with that name"))
            })
            .collect::<anyhow::Result<_>>()?;
    }
    Ok(stages)
}

/// Precedence: explicit options, then the file, then built-in defaults.
pub fn resolve(
    source: String,
    opts: &RunOpts,
    file: &settings::ConfigFile,
) -> anyhow::Result<config::PipelineConfig> {
    for (name, service) in &file.services {
        service.validate(name)?;
    }
    llm::validate(&file.stages, &file.services)?;
    anyhow::ensure!(
        file.speech.service.is_empty() || file.services.contains_key(&file.speech.service),
        "[speech].service: no service named {} in [services]",
        file.speech.service
    );
    let screenshots = &file.screenshots;
    use config::ScreenshotMode;
    let out_root = opts
        .out
        .clone()
        .or_else(|| file.output.dir.clone())
        .map(config::expand_tilde)
        .unwrap_or_else(|| config::DEFAULT_OUT_DIR.into());
    Ok(config::PipelineConfig {
        url: source,
        config_path: opts.config.clone(),
        on_complete: file.output.on_complete.clone(),
        source: file.download.clone(),
        // The pipeline assigns the per-source work directory before processing.
        out_dir: out_root.clone(),
        out_root,
        similarity: opts
            .similarity
            .or(screenshots.sensitivity)
            .unwrap_or(config::DEFAULT_SIMILARITY),
        sample_interval: opts
            .sample_interval
            .or(screenshots.frame_interval)
            .unwrap_or(config::DEFAULT_SAMPLE_INTERVAL),
        cooldown: opts
            .cooldown
            .or(screenshots.min_gap)
            .unwrap_or(config::DEFAULT_COOLDOWN),
        screenshot_mode: opts
            .screenshot_mode
            .or(screenshots.mode)
            .unwrap_or(ScreenshotMode::Stable),
        stable_secs: opts
            .stable_secs
            .or(screenshots.settle)
            .unwrap_or(config::DEFAULT_STABLE_SECS),
        roi: opts
            .roi
            .as_ref()
            .or_else(|| {
                screenshots
                    .region
                    .as_ref()
                    .filter(|region| !region.is_empty())
            })
            .map(|region| config::Roi::parse(region))
            .transpose()?,
        no_resume: opts.no_resume,
        subtitle: opts.subtitle.clone(),
        stages: resolve_stages(opts, file)?,
        services: file.services.clone(),
        asr: resolve_asr(opts, file),
        transcript_source: opts
            .transcript_source
            .or(file.transcript.source)
            .unwrap_or_default(),
    })
}

/// Resolve the speech connection without reading environment credentials.
fn resolve_asr(opts: &RunOpts, file: &settings::ConfigFile) -> crate::settings::Asr {
    let speech = &file.speech;
    let service = file
        .services
        .get(&speech.service)
        .cloned()
        .unwrap_or_default();
    let mut a = settings::Asr {
        service: speech.service.clone(),
        api_key_env: service.api_key_env,
        mode: speech.mode,
        base_url: service.base_url,
        api_key: service.api_key,
        model: speech.model.clone(),
        language: file.transcript.language.clone(),
        concurrency: service.concurrency as usize,
        command: service.adapter,
        max_speech: speech.detection.max_segment,
        vad_threshold: speech.detection.threshold,
        min_silence: speech.detection.min_silence,
    };
    if let Some(v) = &opts.asr_language {
        a.language = v.clone();
    }
    if let Some(v) = opts.max_speech {
        a.max_speech = v;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_flags_override_config_in_order() {
        let mut file = settings::ConfigFile::default();
        file.services.insert("ai".into(), Default::default());
        let mut proofread = serde_json::from_value::<crate::llm::Stage>(serde_json::json!({"name":"proofread","service":"ai","input":"paragraphs","output":"replace","prompt":"CUSTOM"})).unwrap();
        proofread.images = true;
        file.stages = vec![proofread, serde_json::from_value(serde_json::json!({"name":"summary","service":"ai","input":"transcript","output":"top","prompt":"CUSTOM"})).unwrap()];
        assert_eq!(
            resolve("video".into(), &RunOpts::default(), &file)
                .unwrap()
                .stages,
            file.stages
        );
        let opts = RunOpts {
            stages: Some(vec!["summary".into(), "proofread".into()]),
            ..Default::default()
        };
        let cfg = resolve("video".into(), &opts, &file).unwrap();
        assert_eq!(
            cfg.stages
                .iter()
                .map(|stage| stage.name.as_str())
                .collect::<Vec<_>>(),
            ["summary", "proofread"]
        );
        assert!(cfg.stages[1].images);
        let opts = RunOpts {
            no_stages: true,
            ..Default::default()
        };
        assert!(
            resolve("video".into(), &opts, &file)
                .unwrap()
                .stages
                .is_empty()
        );
        let opts = RunOpts {
            stages: Some(vec!["missing".into()]),
            ..Default::default()
        };
        assert!(resolve("video".into(), &opts, &file).is_err());
        assert!(
            resolve("video".into(), &RunOpts::default(), &Default::default())
                .unwrap()
                .stages
                .is_empty()
        );
    }

    #[test]
    fn options_pass_through_and_invalid_values_are_not_clamped() {
        let file = settings::ConfigFile::default();
        let opts = RunOpts {
            no_resume: true,
            stable_secs: Some(-1.0),
            ..Default::default()
        };
        let cfg = resolve("video".into(), &opts, &file).unwrap();
        assert!(cfg.no_resume);
        assert_eq!(cfg.stable_secs, -1.0);
        assert!(cfg.validate().is_err());
        let opts = RunOpts {
            roi: Some(String::new()),
            ..Default::default()
        };
        assert!(resolve("video".into(), &opts, &file).is_err());
    }

    #[test]
    fn subtitle_configuration_does_not_require_api_credentials() {
        let opts = RunOpts {
            transcript_source: Some(config::TranscriptSource::Subtitle),
            ..Default::default()
        };
        resolve("video".into(), &opts, &Default::default())
            .unwrap()
            .validate()
            .unwrap();
    }
}

#[cfg(test)]
mod asr_tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn transcript_language_and_detection_settings_reach_the_cache_identity() {
        let file: settings::ConfigFile = toml::from_str(
            "[transcript]\nlanguage = 'ko'\n[speech.detection]\nthreshold = 0.6\nmin_silence = 0.5\nmax_segment = 30.0\n",
        )
        .unwrap();
        let cfg = resolve("video".into(), &RunOpts::default(), &file).unwrap();
        let identity = crate::checkpoint::AsrIdentity::new(&cfg.asr);
        assert_eq!(identity.language, "ko");
        assert_eq!(identity.vad_threshold, 0.6);
        assert_eq!(identity.min_silence, 0.5);
        assert_eq!(identity.max_speech, 30.0);
    }

    #[test]
    fn speech_uses_its_service_and_retains_language_and_detection_flags() {
        let cli = crate::cli::Cli::try_parse_from([
            "notepipe",
            "video",
            "--language",
            "ko",
            "--max-segment",
            "12",
        ])
        .unwrap();
        let file: settings::ConfigFile = toml::from_str("[services.remote]\nbase_url = 'https://example.test/v1'\napi_key = 'key'\nconcurrency = 2\n[speech]\nservice = 'remote'\nmodel = 'model'\napi = 'chat'\n").unwrap();
        let cfg = resolve("video".into(), &cli.opts, &file).unwrap();
        assert_eq!(cfg.asr.concurrency, 2);
        assert!(cfg.asr.command.is_empty());
        assert_eq!(cfg.asr.base_url, "https://example.test/v1");
        assert_eq!(cfg.asr.api_key, "key");
        assert_eq!(cfg.asr.model, "model");
        assert_eq!(cfg.asr.mode, settings::AsrMode::Chat);
        assert_eq!(cfg.asr.language, "ko");
        assert_eq!(cfg.asr.max_speech, 12.0);
    }
}
