//! Generic connection settings for the first interactive transcription run.

use crate::settings::Asr;
use anyhow::{Context, Result};
use std::io::IsTerminal as _;

pub fn is_first_run(asr: &Asr) -> bool {
    !asr.is_configured()
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

fn set_connection(asr: &mut Asr, adapter: bool, input: &str) -> Result<()> {
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
    Ok(())
}

/// Called only after subtitle discovery finds no usable transcript.
pub fn run(asr: &Asr) -> Result<Asr> {
    let choice = selected_or_cancelled(
        dialoguer::Select::new()
            .with_prompt("Speech recognition connection")
            .items(["Adapter command", "HTTP endpoint"])
            .interact_opt()?,
    )?;
    let adapter = choice == 0;
    let input = dialoguer::Input::<String>::new()
        .with_prompt(if adapter {
            "Adapter command (JSON array, e.g. [\"adapter\", \"--arg\"])"
        } else {
            "Speech endpoint base URL (OpenAI-compatible)"
        })
        .interact_text()?;
    let mut asr = asr.clone();
    set_connection(&mut asr, adapter, &input)?;
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
    let mut file = crate::settings::load()?;
    file.asr = asr.clone();
    let path = crate::settings::save(&file)?;
    eprintln!(
        "Saved [asr] to {}. See README.md for service examples.",
        path.display()
    );
    Ok(asr)
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
    fn connection_settings_preserve_arguments_and_the_configured_protocol() {
        let mut asr = Asr {
            mode: crate::settings::AsrMode::Chat,
            ..Default::default()
        };
        set_connection(
            &mut asr,
            true,
            r#"["/path with spaces/adapter", "--model", "model with spaces"]"#,
        )
        .unwrap();
        assert_eq!(
            asr.command,
            ["/path with spaces/adapter", "--model", "model with spaces"]
        );
        assert!(asr.base_url.is_empty());
        assert_eq!(asr.mode, crate::settings::AsrMode::Chat);
        set_connection(&mut asr, false, " https://example.test/v1 ").unwrap();
        assert!(asr.command.is_empty());
        assert_eq!(asr.base_url, "https://example.test/v1");
        assert_eq!(asr.mode, crate::settings::AsrMode::Chat);
        for input in ["[]", r#"[""]"#, r#"[" "]"#, "[1]", "adapter --model name"] {
            assert!(set_connection(&mut asr, true, input).is_err(), "{input}");
        }
        assert!(set_connection(&mut asr, false, "file:///not-a-service").is_err());
    }
}
