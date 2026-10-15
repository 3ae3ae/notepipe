//! `--login/--logout <平台>` 的 CLI 实现。凭据统一存 `{config_dir}/auth/`（0600 原子写）。

pub mod codex;
pub mod ollama;

use anyhow::Result;

use crate::cli::LoginPlatform;

pub fn login(platform: LoginPlatform) -> Result<()> {
    match platform {
        LoginPlatform::Bilibili => crate::auth::login_bilibili(),
        LoginPlatform::Ollama => ollama::login(),
        LoginPlatform::Codex => codex::login(),
    }
}

pub fn logout(platform: LoginPlatform) -> Result<()> {
    match platform {
        LoginPlatform::Bilibili => crate::auth::logout_bilibili(),
        LoginPlatform::Ollama => ollama::logout(),
        LoginPlatform::Codex => codex::logout(),
    }
}
