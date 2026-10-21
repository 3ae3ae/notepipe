//! Generic connection settings for the first interactive transcription run.

use crate::{
    config::PipelineConfig,
    settings::{Asr, AsrMode},
};
use anyhow::{Context, Result};
use std::{
    io::IsTerminal as _,
    path::{Path, PathBuf},
};

pub fn is_first_run(cfg: &PipelineConfig) -> bool {
    cfg.config_path.as_deref() != Some(Path::new("-"))
        && !cfg.asr.is_configured()
        && !crate::progress::is_json()
        && !crate::progress::is_quiet()
        && std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
}

fn selected_or_cancelled(choice: Option<usize>) -> Result<usize> {
    choice.ok_or_else(|| {
        anyhow::anyhow!(
            "Setup cancelled; no configuration saved. Run your video command again to restart."
        )
    })
}

fn set_connection(asr: &mut Asr, adapter: bool, input: &str, mode: AsrMode) -> Result<()> {
    if adapter {
        let command: Vec<String> = serde_json::from_str(input)
            .context("Enter the adapter command as a JSON array of arguments")?;
        anyhow::ensure!(
            command
                .first()
                .is_some_and(|program| !program.trim().is_empty()),
            "The adapter command must name an executable"
        );
        asr.command = command;
        asr.base_url.clear();
    } else {
        crate::config::ensure_http_url(input)?;
        asr.base_url = input.trim().into();
        asr.command.clear();
    }
    asr.mode = mode;
    Ok(())
}

fn save_asr(cfg: &PipelineConfig) -> Result<PathBuf> {
    cfg.validate_asr()?;
    let mut file = crate::settings::load_from(cfg.config_path.as_deref())?;
    file.asr = cfg.asr.clone();
    crate::settings::save(&file, cfg.config_path.as_deref())
}

