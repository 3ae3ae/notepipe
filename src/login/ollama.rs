//! Ollama 本地服务登录：探测服务、选择模型、写入 LLM 配置。
//! Ollama 的 OpenAI 兼容端点无需凭据（空 api_key 不发 Authorization 头，
//! 见 llm.rs request_chat_once），"登录"实为服务发现 + 配置。

use anyhow::{Context, Result, bail, ensure};
use std::io::IsTerminal;

use crate::llm::LlmProvider;

const DEFAULT_HOST: &str = "http://localhost:11434";

/// Ollama 服务地址：环境变量 OLLAMA_HOST > 默认 localhost:11434。
fn host() -> String {
    normalize_host(std::env::var("OLLAMA_HOST").ok().as_deref())
}

fn normalize_host(raw: Option<&str>) -> String {
    let raw = raw
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(DEFAULT_HOST)
        .trim_end_matches('/');
    if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    }
}

fn agent() -> ureq::Agent {
    crate::runtime::http_agent(std::time::Duration::from_secs(10))
}

/// 已下载的模型名列表（GET {host}/api/tags）。
pub(super) fn models(host: &str) -> Result<Vec<String>> {
    let response = agent()
        .get(&format!("{host}/api/tags"))
        .call()
        .map_err(|_| {
            anyhow::anyhow!(
                "Cannot reach the Ollama service at {host}; make sure it is installed and running (ollama serve)"
            )
        })?;
    let value: serde_json::Value = response
        .into_json()
        .context(crate::tr!("Cannot parse the Ollama response"))?;
    let Some(list) = value["models"].as_array() else {
        bail!(crate::tr!("Ollama response is missing the model list"));
    };
    Ok(list
        .iter()
        .filter_map(|m| m["name"].as_str().map(str::to_string))
        .collect())
}

/// `notepipe --login ollama`：探测本机服务、选模型并启用 AI 校对。
pub(super) fn login() -> Result<()> {
    let host = host();
    let names = models(&host)?;
    ensure!(
        !names.is_empty(),
        crate::tr!("No downloaded models found; run ollama pull <model> first")
    );
    let interactive = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let model = if names.len() == 1 {
        names.into_iter().next().unwrap_or_default()
    } else if interactive {
        let pick = dialoguer::Select::new()
            .with_prompt(crate::tr!("Choose a model for proofreading"))
            .items(&names)
            .default(0)
            .interact_opt()?
            .ok_or_else(|| {
                anyhow::anyhow!(crate::tr!("Login cancelled; no configuration saved."))
            })?;
        names.into_iter().nth(pick).unwrap_or_default()
    } else {
        bail!(crate::tr!(
            "Multiple Ollama models available; run notepipe --login ollama in a terminal to choose"
        ));
    };

    let mut cfg = crate::settings::load()?;
    cfg.llm.provider = LlmProvider::Ollama;
    cfg.llm.base_url = format!("{host}/v1");
    cfg.llm.api_key.clear();
    cfg.llm.model = model.clone();
    if cfg.llm.vision {
        // 本地模型多数不支持图片输入；登录时回落纯文本，需要时可在 llm setup 重开
        cfg.llm.vision = false;
        crate::println_tr!(
            "Screenshot-assisted proofing has been turned off (local models usually do not support image input). / Image-assisted proofreading disabled (local models usually do not accept images)."
        );
    }
    cfg.llm.enabled = true;
    crate::llm::validate(&cfg.llm)?;
    let path = crate::settings::save(&cfg)?;
    let saved = path.display().to_string();
    crate::println_tr!(
        "Logged in to Ollama ({}), model {}, settings saved and enabled: {}",
        host,
        model,
        saved
    );
    match crate::llm::test_connection(&cfg.llm) {
        Ok(()) => crate::println_tr!("Connection test passed."),
        Err(e) => bail!(
            "Settings saved, but the connection test failed; make sure {model} can chat: {e:#}"
        ),
    }
    Ok(())
}

/// `notepipe --logout ollama`：当前 AI 校对使用 Ollama 时重置配置。
pub(super) fn logout() -> Result<()> {
    let mut cfg = crate::settings::load()?;
    if cfg.llm.provider != LlmProvider::Ollama {
        crate::println_tr!(
            "The current LLM configuration is not Ollama and has not been changed. / The current LLM configuration is not Ollama; nothing changed."
        );
        return Ok(());
    }
    cfg.llm = crate::llm::LlmSettings::default();
    crate::settings::save(&cfg)?;
    crate::println_tr!(
        "Cleared Ollama login configuration and turned off AI proofing. / Ollama login removed and AI proofreading disabled."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn host_accepts_bare_addresses_and_trailing_slashes() {
        use super::normalize_host;
        assert_eq!(normalize_host(None), "http://localhost:11434");
        assert_eq!(normalize_host(Some("  ")), "http://localhost:11434");
        assert_eq!(
            normalize_host(Some("127.0.0.1:1234/")),
            "http://127.0.0.1:1234"
        );
        assert_eq!(
            normalize_host(Some("https://ollama.lan")),
            "https://ollama.lan"
        );
    }
}
