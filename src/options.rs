//! CLI 与任务协议共享的配置合并规则。
use crate::cli::RunOpts;
use crate::{config, llm, settings};

/// 内置默认输出格式。settings 的展示与配置模板也由它渲染，避免第二份真相。
pub fn default_formats() -> Vec<config::OutputFormat> {
    vec![config::OutputFormat::Md, config::OutputFormat::Html]
}

/// 配置文件 + CLI 覆盖 -> 生效 LLM 设置。
fn resolve_llm(opts: &RunOpts, file: &settings::ConfigFile) -> llm::LlmSettings {
    let mut s = file.llm.clone();
    s.enabled = config::resolve_flag(opts.llm, opts.no_llm, None, s.enabled);
    s.vision = config::resolve_flag(opts.llm_vision, opts.no_llm_vision, None, s.vision);
    if let Some(v) = &opts.llm_base_url {
        s.base_url = v.clone();
    }
    if let Some(v) = &opts.llm_api_key {
        s.api_key = v.clone();
    }
    if let Some(v) = &opts.llm_model {
        s.model = v.clone();
    }
    if let Some(v) = &opts.llm_prompt {
        s.prompt = Some(v.clone());
    }
    if opts.no_llm_hint {
        s.disable_hint = true;
    }
    s.summarize = config::resolve_flag(opts.summarize, opts.no_summarize, None, s.summarize);
    if opts.no_translate {
        s.translate = None;
    } else if let Some(target) = &opts.translate {
        s.translate = Some(target.clone());
    }
    s
}

/// 优先级：CLI 显式参数 > 配置文件 [defaults] > 内置默认。
pub fn resolve(
    source: String,
    opts: &RunOpts,
    file: &settings::ConfigFile,
) -> anyhow::Result<config::PipelineConfig> {
    let d = &file.defaults;
    use config::SlideMode;
    let out_root = opts
        .out
        .clone()
        .or_else(|| d.out.clone())
        .map(config::expand_tilde)
        .unwrap_or_else(|| config::DEFAULT_OUT_DIR.into());
    Ok(config::PipelineConfig {
        url: source,
        cookies: opts
            .cookies
            .clone()
            .or_else(|| d.cookies.clone())
            .map(config::expand_tilde),
        cookies_from_browser: opts
            .cookies_from_browser
            .clone()
            .or_else(|| d.cookies_from_browser.clone()),
        // out_dir 此处只是初值；pipeline 里会改写为 {out_root}/{platform}/{title}/{id}/
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
        max_height: opts
            .max_height
            .or(d.max_height)
            .unwrap_or(config::DEFAULT_MAX_HEIGHT),
        slide_mode: opts
            .slide_mode
            .or(d.slide_mode)
            .unwrap_or(SlideMode::Stable),
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
        formats: opts
            .formats
            .clone()
            .or_else(|| d.formats.clone())
            .unwrap_or_else(default_formats),
        keep_video: config::resolve_flag(opts.keep_video, opts.no_keep_video, d.keep_video, false),
        no_download: opts.no_download || d.no_download.unwrap_or(false),
        resume: config::resolve_flag(opts.resume, opts.no_resume, d.resume, true),
        llm: resolve_llm(opts, file),
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
    #[test]
    fn cookie_inputs_merge_independently_with_cli_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let saved = dir.path().join("saved.txt");
        let selected = dir.path().join("selected.txt");
        std::fs::write(&saved, "saved").unwrap();
        std::fs::write(&selected, "selected").unwrap();
        let mut file = crate::settings::ConfigFile::default();
        file.defaults.cookies = Some(saved.clone());
        file.defaults.cookies_from_browser = Some("firefox:default-release".into());
        let opts = crate::cli::RunOpts {
            cookies: Some(selected.clone()),
            ..Default::default()
        };
        let cfg = super::resolve("video".into(), &opts, &file).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.cookies, Some(selected));
        assert_eq!(
            cfg.cookies_from_browser.as_deref(),
            Some("firefox:default-release")
        );
        let opts = crate::cli::RunOpts {
            cookies_from_browser: Some("chrome".into()),
            ..Default::default()
        };
        let cfg = super::resolve("video".into(), &opts, &file).unwrap();
        assert_eq!(cfg.cookies, Some(saved));
        assert_eq!(cfg.cookies_from_browser.as_deref(), Some("chrome"));
    }

    use super::*;

    #[test]
    fn explicit_false_overrides_config_and_invalid_values_are_not_clamped() {
        let mut file = settings::ConfigFile::default();
        file.defaults.keep_video = Some(true);
        file.defaults.resume = Some(true);
        let opts = RunOpts {
            no_keep_video: true,
            no_resume: true,
            stable_secs: Some(-1.0),
            ..Default::default()
        };
        let cfg = resolve("video".into(), &opts, &file).unwrap();
        assert!(!cfg.keep_video && !cfg.resume);
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

    #[test]
    fn cli_summarize_flag_overrides_config() {
        let mut file = settings::ConfigFile::default();
        file.llm.summarize = false;
        let opts = RunOpts {
            summarize: true,
            ..Default::default()
        };
        let cfg = resolve("video".into(), &opts, &file).unwrap();
        assert!(cfg.llm.summarize);

        let mut file = settings::ConfigFile::default();
        file.llm.summarize = true;
        let opts = RunOpts {
            no_summarize: true,
            ..Default::default()
        };
        let cfg = resolve("video".into(), &opts, &file).unwrap();
        assert!(!cfg.llm.summarize);
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
            "xai",
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
            assert_eq!(cfg.asr.mode, settings::AsrMode::Xai);
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
