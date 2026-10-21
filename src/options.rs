//! Merge file settings with CLI overrides.
use crate::cli::RunOpts;
use crate::{config, llm, settings};

/// The effective AI settings: the file with command-line overrides.
fn resolve_llm(opts: &RunOpts, file: &settings::ConfigFile) -> anyhow::Result<llm::LlmSettings> {
    let mut settings = file.llm.clone();
    if let Some(value) = &opts.llm_base_url {
        settings.base_url = value.clone();
    }
    if let Some(value) = &opts.llm_api_key {
        settings.api_key = value.clone();
    }
    if let Some(value) = &opts.llm_model {
        settings.model = value.clone();
    }
    if opts.no_stages {
        settings.stages.clear();
    } else if let Some(names) = &opts.stages {
        settings.stages = names
            .iter()
            .map(|name| {
                file.llm
                    .stages
                    .iter()
                    .find(|stage| stage.name == *name)
                    .cloned()
                    .or_else(|| llm::Stage::builtin(name))
                    .ok_or_else(|| anyhow::anyhow!("Unknown LLM stage: {name}"))
            })
            .collect::<anyhow::Result<_>>()?;
    }
    if let Some(prompt) = &opts.llm_prompt {
        for stage in settings
            .stages
            .iter_mut()
            .filter(|stage| stage.name == "proofread")
        {
            stage.prompt = prompt.clone();
        }
    }
    Ok(settings)
}

/// Precedence: explicit options, then the file, then built-in defaults.
pub fn resolve(
    source: String,
    opts: &RunOpts,
    file: &settings::ConfigFile,
) -> anyhow::Result<config::PipelineConfig> {
    let d = &file.defaults;
    use config::ScreenshotMode;
    let out_root = opts
        .out
        .clone()
        .or_else(|| d.out.clone())
        .map(config::expand_tilde)
        .unwrap_or_else(|| config::DEFAULT_OUT_DIR.into());
    Ok(config::PipelineConfig {
        url: source,
        config_path: opts.config.clone(),
        on_complete: d.on_complete.clone(),
        source: file.source.clone(),
        // The pipeline assigns the per-source work directory before processing.
        out_dir: out_root.clone(),
        out_root,
        similarity: opts
            .similarity
            .or(d.similarity)
            .unwrap_or(config::DEFAULT_SIMILARITY),
        sample_interval: opts
            .sample_interval
            .or(d.sample_interval)
            .unwrap_or(config::DEFAULT_SAMPLE_INTERVAL),
        cooldown: opts
            .cooldown
            .or(d.cooldown)
            .unwrap_or(config::DEFAULT_COOLDOWN),
        screenshot_mode: opts
            .screenshot_mode
            .or(d.screenshot_mode)
            .unwrap_or(ScreenshotMode::Stable),
        stable_secs: opts
            .stable_secs
            .or(d.stable_secs)
            .unwrap_or(config::DEFAULT_STABLE_SECS),
        roi: match &opts.roi {
            Some(s) => Some(config::Roi::parse(s)?),
            None => match &d.roi {
                Some(s) => Some(config::Roi::parse(s)?),
                None => None,
            },
        },
        no_resume: opts.no_resume,
        subtitle: opts.subtitle.clone(),
        llm: resolve_llm(opts, file)?,
        asr: resolve_asr(opts, file),
        transcript_source: opts
            .transcript_source
            .or(d.transcript_source)
            .unwrap_or_default(),
    })
}

/// CLI overrides for [asr]; environment credentials are resolved only by the CLI pipeline.
fn resolve_asr(opts: &RunOpts, file: &settings::ConfigFile) -> crate::settings::Asr {
    let mut a = file.asr.clone();
    if let Some(v) = &opts.asr_base_url {
        a.base_url = v.clone();
    }
    if let Some(v) = &opts.asr_api_key {
        a.api_key = v.clone();
    }
    if let Some(v) = &opts.asr_model {
        a.model = v.clone();
    }
    if let Some(v) = opts.asr_mode {
        a.mode = v;
    }
    if let Some(v) = &opts.asr_language {
        a.language = v.clone();
    }
    if let Some(v) = opts.max_speech {
        a.max_speech = v;
    }
    a.concurrency = a.concurrency.clamp(1, 16);
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_flags_override_config_in_order_and_map_the_proofread_prompt() {
        let mut file = settings::ConfigFile::default();
        let mut proofread = llm::Stage::builtin("proofread").unwrap();
        proofread.images = true;
        file.llm.stages = vec![proofread];
        assert_eq!(
            resolve("video".into(), &RunOpts::default(), &file)
                .unwrap()
                .llm
                .stages,
            file.llm.stages
        );
        let opts = RunOpts {
            stages: Some(vec!["summary".into(), "proofread".into()]),
            llm_prompt: Some("Override".into()),
            ..Default::default()
        };
        let cfg = resolve("video".into(), &opts, &file).unwrap();
        assert_eq!(
            cfg.llm
                .stages
                .iter()
                .map(|stage| stage.name.as_str())
                .collect::<Vec<_>>(),
            ["summary", "proofread"]
        );
        assert!(cfg.llm.stages[1].images);
        assert_eq!(cfg.llm.stages[1].prompt, "Override");
        let opts = RunOpts {
            no_stages: true,
            ..Default::default()
        };
        assert!(
            resolve("video".into(), &opts, &file)
                .unwrap()
                .llm
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
                .llm
                .stages
                .is_empty()
        );
    }

    #[test]
    fn explicit_false_overrides_config_and_invalid_values_are_not_clamped() {
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
    fn asr_flags_override_settings_and_concurrency_is_clamped() {
        let cli = crate::cli::Cli::try_parse_from([
            "notepipe",
            "video",
            "--asr-base-url",
            "http://localhost:1234/v1",
            "--asr-api-key",
            "key",
            "--asr-model",
            "model",
            "--asr-mode",
            "chat",
            "--asr-language",
            "ko",
            "--max-speech",
            "12",
        ])
        .unwrap();
        let mut file = settings::ConfigFile::default();
        file.asr.command = vec!["adapter".into()];
        for (given, expected) in [(0, 1), (4, 4), (100, 16)] {
            file.asr.concurrency = given;
            let cfg = resolve("video".into(), &cli.opts, &file).unwrap();
            assert_eq!(cfg.asr.concurrency, expected);
            assert_eq!(cfg.asr.command, ["adapter"]);
            assert_eq!(cfg.asr.base_url, "http://localhost:1234/v1");
            assert_eq!(cfg.asr.api_key, "key");
            assert_eq!(cfg.asr.model, "model");
            assert_eq!(cfg.asr.mode, settings::AsrMode::Chat);
            assert_eq!(cfg.asr.language, "ko");
            assert_eq!(cfg.asr.max_speech, 12.0);
        }
        for flag in [
            "--provider",
            "--threads",
            "--gpu-layers",
            "--mmproj-offload",
            "--no-mmproj-offload",
            "--model-dir",
            "--asr-api-base-url",
            "--asr-api-model",
            "--asr-api-mode",
        ] {
            assert!(
                crate::cli::Cli::try_parse_from(["notepipe", "video", flag, "value"]).is_err(),
                "{flag}"
            );
        }
        assert!(crate::cli::Cli::try_parse_from(["notepipe", "models", "list"]).is_err());
    }
}
