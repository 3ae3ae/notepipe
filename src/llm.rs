//! Prompt-driven paragraph and transcript stages over OpenAI-compatible HTTP.
use crate::timeline::{Section, TranscriptEvent};
use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::path::Path;
#[cfg(test)]
use std::time::Duration;

pub(crate) const IMAGE_NOTE: &str =
    r#" Each request includes the screenshot shown during this passage."#;

#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub prompt: String,
    pub images: bool,
    pub request: Map<String, Value>,
    pub batch_size: usize,
    pub chunk_size: usize,
}

#[derive(Debug, Clone)]
pub struct LlmSettings {
    pub stage: String,
    pub provider: Option<std::sync::Arc<crate::adapter::Provider>>,
    pub warnings: Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
    pub limit: Option<std::sync::Arc<crate::runtime::RequestLimit>>,
    pub cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub base_url: String,
    pub adapter: Vec<String>,
    pub api_key: String,
    pub model: String,
    pub concurrency: usize,
    pub request: Map<String, Value>,
}
impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            stage: String::new(),
            provider: None,
            warnings: None,
            limit: None,
            cancelled: Default::default(),
            base_url: String::new(),
            adapter: Vec::new(),
            api_key: String::new(),
            model: String::new(),
            concurrency: crate::settings::Provider::default().concurrency as usize,
            request: Map::new(),
        }
    }
}
impl LlmSettings {
    pub fn for_stage(provider: &crate::settings::Provider, stage: &Stage, key: String) -> Self {
        Self {
            stage: stage.name.clone(),
            provider: None,
            warnings: None,
            limit: None,
            cancelled: Default::default(),
            base_url: provider.base_url.clone().unwrap_or_default(),
            adapter: provider.argv(),
            api_key: key,
            model: stage.model.clone(),
            concurrency: provider.concurrency as usize,
            request: stage.request.clone(),
        }
    }

    fn cache_identity(&self) -> String {
        if self.adapter.is_empty() {
            endpoint(&self.base_url)
        } else {
            format!("adapter:{}/chat/completions", self.adapter.join("\0"))
        }
    }

    fn check_cancelled(&self) -> Result<()> {
        if self.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(crate::runtime::Cancelled.into());
        }
        crate::runtime::check_cancelled()
    }

    fn acquire_permit(&self) -> Result<Option<crate::runtime::RequestPermit<'_>>> {
        let permit = self
            .limit
            .as_ref()
            .map(|limit| limit.acquire())
            .transpose()?;
        self.check_cancelled()?;
        Ok(permit)
    }
}

pub(crate) fn validate_request(request: &Map<String, Value>) -> Result<()> {
    // The engine owns the protocol and output contract.
    anyhow::ensure!(
        !["model", "messages", "stream", "response_format"]
            .iter()
            .any(|key| request.contains_key(*key)),
        "request cannot override model, messages, stream or response_format"
    );
    Ok(())
}
pub(crate) fn chat_body(
    settings: &LlmSettings,
    system: &str,
    user: impl Into<Value>,
) -> Result<Value> {
    validate_request(&settings.request)?;
    let mut body = settings.request.clone();
    if !settings.model.is_empty() {
        body.insert("model".into(), settings.model.clone().into());
    }
    body.insert("messages".into(), serde_json::json!([{"role":"system","content":system},{"role":"user","content":user.into()}]));
    Ok(body.into())
}

pub fn endpoint(base_url: &str) -> String {
    crate::config::append_endpoint_suffix(base_url.trim(), "/chat/completions")
}
/// HTTP agent shared by all stages of a run, so they reuse one connection pool.
pub(crate) fn chat_agent() -> ureq::Agent {
    crate::dispatch::http_agent()
}

