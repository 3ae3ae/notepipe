pub mod adapter;
pub mod artifact;
pub mod asr;
pub mod auth;
pub mod checkpoint;
pub mod cli;
pub mod config;
pub mod dispatch;
pub mod doctor;
pub mod error;
pub mod execution;
pub mod fetch;
pub mod language;
pub mod llm;
pub mod math;
pub mod media;
pub mod options;
pub mod pipeline;
pub mod portable;
pub mod progress;
pub mod render;
pub mod runtime;
pub mod scene;
pub mod settings;
pub mod subtitle;
pub mod summarize;
pub mod timeline;
pub mod wizard;
pub mod ytdlp;

/// OS-generated URL-safe random bytes for multipart boundaries.
pub(crate) fn random_b64url(bytes: usize) -> anyhow::Result<String> {
    use base64::Engine as _;
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf)
        .map_err(|_| anyhow::anyhow!("Cannot generate secure random bytes"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}
