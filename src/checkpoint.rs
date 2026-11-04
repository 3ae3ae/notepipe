use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

const TRANSCRIPT_SCHEMA_VERSION: u32 = 5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrIdentity {
    pub schema_version: u32,
    pub media_sha256: String,
    pub mode: crate::settings::AsrMode,
    pub endpoint_or_command: String,
    pub model: String,
    pub prompt: String,
    pub language: String,
    pub max_speech: f32,
    pub vad_threshold: f32,
    pub min_silence: f32,
}

impl AsrIdentity {
    pub fn new(asr: &crate::settings::Asr, media_sha256: &str) -> Self {
        let endpoint_or_command = if asr.command.is_empty() {
            let base = asr.base_url.trim();
            url::Url::parse(base)
                .map(|url| url.to_string())
                .unwrap_or_else(|_| base.to_owned())
                .trim_end_matches('/')
                .to_owned()
        } else {
            asr.command.join("\0")
        };
        Self {
            schema_version: TRANSCRIPT_SCHEMA_VERSION,
            media_sha256: media_sha256.into(),
            mode: asr.mode,
            endpoint_or_command,
            model: asr.model.clone(),
            prompt: asr.prompt.clone(),
            language: asr.language.clone(),
            max_speech: asr.max_speech,
            vad_threshold: asr.vad_threshold,
            min_silence: asr.min_silence,
        }
    }
}

/// Atomic write for small files: temporary file, fsync, rename, so a crash never leaves a partial file.
/// Crash-safe because the file is synced before the rename; the parent directory is not synced,
/// so after a power loss the directory entry itself may be missing.
/// Files are readable only by the owner (0600), because configuration and credentials use this path.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create directory {}", dir.display()))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)
        .with_context(|| format!("Failed to replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_existing_file_without_touching_sibling_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(path.with_extension("tmp"), b"unrelated").unwrap();
        atomic_write(&path, b"first").unwrap();
        atomic_write(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(
            std::fs::read(path.with_extension("tmp")).unwrap(),
            b"unrelated"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode(&path),
                0o600,
                "configuration and credentials are private"
            );
        }
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    use crate::settings::{Asr, AsrMode};

    #[test]
    fn identity_tracks_semantic_settings_and_ignores_keys_and_concurrency() {
        let asr = Asr {
            base_url: " https://EXAMPLE.test/v1/ ".into(),
            model: "model".into(),
            ..Default::default()
        };
        let identity = AsrIdentity::new(&asr, "media");
        assert_eq!(identity.schema_version, 5);
        assert_ne!(identity, AsrIdentity::new(&asr, "other media"));
        assert_eq!(identity.endpoint_or_command, "https://example.test/v1");
        let changes: &[fn(&mut Asr)] = &[
            |a| a.mode = AsrMode::Chat,
            |a| a.base_url.push_str("other"),
            |a| a.model.push('2'),
            |a| a.prompt = "Transcribe verbatim.".into(),
            |a| a.language = "ko".into(),
            |a| a.max_speech = 30.0,
            |a| a.vad_threshold = 0.6,
            |a| a.min_silence = 0.5,
            |a| a.command = vec!["adapter".into()],
        ];
        for change in changes {
            let mut changed = asr.clone();
            change(&mut changed);
            assert_ne!(identity, AsrIdentity::new(&changed, "media"));
        }
        let mut changed = asr.clone();
        changed.api_key = "secret".into();
        changed.concurrency = 1;
        assert_eq!(identity, AsrIdentity::new(&changed, "media"));
        let mut managed = asr.clone();
        managed.command = vec!["adapter".into(), "--model".into(), "some model".into()];
        let managed_id = AsrIdentity::new(&managed, "media");
        managed.base_url = "ignored".into();
        assert_eq!(managed_id, AsrIdentity::new(&managed, "media"));
        managed.command = vec!["adapter --model".into(), "some model".into()];
        assert_ne!(managed_id, AsrIdentity::new(&managed, "media"));
    }
}