pub fn paragraphs(
    sections: &mut [Section],
    frames_root: &Path,
    s: &LlmSettings,
    stage: &Stage,
) -> Result<Vec<String>> {
    if !sections.iter().any(|section| !section.speech.is_empty()) {
        return Ok(Vec::new());
    }
    let mut chunks: Vec<(Option<std::path::PathBuf>, usize, Vec<&mut TranscriptEvent>)> =
        Vec::new();
    if stage.images {
        for (index, sec) in sections.iter_mut().enumerate() {
            crate::runtime::check_cancelled()?;
            if sec.speech.is_empty() {
                continue;
            }
            // Read and encode once per request batch, not all up front.
            let image = (!sec.image.is_empty()).then(|| frames_root.join(&sec.image));
            let mut refs: Vec<_> = sec.speech.iter_mut().collect();
            while !refs.is_empty() {
                let rest = refs.split_off(refs.len().min(stage.batch_size));
                chunks.push((image.clone(), index + 1, std::mem::replace(&mut refs, rest)));
            }
        }
    } else {
        let mut refs: Vec<_> = sections
            .iter_mut()
            .flat_map(|sec| sec.speech.iter_mut())
            .collect();
        while !refs.is_empty() {
            crate::runtime::check_cancelled()?;
            let rest = refs.split_off(refs.len().min(stage.batch_size));
            chunks.push((None, 0, std::mem::replace(&mut refs, rest)));
        }
    }
    let pb = crate::progress::Bar::new(&stage.name, chunks.len() as u64)
        .with_template("{spinner:.green} llm {pos}/{len} [{bar:32.cyan/blue}] {msg}");
    let agent = chat_agent();
    let image_warnings = std::sync::Mutex::new(Vec::new());
    crate::runtime::map_pool(chunks, s.concurrency, |(image_path, section, mut chunk)| {
        pb.inc(1);
        let image = image_path.as_deref().and_then(|path| match section_image_b64(path) {
            Ok(image) => Some(image),
            Err(error) => {
                let warning = format!("{}: screenshot for section {section} could not be read; batch sent as text: {}: {error:#}", stage.name, path.display());
                let mut warnings = crate::runtime::lock_recover(&image_warnings);
                if !warnings.contains(&warning) {
                    tracing::warn!("{warning}");
                    if let Some(shared) = &s.warnings {
                        crate::runtime::lock_recover(shared).push(warning.clone());
                    }
                    warnings.push(warning);
                }
                None
            }
        });
        let items: Vec<&str> = chunk.iter().map(|event| event.text.as_str()).collect();
        let detail = format!(
            "paragraphs at {}–{}",
            crate::render::fmt_ts(chunk.first().unwrap().start),
            crate::render::fmt_ts(chunk.last().unwrap().end)
        );
        let results = chat(&agent, s, stage, &items, image.as_deref(), &detail)?;
        apply_results(&mut chunk, &results);
        Ok(())
    })?;
    pb.finish();
    // Remove paragraphs a replacing stage emptied, once all batches are done.
    for sec in sections.iter_mut() {
        sec.speech.retain(|e| !e.text.trim().is_empty());
    }
    if !crate::artifact::has_readable_body(sections) {
        crate::dispatch::warn(format!("Stage {} removed every paragraph", stage.name));
    }
    Ok(crate::runtime::lock_recover(&image_warnings).clone())
}

fn section_image_b64(path: &Path) -> Result<String> {
    use base64::Engine as _;
    Ok(base64::engine::general_purpose::STANDARD.encode(std::fs::read(path)?))
}

fn apply_results(chunk: &mut [&mut TranscriptEvent], results: &[(usize, String)]) {
    for (id, text) in results {
        // chat validates every id before applying a batch.
        let event = &mut chunk[*id];
        if *text != event.text {
            event.raw.get_or_insert_with(|| event.text.clone());
            event.text = text.clone();
        }
    }
}

fn chat(
    agent: &ureq::Agent,
    settings: &LlmSettings,
    stage: &Stage,
    items: &[&str],
    image: Option<&str>,
    detail: &str,
) -> Result<Vec<(usize, String)>> {
    let tag = batch_tag(items);
    let body = build_chat_body(settings, stage, items, image)?;
    let validate = |content: &str| {
        parse_blocks(content, &tag, items.len())
            .context("The answer's blocks do not match the paragraphs sent")
    };
    let content = send_chat(agent, settings, &body, &stage.name, detail, &|content| {
        validate(content).map(|_| ())
    })?;
    validate(&content)
}

fn batch_tag(items: &[&str]) -> String {
    let digest = crate::runtime::digest(items.join("\n").as_bytes());
    let mut length = 6;
    while items
        .iter()
        .any(|text| text.contains(&format!("<<<np-{}:", &digest[..length])))
    {
        length += 1;
    }
    format!("np-{}", &digest[..length])
}