/// Called only after subtitle discovery finds no usable transcript.
pub fn run(cfg: &PipelineConfig) -> Result<Asr> {
    anyhow::ensure!(
        cfg.config_path.as_deref() != Some(Path::new("-")),
        "Setup is unavailable with --config -; configure [asr] in the supplied configuration."
    );
    let choice = selected_or_cancelled(
        dialoguer::Select::new()
            .with_prompt("Speech recognition connection")
            .items(["Adapter command", "HTTP endpoint"])
            .interact_opt()?,
    )?;
    let adapter = choice == 0;
    let modes = [AsrMode::Transcriptions, AsrMode::Chat];
    let mode = modes[selected_or_cancelled(
        dialoguer::Select::new()
            .with_prompt("Speech recognition protocol")
            .items(modes)
            .interact_opt()?,
    )?];
    let input = dialoguer::Input::<String>::new()
        .with_prompt(if adapter {
            "Adapter command (JSON array, e.g. [\"adapter\", \"--arg\"])"
        } else {
            "Speech endpoint base URL (OpenAI-compatible)"
        })
        .interact_text()?;
    let mut cfg = cfg.clone();
    let asr = &mut cfg.asr;
    set_connection(asr, adapter, &input, mode)?;
    asr.model = dialoguer::Input::<String>::new()
        .with_prompt("Model (optional)")
        .with_initial_text(&asr.model)
        .allow_empty(true)
        .interact_text()?
        .trim()
        .to_owned();
    if asr.api_key.trim().is_empty() && crate::config::asr_api_key_from_env().is_none() {
        asr.api_key = dialoguer::Password::new()
            .with_prompt("API key (optional; hidden)")
            .allow_empty_password(true)
            .interact()?;
    }
    asr.language = dialoguer::Input::<String>::new()
        .with_prompt("Spoken language (optional; blank means automatic)")
        .with_initial_text(&asr.language)
        .allow_empty(true)
        .interact_text()?
        .trim()
        .to_owned();
    let path = save_asr(&cfg)?;
    eprintln!(
        "Saved [asr] to {}. See README.md for service examples.",
        path.display()
    );
    Ok(cfg.asr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_cancels_setup() {
        assert!(selected_or_cancelled(None).is_err());
        assert_eq!(selected_or_cancelled(Some(0)).unwrap(), 0);
    }

    #[test]
    fn connection_settings_preserve_arguments_and_select_the_protocol() {
        let mut asr = Asr::default();
        set_connection(
            &mut asr,
            true,
            r#"["/path with spaces/adapter", "--model", "model with spaces"]"#,
            AsrMode::Chat,
        )
        .unwrap();
        assert_eq!(
            asr.command,
            ["/path with spaces/adapter", "--model", "model with spaces"]
        );
        assert!(asr.base_url.is_empty());
        assert_eq!(asr.mode, AsrMode::Chat);
        set_connection(
            &mut asr,
            false,
            " https://example.test/v1 ",
            AsrMode::Transcriptions,
        )
        .unwrap();
        assert!(asr.command.is_empty());
        assert_eq!(asr.base_url, "https://example.test/v1");
        assert_eq!(asr.mode, AsrMode::Transcriptions);
        for input in ["[]", r#"[""]"#, r#"[" "]"#, "[1]", "adapter --model name"] {
            assert!(
                set_connection(&mut asr, true, input, AsrMode::Chat).is_err(),
                "{input}"
            );
        }
        assert!(set_connection(&mut asr, false, "file:///not-a-service", AsrMode::Chat).is_err());
    }

    #[test]
    fn setup_saves_to_the_selected_file_and_preserves_its_format_and_other_settings() {
        let root = tempfile::tempdir().unwrap();
        for json in [false, true] {
            let path = root.path().join(if json {
                "selected.json"
            } else {
                "selected.toml"
            });
            let mut file = crate::settings::ConfigFile::default();
            file.defaults.out = Some("my-notes".into());
            file.llm.model = "my-model".into();
            let original = if json {
                serde_json::to_string(&file).unwrap()
            } else {
                toml::to_string(&file).unwrap()
            };
            std::fs::write(&path, &original).unwrap();
            let opts = crate::cli::RunOpts {
                config: Some(path.clone()),
                ..Default::default()
            };
            let mut cfg = crate::options::resolve("video".into(), &opts, &file).unwrap();
            set_connection(
                &mut cfg.asr,
                false,
                "https://example.test/v1/chat/completions",
                AsrMode::Chat,
            )
            .unwrap();
            assert_eq!(save_asr(&cfg).unwrap(), path);
            let saved = crate::settings::load_from(Some(&path)).unwrap();
            file.asr = cfg.asr;
            assert_eq!(saved, file);
            assert_eq!(
                std::fs::read_to_string(&path).unwrap().starts_with('{'),
                json
            );
            let backup = root.path().join(if json {
                "selected.json.bak"
            } else {
                "selected.toml.bak"
            });
            assert_eq!(std::fs::read_to_string(backup).unwrap(), original);
        }
    }

    #[test]
    fn setup_rejects_a_mismatched_endpoint_before_saving() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("selected.toml");
        let original = "[defaults]\nout = 'my-notes'\n";
        std::fs::write(&path, original).unwrap();
        let opts = crate::cli::RunOpts {
            config: Some(path.clone()),
            ..Default::default()
        };
        let mut cfg = crate::options::resolve("video".into(), &opts, &Default::default()).unwrap();
        for (mode, endpoint) in [
            (
                AsrMode::Chat,
                "https://example.test/v1/audio/transcriptions",
            ),
            (
                AsrMode::Transcriptions,
                "https://example.test/v1/chat/completions",
            ),
        ] {
            set_connection(&mut cfg.asr, false, endpoint, mode).unwrap();
            assert!(
                save_asr(&cfg)
                    .unwrap_err()
                    .to_string()
                    .contains("endpoint and protocol do not match")
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            assert!(!path.with_extension("toml.bak").exists());
        }
    }

    #[test]
    fn stdin_configuration_disables_setup_before_prompting_or_saving() {
        let opts = crate::cli::RunOpts {
            config: Some("-".into()),
            ..Default::default()
        };
        let cfg = crate::options::resolve("video".into(), &opts, &Default::default()).unwrap();
        assert!(!is_first_run(&cfg));
        assert!(
            run(&cfg)
                .unwrap_err()
                .to_string()
                .contains("Setup is unavailable with --config -")
        );
    }
}
