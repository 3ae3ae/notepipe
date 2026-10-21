//! Presets for the first interactive run that actually needs transcription.

use crate::settings::{Asr, AsrMode};
use anyhow::Result;
use std::io::IsTerminal as _;

struct Preset {
    label: &'static str,
    mode: AsrMode,
    base_url: &'static str,
    model: &'static str,
    command: &'static [&'static str],
    concurrency: usize,
    ask_api_key: bool,
}

const PRESETS: &[Preset] = &[
    Preset {
        label: "Groq",
        mode: AsrMode::Transcriptions,
        base_url: "https://api.groq.com/openai/v1",
        model: "whisper-large-v3-turbo",
        command: &[],
        concurrency: 4,
        ask_api_key: true,
    },
    Preset {
        label: "llama.cpp on this computer",
        mode: AsrMode::Chat,
        base_url: "",
        model: "",
        command: &["llama-server", "-hf", "ggml-org/Qwen3-ASR-1.7B-GGUF:Q8_0"],
        concurrency: 1,
        ask_api_key: false,
    },
    Preset {
        label: "Meta (requires notepipe-asr-meta to be installed)",
        mode: AsrMode::Transcriptions,
        base_url: "",
        model: "muse-voice-transcribe-1.0",
        command: &["notepipe-asr-meta"],
        concurrency: 4,
        ask_api_key: true,
    },
    Preset {
        label: "Mistral",
        mode: AsrMode::Transcriptions,
        base_url: "https://api.mistral.ai/v1",
        model: "voxtral-mini-latest",
        command: &[],
        concurrency: 4,
        ask_api_key: true,
    },
    Preset {
        label: "OpenAI",
        mode: AsrMode::Transcriptions,
        base_url: "https://api.openai.com/v1",
        model: "gpt-4o-mini-transcribe",
        command: &[],
        concurrency: 4,
        ask_api_key: true,
    },
    Preset {
        label: "Other OpenAI-compatible service",
        mode: AsrMode::Transcriptions,
        base_url: "",
        model: "",
        command: &[],
        concurrency: 4,
        ask_api_key: true,
    },
    Preset {
        label: "xAI (requires notepipe-asr-xai to be installed)",
        mode: AsrMode::Transcriptions,
        base_url: "",
        model: "grok-voice-transcribe-2.0",
        command: &["notepipe-asr-xai"],
        concurrency: 4,
        ask_api_key: true,
    },
];

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

fn apply_preset(asr: &Asr, preset: &Preset) -> Asr {
    Asr {
        mode: preset.mode,
        base_url: preset.base_url.into(),
        model: if preset.model.is_empty() {
            asr.model.clone()
        } else {
            preset.model.into()
        },
        command: preset.command.iter().map(|arg| (*arg).into()).collect(),
        concurrency: preset.concurrency,
        ..asr.clone()
    }
}

/// Called only after subtitle discovery finds no usable transcript.
pub fn run(asr: &Asr) -> Result<Asr> {
    let labels: Vec<_> = PRESETS.iter().map(|preset| preset.label).collect();
    let index = selected_or_cancelled(
        dialoguer::Select::new()
            .with_prompt("Choose a transcription service")
            .items(&labels)
            .interact_opt()?,
    )?;
    let preset = &PRESETS[index];
    let mut asr = apply_preset(asr, preset);
    if asr.command.is_empty() && asr.base_url.is_empty() {
        asr.base_url = dialoguer::Input::<String>::new()
            .with_prompt("Service address (compatible with OpenAI interface) / Service URL (OpenAI-compatible)")
            .interact_text()?
            .trim()
            .to_owned();
        crate::config::ensure_http_url(&asr.base_url)?;
    }
    if preset.ask_api_key
        && asr.api_key.trim().is_empty()
        && crate::config::asr_api_key_from_env().is_none()
    {
        asr.api_key = dialoguer::Password::new()
            .with_prompt("API key (optional; hidden)")
            .allow_empty_password(true)
            .interact()?;
    }
    if asr.command.is_empty() && asr.model.is_empty() {
        asr.model = dialoguer::Input::<String>::new()
            .with_prompt("Model name")
            .allow_empty(true)
            .interact_text()?
            .trim()
            .to_owned();
    }
    let mut file = crate::settings::load()?;
    file.asr = asr.clone();
    let path = crate::settings::save(&file)?;
    eprintln!("Configuration saved: {}", path.display());
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
    fn presets_are_alphabetical_and_supply_their_service_settings() {
        assert_eq!(PRESETS.len(), 7);
        for pair in PRESETS.windows(2) {
            assert!(pair[0].label.to_ascii_lowercase() < pair[1].label.to_ascii_lowercase());
        }
        for (index, preset) in PRESETS.iter().enumerate() {
            let asr = apply_preset(
                &Asr {
                    language: "ko".into(),
                    api_key: "saved".into(),
                    ..Default::default()
                },
                preset,
            );
            assert_eq!(asr.language, "ko");
            assert_eq!(asr.api_key, "saved");
            if index == 1 {
                assert_eq!(asr.mode, AsrMode::Chat);
                assert_eq!(
                    asr.command,
                    ["llama-server", "-hf", "ggml-org/Qwen3-ASR-1.7B-GGUF:Q8_0"]
                );
                assert_eq!(asr.concurrency, 1);
                assert!(!preset.ask_api_key);
            } else if index == 2 || index == 6 {
                assert_eq!(asr.mode, AsrMode::Transcriptions);
                assert_eq!(
                    asr.command,
                    [if index == 2 {
                        "notepipe-asr-meta"
                    } else {
                        "notepipe-asr-xai"
                    }]
                );
                assert_eq!(asr.concurrency, 4);
                assert!(asr.base_url.is_empty());
                assert!(!asr.model.is_empty());
                assert!(preset.ask_api_key);
                assert!(preset.label.contains("to be installed"));
            } else if index != 5 {
                assert!(asr.command.is_empty());
                assert!(!asr.model.is_empty());
                crate::config::ensure_http_url(&crate::config::asr_endpoint(&asr).unwrap())
                    .unwrap();
            } else {
                assert!(!asr.is_configured());
            }
        }
    }
}