fn blocks_contract(tag: &str) -> String {
    format!(
        "The input is a series of blocks, each opened by a line <<<{tag}:ID>>> and the last closed by a line <<<{tag}:end>>>. Output the same series of blocks, one for every input ID in the same order, with your result as plain text between the marker lines, and nothing outside them. Write text exactly as it should appear; nothing is escaped."
    )
}
fn build_chat_body(
    settings: &LlmSettings,
    stage: &Stage,
    items: &[&str],
    image: Option<&str>,
) -> Result<Value> {
    let tag = batch_tag(items);
    let mut payload = String::new();
    for (id, text) in items.iter().enumerate() {
        payload.push_str(&format!("<<<{tag}:{id}>>>\n{text}\n"));
    }
    payload.push_str(&format!("<<<{tag}:end>>>"));
    let image = image.filter(|_| stage.images);
    let mut system = format!("{} {}", stage.prompt, blocks_contract(&tag));
    if image.is_some() {
        system.push_str(IMAGE_NOTE);
    }
    let mut content = vec![serde_json::json!({"type":"text","text":payload})];
    if let Some(image) = image {
        content.push(serde_json::json!({"type":"image_url","image_url":{"url":format!("data:image/jpeg;base64,{image}")}}));
    }
    chat_body(settings, &system, Value::Array(content))
}

fn parse_blocks(content: &str, tag: &str, expected: usize) -> Option<Vec<(usize, String)>> {
    let prefix = format!("<<<{tag}:");
    let mut blocks = Vec::new();
    let mut lines = Vec::new();
    let mut opened = 0;
    let mut ended = false;
    for line in content.split('\n') {
        let marker = line
            .trim_end()
            .strip_prefix(&prefix)
            .and_then(|s| s.strip_suffix(">>>"));
        if let Some(id) = marker {
            if ended {
                return None;
            }
            if opened > 0 {
                blocks.push((opened - 1, lines.join("\n")));
                lines.clear();
            }
            if id == "end" {
                if opened != expected {
                    return None;
                }
                ended = true;
            } else if opened < expected && id == opened.to_string() {
                opened += 1;
            } else {
                return None;
            }
        } else if opened > 0 && !ended {
            lines.push(line);
        }
    }
    ended.then_some(blocks)
}

/// Only validated responses enter the cache.
pub(crate) fn send_chat(
    agent: &ureq::Agent,
    s: &LlmSettings,
    body: &Value,
    stage: &str,
    detail: &str,
    validate: &dyn Fn(&str) -> Result<()>,
) -> Result<String> {
    let item = crate::dispatch::Item::stage(stage, detail);
    let response = crate::dispatch::request(&item, &|| s.check_cancelled(), || {
        request_chat_once(agent, s, body, stage, validate)
    })?;
    Ok(response["choices"][0]["message"]["content"]
        .as_str()
        .unwrap()
        .to_owned())
}

