mod adapter;
mod app;
mod artifact;
mod asr;
mod checkpoint;
mod cli;
mod config;
mod dispatch;
mod doctor;
mod error;
mod llm;
mod media;
mod options;
mod pipeline;
mod progress;
mod render;
mod runtime;
mod scene;
mod settings;
mod source;
mod subtitle;
mod summarize;
mod timeline;
mod vad;

/// OS-generated URL-safe random bytes for multipart boundaries.
fn random_b64url(bytes: usize) -> anyhow::Result<String> {
    use base64::Engine as _;
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf)
        .map_err(|_| anyhow::anyhow!("Cannot generate secure random bytes"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

/// Run the CLI. Integration uses stdin configuration, NDJSON events and the note folder.
pub fn main() -> std::process::ExitCode {
    app::main()
}

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;
