//! Fixed task inputs. The transport may contain credentials; persisted identities never do.

use crate::{config::PipelineConfig, settings::ConfigFile, timeline::TranscriptEvent};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};

pub const REQUEST_SCHEMA: u32 = 1;
pub const MAX_REQUEST_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Operation {
    #[default]
    Generate,
    Reprocess {
        base_version_dir: PathBuf,
        components: Vec<Component>,
        prior_work_dir: Option<PathBuf>,
    },
}

/// A version component that a reprocess task can redo. The wire format stays the
/// lowercase name so stored task plans keep loading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Component {
    Screenshots,
    Proofreading,
    Summary,
    Exports,
}

impl Component {
    pub const ALL: [Self; 4] = [
        Self::Screenshots,
        Self::Proofreading,
        Self::Summary,
        Self::Exports,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Screenshots => "screenshots",
            Self::Proofreading => "proofreading",
            Self::Summary => "summary",
            Self::Exports => "exports",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == name)
    }
}

/// Sent once over stdin. Do not derive Debug: config contains decrypted credentials.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema: u32,
    #[serde(default)]
    pub operation: Operation,
    pub task_id: String,
    pub note_id: String,
    pub version_id: String,
    pub source: String,
    pub source_id: String,
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub subtitle: Option<PathBuf>,
    #[serde(default)]
    pub subtitle_events: Option<Vec<TranscriptEvent>>,
    pub config: ConfigFile,
    pub work_dir: PathBuf,
    pub note_dir: PathBuf,
    /// Reserved for the cooperative pause/cancel protocol; never sent as argv.
    #[serde(default)]
    pub control_path: Option<PathBuf>,
    #[serde(default)]
    pub service_versions: BTreeMap<String, String>,
}

impl Request {
    pub fn read(reader: impl Read) -> Result<Self> {
        let mut data = Vec::new();
        reader.take(MAX_REQUEST_BYTES + 1).read_to_end(&mut data)?;
        anyhow::ensure!(
            data.len() as u64 <= MAX_REQUEST_BYTES,
            crate::tr!(
                "Task input exceeds {} MiB",
                MAX_REQUEST_BYTES / (1024 * 1024)
            )
        );
        // serde's error can include unknown field names. Do not echo attacker-supplied
        // values or accidentally repeat a secret from a malformed JSON document.
        let request: Self = serde_json::from_slice(&data).map_err(|e| {
            anyhow::anyhow!(
                "{}",
                crate::tr!(
                    "Unable to read task parameters (line {}, column {}) / Invalid task JSON",
                    e.line(),
                    e.column()
                )
            )
        })?;
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.schema == REQUEST_SCHEMA,
            crate::tr!("Unsupported task schema")
        );
        for (name, value) in [
            ("task_id", &self.task_id),
            ("note_id", &self.note_id),
            ("version_id", &self.version_id),
        ] {
            anyhow::ensure!(valid_id(value), "Invalid task identifier: {name}");
        }
        anyhow::ensure!(
            !self.source.trim().is_empty() && !self.source_id.trim().is_empty(),
            crate::tr!("Task source is missing")
        );
        anyhow::ensure!(
            !self.title.trim().is_empty(),
            crate::tr!("Confirmed title is missing")
        );
        anyhow::ensure!(
            self.duration.is_finite() && self.duration >= 0.0,
            crate::tr!("Invalid duration")
        );
        anyhow::ensure!(
            self.work_dir.is_absolute() && self.note_dir.is_absolute(),
            crate::tr!("Task paths must be absolute")
        );
        anyhow::ensure!(
            self.work_dir != self.note_dir
                && !self.work_dir.starts_with(self.note_dir.join("versions")),
            crate::tr!("Work directory overlaps published versions")
        );
        anyhow::ensure!(
            self.subtitle.is_none() || self.subtitle_events.is_none(),
            crate::tr!("Specify subtitle file or parsed events, not both")
        );
        if let Some(path) = &self.subtitle {
            anyhow::ensure!(
                path.is_absolute(),
                crate::tr!("Subtitle path must be absolute")
            );
        }
        if let Some(events) = &self.subtitle_events {
            validate_events(events)?;
        }
        if let Operation::Reprocess {
            base_version_dir,
            components,
            prior_work_dir,
        } = &self.operation
        {
            anyhow::ensure!(
                base_version_dir.is_absolute(),
                crate::tr!("Base version requires an absolute path")
            );
            anyhow::ensure!(
                !components.is_empty(),
                crate::tr!("Invalid requested components")
            );
            anyhow::ensure!(
                prior_work_dir
                    .as_ref()
                    .is_none_or(|p| p.is_absolute() && p != &self.work_dir),
                crate::tr!("Invalid prior work directory")
            );
        }
        if let Some(path) = &self.control_path {
            anyhow::ensure!(
                path.is_absolute() && path.starts_with(&self.work_dir),
                crate::tr!("Invalid control file location")
            );
        }
        Ok(())
    }

    pub fn resolve(&self) -> Result<PipelineConfig> {
        self.validate()?;
        let mut cfg =
            crate::options::resolve(self.source.clone(), &Default::default(), &self.config)?;
        cfg.out_dir = self.work_dir.clone();
        cfg.out_root = self.note_dir.clone();
        // Recovery is a task invariant, not a configurable preference.
        cfg.resume = true;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Identity includes effective parameters and selected content, excluding secret values.
    pub fn binding(&self, cfg: &PipelineConfig) -> Result<serde_json::Value> {
        let mut config = cfg.redacted_for_identity();
        // Storage moves are resolved locations, not a change in the processing plan.
        config.out_dir = PathBuf::new();
        config.out_root = PathBuf::new();
        // A missing original file must not change the identity of an AI/export-only
        // continuation. Local content identity is independent of current file presence.
        if Path::new(&self.source).is_absolute() || self.source_id.starts_with("local:") {
            config.url = self.source_id.clone();
        }
        let generating = matches!(self.operation, Operation::Generate);
        let subtitle_digest = if generating {
            self.subtitle
                .as_ref()
                .map(|path| file_digest(path))
                .transpose()?
        } else {
            None
        };
        let needs_source = generating
            || matches!(&self.operation, Operation::Reprocess { components, .. }
                if components.contains(&Component::Screenshots));
        let local_digest = if needs_source && Path::new(&self.source).is_file() {
            Some(file_digest(Path::new(&self.source))?)
        } else {
            None
        };
        if let (Some(expected), Some(actual)) = (
            self.source_id.strip_prefix("local:sha256:"),
            local_digest.as_deref(),
        ) {
            anyhow::ensure!(
                expected == actual,
                crate::tr!("Source content changed after confirmation; reread the source")
            );
        }
        let operation = match &self.operation {
            Operation::Generate => serde_json::json!({"kind":"generate"}),
            Operation::Reprocess {
                base_version_dir,
                components,
                ..
            } => {
                let manifest =
                    crate::artifact::read_manifest(&base_version_dir.join("manifest.json"))?;
                serde_json::json!({"kind":"reprocess", "source_id":manifest.source_id, "base_version_id":manifest.version_id,"components":components})
            }
        };
        Ok(serde_json::json!({
            "operation":operation,
            "schema": self.schema, "task_id": self.task_id, "note_id": self.note_id,
            "version_id": self.version_id, "source_id": self.source_id, "title": self.title,
            "author": self.author, "duration": self.duration, "config": config,
            "subtitle_digest": subtitle_digest,
            "subtitle_events": self.subtitle_events, "local_digest": local_digest,
            "service_versions": self.service_versions,
        }))
    }
}

pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        && value != "."
        && value != ".."
}

pub fn validate_events(events: &[TranscriptEvent]) -> Result<()> {
    anyhow::ensure!(
        events.iter().all(|e| e.start.is_finite()
            && e.end.is_finite()
            && e.start >= 0.0
            && e.end >= e.start),
        crate::tr!("Invalid subtitle timestamps")
    );
    anyhow::ensure!(
        events.iter().any(|e| !e.text.trim().is_empty()),
        crate::tr!("Selected subtitles contain no readable text")
    );
    Ok(())
}

pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

pub fn file_digest(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path)
        .with_context(|| crate::tr!("Cannot read {}", path.display(), path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// A mismatch is evidence that this is a different task; preserve both old work and error.
pub fn bind_work_dir(work_dir: &Path, binding: &serde_json::Value) -> Result<()> {
    std::fs::create_dir_all(work_dir)?;
    let path = work_dir.join("task-identity.json");
    if path.exists() {
        let old: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)
            .context(crate::tr!("Task identity is damaged"))?;
        anyhow::ensure!(
            old == *binding,
            crate::tr!(
                "Task inputs changed; create a new task instead of reusing this work directory"
            )
        );
    } else {
        anyhow::ensure!(
            !work_dir.join("asr.jsonl").exists() && !work_dir.join("media.mp4").exists(),
            crate::tr!(
                "This folder contains earlier progress of unknown origin; choose another location"
            )
        );
        crate::checkpoint::atomic_write(&path, &serde_json::to_vec_pretty(binding)?)?;
    }
    Ok(())
}

pub async fn run(request: Request) -> Result<()> {
    let cfg = request.resolve()?;
    let _dispatch = crate::dispatch::install(
        &request.work_dir,
        request.control_path.as_deref(),
        &request.service_versions,
    )?;
    // No settings::load, environment credential resolution or wizard on this path.
    crate::pipeline::run_task(&request, &cfg).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_work_rejects_changed_inputs_without_destroying_progress() {
        let dir = tempfile::tempdir().unwrap();
        bind_work_dir(dir.path(), &serde_json::json!({"title":"A"})).unwrap();
        std::fs::write(dir.path().join("asr.jsonl"), "saved result").unwrap();
        bind_work_dir(dir.path(), &serde_json::json!({"title":"A"})).unwrap();
        assert!(bind_work_dir(dir.path(), &serde_json::json!({"title":"B"})).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("asr.jsonl")).unwrap(),
            "saved result"
        );
    }

    #[test]
    fn malformed_request_does_not_echo_secret() {
        let error = Request::read(br#"{"api_key":"private-test-key"}"#.as_slice())
            .err()
            .unwrap();
        assert!(!format!("{error:#}").contains("private-test-key"));
        assert!(!valid_id("../elsewhere"));
    }
}