fn request_chat_once(
    agent: &ureq::Agent,
    s: &LlmSettings,
    body: &Value,
    stage: &str,
    validate: &dyn Fn(&str) -> Result<()>,
) -> std::result::Result<Value, crate::dispatch::Failure> {
    s.check_cancelled()
        .map_err(crate::dispatch::Failure::local)?;
    crate::dispatch::cached_json_request(
        &s.cache_identity(),
        body,
        &s.api_key,
        stage,
        || {
            let _permit = s
                .acquire_permit()
                .map_err(crate::dispatch::Failure::local)?;
            let base = match &s.provider {
                Some(provider) => provider.base(&s.stage, &|| s.check_cancelled()),
                None => Ok(s.base_url.clone()),
            }
            .map_err(crate::dispatch::Failure::local)?;
            let url = endpoint(&base);
            let mut request = agent.post(&url).set("Content-Type", "application/json");
            if !s.api_key.is_empty() {
                request = request.set("Authorization", &format!("Bearer {}", s.api_key));
            }
            crate::dispatch::send_http(
                &|| s.check_cancelled(),
                request,
                serde_json::to_vec(body).map_err(crate::dispatch::Failure::local)?,
                &s.api_key,
            )
        },
        |value| {
            crate::dispatch::check_finish_reason(value)?;
            anyhow::ensure!(
                value.get("error").is_none_or(Value::is_null),
                "The AI service returned an error"
            );
            let content = value["choices"][0]["message"]["content"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .context("The AI service returned no content")?;
            validate(content)
        },
    )
}

#[cfg(test)]
pub(crate) fn test_stage(name: &str, value: Value) -> Result<Stage> {
    let definition: crate::settings::Stage = serde_json::from_value(value)?;
    let stage = definition.ai(name).context("Not an AI stage")?;
    validate_request(&stage.request)?;
    Ok(stage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::paragraph_answer;

    #[test]
    fn cancellation_is_checked_after_waiting_for_a_provider_permit() {
        use std::sync::{Arc, atomic::Ordering};
        let limit = Arc::new(crate::runtime::RequestLimit::new(1));
        let permit = limit.acquire().unwrap();
        let settings = LlmSettings {
            limit: Some(limit.clone()),
            ..Default::default()
        };
        let (started, initial_check_passed) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                settings.check_cancelled().unwrap();
                started.send(()).unwrap();
                settings
                    .acquire_permit()
                    .err()
                    .expect("post-permit cancellation")
            });
            initial_check_passed
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            settings.cancelled.store(true, Ordering::Relaxed);
            drop(permit);
            assert!(worker.join().unwrap().to_string().contains("Cancelled"));
        });
    }

    #[test]
    fn stages_resolve_only_their_named_provider_and_allow_an_empty_model_and_prompt() {
        let service = crate::settings::Provider {
            base_url: Some("https://example.test/v1".into()),
            concurrency: 2,
            ..Default::default()
        };
        let stage: Stage = crate::llm::test_stage("custom", serde_json::json!({"type":"write","output":"test.md","provider":"ai","prompt":"  ","request":{"temperature":0.7}})).unwrap();
        assert!(stage.prompt.is_empty());
        let effective = LlmSettings::for_stage(&service, &stage, String::new());
        assert_eq!(effective.base_url, service.base_url.unwrap());
        assert!(effective.api_key.is_empty());
        assert_eq!(effective.concurrency, 2);
        let body = chat_body(&effective, &stage.prompt, "User").unwrap();
        assert!(body.get("model").is_none());
        assert_eq!(body["messages"][0]["content"], "");
        assert_eq!(body["temperature"], 0.7);
    }

    #[test]
    fn adapter_cache_identity_tracks_arguments_instead_of_the_port() {
        let mut settings = LlmSettings {
            adapter: vec!["ai-adapter".into(), "--option".into()],
            base_url: "http://127.0.0.1:12345/v1".into(),
            ..Default::default()
        };
        let identity = settings.cache_identity();
        assert_eq!(identity, "adapter:ai-adapter\0--option/chat/completions");
        assert!(!identity.contains("12345"));
        settings.base_url = "http://127.0.0.1:54321/v1".into();
        assert_eq!(settings.cache_identity(), identity);
        settings.adapter.push("changed".into());
        assert_ne!(settings.cache_identity(), identity);
        settings.adapter.clear();
        assert_eq!(settings.cache_identity(), endpoint(&settings.base_url));
    }

    #[test]
    fn adapter_cache_survives_port_changes() {
        use crate::test_support::MockAi;
        use serde_json::json;
        use std::sync::atomic::Ordering;
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        let root = tempfile::tempdir().unwrap();
        let _cache = crate::dispatch::install(root.path()).unwrap();
        let first = MockAi::respond_with(|_, body| {
            assert!(body.get("response_format").is_none());
            Some((
                200,
                json!({"choices":[{"message":{"content":"Cached response."}}]}),
            ))
        });
        let second = MockAi::respond_with(|_, _| {
            Some((
                200,
                json!({"choices":[{"message":{"content":"Unexpected request."}}]}),
            ))
        });
        let mut settings = LlmSettings {
            adapter: vec!["ai-adapter".into()],
            base_url: first.url.clone(),
            ..Default::default()
        };
        let body = chat_body(&settings, "System", "User").unwrap();
        for url in [&first.url, &second.url] {
            settings.base_url.clone_from(url);
            assert_eq!(
                send_chat(
                    &chat_agent(),
                    &settings,
                    &body,
                    "proofread",
                    "",
                    &|_| Ok(())
                )
                .unwrap(),
                "Cached response."
            );
        }
        assert_eq!(first.calls.load(Ordering::SeqCst), 1);
        assert_eq!(second.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn truncated_chat_is_not_retried_validated_or_cached() {
        use crate::test_support::MockAi;
        use serde_json::json;
        use std::sync::atomic::Ordering;
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        for content in [
            "<<<np-test:0>>>\nChanged.\n<<<np-test:end>>>",
            "## Summary\nA complete-looking result.",
        ] {
            let root = tempfile::tempdir().unwrap();
            let _cache = crate::dispatch::install(root.path()).unwrap();
            let mock = MockAi::respond_with(move |call, _| {
                Some((
                    200,
                    json!({"choices":[{"finish_reason":if call == 0 { "length" } else { "stop" },"message":{"content":content}}]}),
                ))
            });
            let settings = LlmSettings {
                base_url: mock.url.clone(),
                ..Default::default()
            };
            let body = chat_body(&settings, "System", "User").unwrap();
            let validations = std::cell::Cell::new(0);
            let validate = |value: &str| {
                validations.set(validations.get() + 1);
                assert_eq!(value, content);
                Ok(())
            };
            let agent = chat_agent();
            let error =
                send_chat(&agent, &settings, &body, "proofread", "", &validate).unwrap_err();
            let failure = error.downcast_ref::<crate::dispatch::Failure>().unwrap();
            assert!(!failure.retryable);
            assert_eq!(
                error.to_string(),
                "Stage proofread failed: the model stopped at its output limit"
            );
            assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
            assert_eq!(validations.get(), 0);
            assert!(!root.path().join("responses").exists());

            let response = send_chat(&agent, &settings, &body, "proofread", "", &validate).unwrap();
            assert_eq!(response, content);
            let cached = send_chat(&agent, &settings, &body, "proofread", "", &validate).unwrap();
            assert_eq!(cached, content);
            assert_eq!(mock.calls.load(Ordering::SeqCst), 2);
            assert_eq!(validations.get(), 2);
        }
    }

    #[test]
    fn truncated_paragraph_stage_fails() {
        use crate::test_support::MockAi;
        use serde_json::json;
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        let mock = MockAi::respond_with(|_, body| {
            Some((
                200,
                json!({"choices":[{"finish_reason":"length","message":{"content":paragraph_answer(body, &["Changed."])}}]}),
            ))
        });
        let event = TranscriptEvent {
            extra: Default::default(),
            start: 0.,
            end: 1.,
            text: "Original.".into(),
            raw: None,
        };
        let mut sections = vec![Section {
            extra: Default::default(),
            t: 0.,
            end: 1.,
            image: String::new(),
            speech: vec![event.clone()],
        }];
        let settings = LlmSettings {
            base_url: mock.url.clone(),
            ..Default::default()
        };
        let root = tempfile::tempdir().unwrap();
        let report = paragraphs(
            &mut sections,
            root.path(),
            &settings,
            &crate::llm::test_stage(
                "proofread",
                serde_json::json!({"type":"rewrite","output":"test.json","provider":"ai","prompt":"CUSTOM"}),
            )
            .unwrap(),
        )
        .unwrap_err();
        assert!(
            report
                .to_string()
                .contains("the model stopped at its output limit")
        );
        assert_eq!(sections[0].speech, vec![event]);
        assert_eq!(mock.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn paragraph_blocks_preserve_latex_whitespace_and_raw_byte_for_byte() {
        use crate::test_support::MockAi;
        use serde_json::json;
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        let text = " \n\t$\\hat{y} \\beta \\frac{1}{2} \\theta \\nabla$  \n";
        let mock = MockAi::respond_with(move |_, body| {
            assert!(body.get("response_format").is_none());
            Some((
                200,
                json!({"choices":[{"message":{"content":format!("Ignored before\n{}\nIgnored after", paragraph_answer(body, &[text]))}}]}),
            ))
        });
        let settings = LlmSettings {
            base_url: mock.url.clone(),
            ..Default::default()
        };
        let stage = test_stage(
            "proofread",
            json!({"type":"rewrite","output":"plain","provider":"ai"}),
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut sections = vec![Section {
            extra: Default::default(),
            t: 0.,
            end: 1.,
            image: String::new(),
            speech: vec![TranscriptEvent {
                extra: Default::default(),
                start: 0.,
                end: 1.,
                text: "Original".into(),
                raw: None,
            }],
        }];
        paragraphs(&mut sections, root.path(), &settings, &stage).unwrap();
        assert_eq!(sections[0].speech[0].text.as_bytes(), text.as_bytes());
        assert_eq!(sections[0].speech[0].raw.as_deref(), Some("Original"));
        paragraphs(&mut sections, root.path(), &settings, &stage).unwrap();
        assert_eq!(sections[0].speech[0].raw.as_deref(), Some("Original"));
    }

    #[test]
    fn mismatched_blocks_retry_and_only_the_valid_answer_is_cached() {
        use crate::test_support::MockAi;
        use serde_json::json;
        use std::sync::atomic::Ordering;
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        for bad in ["missing", "reordered", "repeated", "end", "extra"] {
            let root = tempfile::tempdir().unwrap();
            let _cache = crate::dispatch::install(root.path()).unwrap();
            let mock = MockAi::respond_with(move |call, body| {
                let mut answer = paragraph_answer(body, &["First", "Second"]);
                if call == 0 {
                    let tag = batch_tag(&["one", "two"]);
                    answer = match bad {
                        "missing" => paragraph_answer(body, &["First"]),
                        "reordered" => answer
                            .replace(":0>>>", ":TEMP>>>")
                            .replace(":1>>>", ":0>>>")
                            .replace(":TEMP>>>", ":1>>>"),
                        "repeated" => answer.replace(":1>>>", ":0>>>"),
                        "end" => answer.replace(&format!("<<<{tag}:end>>>"), ""),
                        _ => paragraph_answer(body, &["First", "Second", "Extra"]),
                    };
                }
                Some((200, json!({"choices":[{"message":{"content":answer}}]})))
            });
            let settings = LlmSettings {
                base_url: mock.url.clone(),
                ..Default::default()
            };
            let stage = test_stage(
                "proofread",
                json!({"type":"rewrite","output":"plain","provider":"ai"}),
            )
            .unwrap();
            for _ in 0..2 {
                assert_eq!(
                    chat(&chat_agent(), &settings, &stage, &["one", "two"], None, "").unwrap(),
                    vec![(0, "First".into()), (1, "Second".into())]
                );
                assert_eq!(mock.calls.load(Ordering::SeqCst), 2, "{bad}");
                assert_eq!(
                    std::fs::read_dir(root.path().join("responses"))
                        .unwrap()
                        .count(),
                    1
                );
            }
        }
    }

    #[test]
    fn marker_parser_requires_ordered_ids_and_end_and_ignores_outside_text() {
        let answer = "Before\n<<<np-test:0>>> \t\n  first\t\n\n<<<np-test:1>>>\n\t \n<<<np-test:end>>> \t\nAfter";
        assert_eq!(
            parse_blocks(answer, "np-test", 2).unwrap(),
            vec![(0, "  first\t\n".into()), (1, "\t ".into())]
        );
        assert_eq!(
            parse_blocks("<<<np-test:0>>>\n<<<np-test:end>>>", "np-test", 1).unwrap(),
            vec![(0, "".into())]
        );
        for answer in [
            "no blocks",
            "<<<np-test:0>>>\ntext",
            "<<<np-test:1>>>\n<<<np-test:end>>>",
            "<<<np-test:0>>>\n<<<np-test:0>>>\n<<<np-test:end>>>",
            "<<<np-test:0>>>\n<<<np-test:1>>>\n<<<np-test:end>>>",
            "<<<np-test:00>>>\n<<<np-test:end>>>",
            "<<<np-test:0>>>\n<<<np-test:end>>>\n<<<np-test:1>>>",
            " <<<np-test:0>>>\n<<<np-test:end>>>",
        ] {
            assert!(parse_blocks(answer, "np-test", 1).is_none(), "{answer}");
        }
    }

    #[test]
    fn batch_tags_hash_joined_text_and_lengthen_for_a_marker_collision() {
        assert_eq!(
            batch_tag(&["one", "two"]),
            format!("np-{}", &crate::runtime::digest(b"one\ntwo")[..6])
        );
        // This text's SHA-256 starts with 0000009, colliding with its own six-digit marker.
        let text = "<<<np-000000: marker 32684245";
        assert_eq!(batch_tag(&[text]), "np-0000009");
        let stage = test_stage(
            "proofread",
            serde_json::json!({"type":"rewrite","output":"plain","provider":"ai"}),
        )
        .unwrap();
        let body = build_chat_body(&test_settings(), &stage, &[text], None).unwrap();
        assert_eq!(
            body["messages"][1]["content"][0]["text"],
            format!("<<<np-0000009:0>>>\n{text}\n<<<np-0000009:end>>>")
        );
    }

    #[test]
    fn endpoint_join() {
        assert_eq!(
            endpoint("https://api.deepseek.com/v1"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            endpoint("https://api.x.com/v1/"),
            "https://api.x.com/v1/chat/completions"
        );
        assert_eq!(
            endpoint("https://api.x.com/v1/chat/completions"),
            "https://api.x.com/v1/chat/completions"
        );
    }

    fn test_settings() -> LlmSettings {
        LlmSettings {
            base_url: "https://example.test/v1".into(),
            api_key: "k".into(),
            model: "m".into(),
            ..Default::default()
        }
    }

    #[test]
    fn polish_records_raw_text() {
        let event = |text: &str| TranscriptEvent {
            extra: Default::default(),
            start: 0.0,
            end: 1.0,
            text: text.into(),
            raw: None,
        };
        let (mut a, mut b) = (event("今天讲编译原理"), event("啊"));
        let mut refs = vec![&mut a, &mut b];
        let polished = [(0, "今天讲编译原理".into()), (1, String::new())];
        apply_results(&mut refs, &polished);
        assert_eq!(a.raw, None, "unchanged text keeps no raw copy");
        assert_eq!(b.text, "", "filler is emptied and removed by the caller");
        assert_eq!(b.raw.as_deref(), Some("啊"), "the original is kept in raw");
    }

    #[test]
    fn chat_body_text_vs_vision() {
        let s = test_settings();
        let mut stage: Stage = crate::llm::test_stage(
            "proofread",
            serde_json::json!({"type":"rewrite","output":"test.json","provider":"ai","prompt":"CUSTOM"}),
        )
        .unwrap();
        let items = ["hello"];
        let text_only = build_chat_body(&s, &stage, &items, None).unwrap();
        assert!(text_only.get("response_format").is_none());
        assert_eq!(
            text_only["messages"][1]["content"][0]["text"],
            "<<<np-2cf24d:0>>>\nhello\n<<<np-2cf24d:end>>>"
        );
        assert_eq!(text_only["model"], "m");
        assert!(text_only.get("max_tokens").is_none());
        assert!(text_only.get("temperature").is_none());
        let user: &Vec<serde_json::Value> = text_only["messages"][1]["content"].as_array().unwrap();
        assert_eq!(user.len(), 1);
        assert_eq!(user[0]["type"], "text");

        let disabled = build_chat_body(&s, &stage, &items, Some("aGVsbG8=")).unwrap();
        assert_eq!(disabled, text_only);
        stage.images = true;
        let vision = build_chat_body(&s, &stage, &items, Some("aGVsbG8=")).unwrap();
        let user: &Vec<serde_json::Value> = vision["messages"][1]["content"].as_array().unwrap();
        assert_eq!(user.len(), 2, "an image adds an image_url part");
        assert_eq!(user[1]["type"], "image_url");
        assert_eq!(
            user[1]["image_url"]["url"].as_str().unwrap(),
            "data:image/jpeg;base64,aGVsbG8="
        );
        let sys = vision["messages"][0]["content"].as_str().unwrap();
        assert_eq!(
            sys,
            format!("CUSTOM {}{IMAGE_NOTE}", blocks_contract(&batch_tag(&items)))
        );
    }

    #[test]
    fn stages_keep_their_declared_contracts() {
        let settings = LlmSettings::default();
        let mut custom: Stage = crate::llm::test_stage(
            "annotate",
            serde_json::json!({"type":"rewrite","output":"test.json","provider":"ai","prompt":"CUSTOM"}),
        )
        .unwrap();
        let body = build_chat_body(&settings, &custom, &["Text"], None).unwrap();
        assert_eq!(
            body["messages"][0]["content"],
            format!("CUSTOM {}", blocks_contract(&batch_tag(&["Text"])))
        );
        assert!(
            !body["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("LaTeX")
        );
        let empty_prompt: Stage = crate::llm::test_stage(
            "custom",
            serde_json::json!({"type":"write","output":"test.md","provider":"ai"}),
        )
        .unwrap();
        assert!(empty_prompt.prompt.is_empty());
        custom.images = true;
        for name in ["proofread", "annotate"] {
            custom.name = name.into();
            let body = build_chat_body(&settings, &custom, &["Text"], Some("image")).unwrap();
            assert_eq!(
                body["messages"][0]["content"],
                format!(
                    "CUSTOM {}{IMAGE_NOTE}",
                    blocks_contract(&batch_tag(&["Text"]))
                )
            );
            let body = build_chat_body(&settings, &custom, &["Text"], None).unwrap();
            assert_eq!(
                body["messages"][0]["content"],
                format!("CUSTOM {}", blocks_contract(&batch_tag(&["Text"])))
            );
        }
    }

    #[test]
    fn stage_requests_merge_with_the_protocol_and_protected_keys_are_rejected() {
        let stage: Stage = crate::llm::test_stage("summary", serde_json::json!({"type":"write","output":"test.md","provider":"ai","model":"stage-model","request":{"temperature":0.7,"top_p":0.8}})).unwrap();
        let service = crate::settings::Provider {
            base_url: Some("https://example.test/v1".into()),
            ..Default::default()
        };
        let effective = LlmSettings::for_stage(&service, &stage, String::new());
        let body = chat_body(&effective, "System", "User").unwrap();
        assert_eq!(body["model"], "stage-model");
        assert_eq!(body["temperature"], 0.7);
        assert_eq!(body["top_p"], 0.8);
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("response_format").is_none());
        for request in [
            serde_json::json!({"model":"forbidden"}),
            serde_json::json!({"messages":[]}),
            serde_json::json!({"stream":false}),
            serde_json::json!({"response_format":{}}),
            serde_json::json!([]),
            serde_json::json!("text"),
            serde_json::json!(1),
        ] {
            assert!(
                crate::llm::test_stage(
                    "summary",
                    serde_json::json!({"type":"write","output":"test.md","provider":"ai","request":request})
                )
                .is_err()
            );
        }
    }

    #[test]
    fn paragraph_images_fall_back_to_text_and_warn_only_for_missing_assets() {
        use crate::test_support::MockAi;
        use serde_json::json;
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        let mock = MockAi::respond_with(|_, body| {
            let content = body["messages"][1]["content"].as_array().unwrap();
            assert_eq!(content.len(), 1);
            let system = body["messages"][0]["content"].as_str().unwrap();
            assert!(!system.contains("Each request includes the screenshot"));
            let answer = content[0]["text"]
                .as_str()
                .unwrap()
                .lines()
                .map(|line| {
                    if line.starts_with("<<<np-") {
                        line.to_owned()
                    } else {
                        format!("Processed {line}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            Some((200, json!({"choices":[{"message":{"content":answer}}]})))
        });
        let root = tempfile::tempdir().unwrap();
        let mut sections = ["", "missing.jpg"]
            .into_iter()
            .enumerate()
            .map(|(index, image)| crate::timeline::Section {
                extra: Default::default(),
                t: index as f64,
                end: index as f64 + 1.,
                image: image.into(),
                speech: (0..21)
                    .map(|number| crate::timeline::TranscriptEvent {
                        extra: Default::default(),
                        start: index as f64,
                        end: index as f64 + 1.,
                        text: format!("Original {number}."),
                        raw: None,
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        let settings = crate::llm::LlmSettings {
            base_url: mock.url.clone(),
            model: "fixture".into(),
            ..Default::default()
        };
        let mut stage = crate::llm::test_stage(
            "proofread",
            serde_json::json!({"type":"rewrite","output":"test.json","provider":"ai","prompt":"CUSTOM"}),
        )
        .unwrap();
        stage.name = "annotate".into();
        stage.images = true;
        stage.batch_size = 7;
        let warnings =
            crate::llm::paragraphs(&mut sections, root.path(), &settings, &stage).unwrap();
        assert_eq!(
            warnings.len(),
            1,
            "one warning per section, even across batches"
        );
        assert!(
            warnings[0].contains("annotate")
                && warnings[0].contains("section 2")
                && warnings[0].contains("missing.jpg")
        );
        assert!(
            sections
                .iter()
                .flat_map(|section| &section.speech)
                .all(|event| event.text.starts_with("Processed Original"))
        );
        let sent = mock
            .bodies
            .lock()
            .unwrap()
            .iter()
            .map(|body| {
                body["messages"][1]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .lines()
                    .filter(|line| line.starts_with("<<<np-") && !line.ends_with(":end>>>"))
                    .count()
            })
            .sum::<usize>();
        assert_eq!(sent, 42);
        assert!(
            mock.headers
                .lock()
                .unwrap()
                .iter()
                .all(|header| !header.to_ascii_lowercase().contains("authorization:"))
        );
        assert_eq!(mock.calls.load(std::sync::atomic::Ordering::SeqCst), 6);
        let failed = MockAi::respond_with(|_, _| Some((400, json!({}))));
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let settings = LlmSettings {
            base_url: failed.url.clone(),
            warnings: Some(shared.clone()),
            ..settings
        };
        assert!(paragraphs(&mut sections[1..], root.path(), &settings, &stage).is_err());
        let retained = crate::runtime::lock_recover(&shared);
        assert_eq!(retained.len(), 1);
        assert!(retained[0].contains("missing.jpg"));
    }
}
