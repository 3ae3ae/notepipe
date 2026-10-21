//! Prompt-driven paragraph and transcript stages over OpenAI-compatible HTTP.
use crate::timeline::{Section, StageText, TranscriptEvent};
use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::{path::Path, time::Duration};

pub(crate) const PROOFREAD_PROMPT: &str = r#"You proofread transcripts of spoken recordings. Each input item is a stretch of continuous speech, already split at natural pauses. Fix obvious speech-recognition errors (misheard words, homophones, misspelled names and terms), remove filler that adds nothing, and repair awkward sentence breaks and punctuation so the text reads naturally. Do not summarize, expand, translate, add or remove substantive content, or change the meaning. Keep the original language. If an item contains only filler or meaningless fragments (such as a lone "um" or "you know"), return an empty text for it; never empty an item that has real content. Write mathematical formulas, symbols and expressions as LaTeX: wrap inline math in $...$ and put display math on its own line in $$...$$; rewrite spoken formulas, such as "x squared", as LaTeX too; do not add $ when there is no math. If a screenshot is attached, use it only to correct the spelling of terms and proper names; do not describe or comment on it."#;
pub(crate) const PARAGRAPHS_CONTRACT: &str = r#"Output a JSON object {"segments":[{"id":number,"text":result}]} with exactly one entry per input id. Inside JSON strings, escape every backslash as \\."#;
pub(crate) const IMAGE_NOTE: &str =
    r#" Each request includes the screenshot shown during this passage."#;

const BATCH: usize = 20;
const DEFAULT_CONCURRENCY: usize = 8;
const MAX_CONCURRENCY: usize = 16;
const MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Input {
    Paragraphs,
    Transcript,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Output {
    Replace,
    Below,
    Top,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stage {
    pub name: String,
    pub input: Input,
    pub output: Output,
    pub prompt: String,
    pub images: bool,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub request: Map<String, Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StageFields {
    name: String,
    input: Option<Input>,
    output: Option<Output>,
    prompt: Option<String>,
    #[serde(default)]
    images: bool,
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    #[serde(default, deserialize_with = "deserialize_request")]
    request: Map<String, Value>,
}

fn builtin_defaults(name: &str) -> Option<(Input, Output, &'static str)> {
    match name {
        "proofread" => Some((Input::Paragraphs, Output::Replace, PROOFREAD_PROMPT)),
        "summary" => Some((
            Input::Transcript,
            Output::Top,
            crate::summarize::SUMMARY_PROMPT,
        )),
        _ => None,
    }
}
impl<'de> Deserialize<'de> for Stage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error;
        let fields = StageFields::deserialize(deserializer)?;
        let defaults = builtin_defaults(&fields.name);
        let stage = Self {
            name: fields.name,
            input: fields
                .input
                .or(defaults.map(|value| value.0))
                .ok_or_else(|| D::Error::custom("Custom stages require input"))?,
            output: fields
                .output
                .or(defaults.map(|value| value.1))
                .ok_or_else(|| D::Error::custom("Custom stages require output"))?,
            prompt: fields
                .prompt
                .or_else(|| defaults.map(|value| value.2.to_owned()))
                .ok_or_else(|| D::Error::custom("Custom stages require prompt"))?,
            images: fields.images,
            base_url: fields.base_url,
            api_key: fields.api_key,
            model: fields.model,
            request: fields.request,
        };
        stage.validate().map_err(D::Error::custom)?;
        Ok(stage)
    }
}
impl Stage {
    pub fn builtin(name: &str) -> Option<Self> {
        let (input, output, prompt) = builtin_defaults(name)?;
        Some(Self {
            name: name.into(),
            input,
            output,
            prompt: prompt.into(),
            images: false,
            base_url: None,
            api_key: None,
            model: None,
            request: Map::new(),
        })
    }
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.name.trim().is_empty(),
            "LLM stage names must not be empty"
        );
        anyhow::ensure!(
            matches!(
                (self.input, self.output),
                (Input::Paragraphs, Output::Replace | Output::Below)
                    | (Input::Transcript, Output::Top)
            ),
            "Invalid LLM stage input/output combination"
        );
        anyhow::ensure!(
            !self.images || self.input == Input::Paragraphs,
            "Only paragraphs stages support images"
        );
        validate_request(&self.request)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LlmSettings {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub concurrency: usize,
    #[serde(deserialize_with = "deserialize_request")]
    pub request: Map<String, Value>,
    #[serde(deserialize_with = "deserialize_stages")]
    pub stages: Vec<Stage>,
}
impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            concurrency: DEFAULT_CONCURRENCY,
            request: Map::new(),
            stages: Vec::new(),
        }
    }
}
impl LlmSettings {
    pub fn for_stage(&self, stage: &Stage) -> Result<Self> {
        validate_request(&self.request)?;
        stage.validate()?;
        let mut request = self.request.clone();
        request.extend(stage.request.clone());
        Ok(Self {
            base_url: stage
                .base_url
                .clone()
                .unwrap_or_else(|| self.base_url.clone()),
            api_key: stage
                .api_key
                .clone()
                .unwrap_or_else(|| self.api_key.clone()),
            model: stage.model.clone().unwrap_or_else(|| self.model.clone()),
            concurrency: self.concurrency.clamp(1, MAX_CONCURRENCY),
            request,
            stages: Vec::new(),
        })
    }
}

fn validate_request(request: &Map<String, Value>) -> Result<()> {
    // The engine owns the protocol and output contract.
    anyhow::ensure!(
        !["model", "messages", "stream", "response_format"]
            .iter()
            .any(|key| request.contains_key(*key)),
        "LLM request cannot override model, messages, stream or response_format"
    );
    Ok(())
}
fn deserialize_request<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Map<String, Value>, D::Error> {
    let request = Map::deserialize(deserializer)?;
    validate_request(&request).map_err(serde::de::Error::custom)?;
    Ok(request)
}

fn validate_stage_names(stages: &[Stage]) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    for stage in stages {
        anyhow::ensure!(
            names.insert(&stage.name),
            "Duplicate LLM stage name: {}",
            stage.name
        );
    }
    Ok(())
}
fn deserialize_stages<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<Stage>, D::Error> {
    let stages = Vec::deserialize(deserializer)?;
    validate_stage_names(&stages).map_err(serde::de::Error::custom)?;
    Ok(stages)
}

pub(crate) fn chat_body(
    settings: &LlmSettings,
    system: &str,
    user: impl Into<Value>,
    input: Input,
) -> Result<Value> {
    validate_request(&settings.request)?;
    let mut body = settings.request.clone();
    body.insert("model".into(), settings.model.clone().into());
    body.insert("messages".into(), serde_json::json!([{"role":"system","content":system},{"role":"user","content":user.into()}]));
    if input == Input::Paragraphs {
        body.insert(
            "response_format".into(),
            serde_json::json!({"type":"json_object"}),
        );
    }
    Ok(body.into())
}

pub fn endpoint(base_url: &str) -> String {
    crate::config::append_endpoint_suffix(base_url.trim(), "/chat/completions")
}
pub fn validate(settings: &LlmSettings) -> Result<()> {
    validate_request(&settings.request)?;
    validate_stage_names(&settings.stages)?;
    for stage in &settings.stages {
        let settings = settings.for_stage(stage)?;
        anyhow::ensure!(
            !settings.base_url.trim().is_empty(),
            "AI service URL is not set. Set [llm].base_url or the stage's base_url."
        );
        anyhow::ensure!(
            !settings.model.trim().is_empty(),
            "AI model is not set. Set [llm].model or the stage's model."
        );
        crate::config::ensure_http_url(&settings.base_url)
            .with_context(|| format!("Invalid service for stage {}", stage.name))?;
    }
    Ok(())
}

/// HTTP agent shared by all stages of a run, so they reuse one connection pool.
/// A response that sends nothing for [`STALL_TIMEOUT`] has stalled; it is
/// abandoned then instead of holding the task for the whole request budget.
pub(crate) fn chat_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(300))
        .timeout_read(STALL_TIMEOUT)
        .redirects(0)
        .build()
}

const STALL_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Default)]
pub struct StageReport {
    pub attempted: usize,
    pub failed: usize,
    /// The first failure, shortened for messages; None when nothing failed.
    pub note: Option<String>,
    pub warnings: Vec<String>,
}

pub fn paragraphs(
    sections: &mut [Section],
    frames_root: &Path,
    s: &LlmSettings,
    stage: &Stage,
) -> Result<StageReport> {
    let attempted = sections.iter().map(|section| section.speech.len()).sum();
    if attempted == 0 {
        return Ok(StageReport::default());
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
                let rest = refs.split_off(refs.len().min(BATCH));
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
            let rest = refs.split_off(refs.len().min(BATCH));
            chunks.push((None, 0, std::mem::replace(&mut refs, rest)));
        }
    }
    let pb = crate::progress::Bar::new(&stage.name, chunks.len() as u64)
        .with_template("{spinner:.green} llm {pos}/{len} [{bar:32.cyan/blue}] {msg}");
    let agent = chat_agent();
    let first_error = std::sync::Mutex::new(None::<String>);
    let image_warnings = std::sync::Mutex::new(Vec::new());
    let failures = crate::runtime::map_pool(
        chunks,
        s.concurrency.clamp(1, MAX_CONCURRENCY),
        |(image_path, section, mut chunk)| {
            pb.inc(1);
            let image = image_path.as_deref().and_then(|path| match section_image_b64(path) {
                Ok(image) => Some(image),
                Err(error) => {
                    let warning = format!("{}: screenshot for section {section} could not be read; batch sent as text: {}: {error:#}", stage.name, path.display());
                    let mut warnings = crate::runtime::lock_recover(&image_warnings);
                    if !warnings.contains(&warning) {
                        tracing::warn!("{warning}");
                        warnings.push(warning);
                    }
                    None
                }
            });
            let items: Vec<&str> = chunk.iter().map(|event| event.text.as_str()).collect();
            match chat(&agent, s, stage, &items, image.as_deref()) {
                Ok(results) => {
                    apply_results(&mut chunk, &results, stage);
                    Ok(0)
                }
                Err(error) => {
                    let mut first = crate::runtime::lock_recover(&first_error);
                    if first.is_none() {
                        tracing::warn!(
                            "LLM stage {} failed; original text retained: {error:#}",
                            stage.name
                        );
                        *first = Some(brief(&format!("{error:#}")));
                    }
                    Ok(chunk.len())
                }
            }
        },
    )?;
    pb.finish();
    // Remove paragraphs a replacing stage emptied, once all batches are done.
    if stage.output == Output::Replace {
        for sec in sections.iter_mut() {
            sec.speech.retain(|e| !e.text.trim().is_empty());
        }
    }
    Ok(StageReport {
        attempted,
        failed: failures.into_iter().sum(),
        note: crate::runtime::lock_recover(&first_error).take(),
        warnings: crate::runtime::lock_recover(&image_warnings).clone(),
    })
}

fn section_image_b64(path: &Path) -> Result<String> {
    use base64::Engine as _;
    Ok(base64::engine::general_purpose::STANDARD.encode(std::fs::read(path)?))
}

fn brief(msg: &str) -> String {
    const MAX: usize = 160;
    let mut s: String = msg.chars().take(MAX).collect();
    if msg.chars().count() > MAX {
        s.push('…');
    }
    s
}

/// Whether the result ids are exactly 0..expected, with none missing, repeated or out of range.
fn segment_ids_match(polished: &[(usize, String)], expected: usize) -> bool {
    let ids: std::collections::HashSet<usize> = polished.iter().map(|(id, _)| *id).collect();
    polished.len() == expected && ids.len() == expected && (0..expected).all(|id| ids.contains(&id))
}

fn apply_results(chunk: &mut [&mut TranscriptEvent], results: &[(usize, String)], stage: &Stage) {
    for (id, text) in results {
        // chat validates every id before applying a batch.
        let event = &mut chunk[*id];
        match stage.output {
            Output::Replace if *text != event.text => {
                event.raw.get_or_insert_with(|| event.text.clone());
                event.text = text.clone();
            }
            Output::Below if !text.trim().is_empty() && text.trim() != event.text.trim() => {
                event.below.push(StageText {
                    stage: stage.name.clone(),
                    text: text.clone(),
                });
            }
            _ => {}
        }
    }
}

fn chat(
    agent: &ureq::Agent,
    settings: &LlmSettings,
    stage: &Stage,
    items: &[&str],
    image: Option<&str>,
) -> Result<Vec<(usize, String)>> {
    let body = build_chat_body(settings, stage, items, image)?;
    let content = send_chat(agent, settings, &body, &stage.name, &|content| {
        let parsed = parse_segments(content).context("Invalid paragraphs response")?;
        anyhow::ensure!(
            segment_ids_match(&parsed, items.len()),
            "Result segments do not match the input"
        );
        Ok(())
    })?;
    parse_segments(&content).context("Invalid paragraphs response")
}
fn build_chat_body(
    settings: &LlmSettings,
    stage: &Stage,
    items: &[&str],
    image: Option<&str>,
) -> Result<Value> {
    let payload: Vec<Value> = items
        .iter()
        .enumerate()
        .map(|(id, text)| serde_json::json!({"id":id,"text":text}))
        .collect();
    let image = image.filter(|_| stage.images);
    let mut system = format!("{} {PARAGRAPHS_CONTRACT}", stage.prompt);
    if image.is_some() {
        system.push_str(IMAGE_NOTE);
    }
    let mut content =
        vec![serde_json::json!({"type":"text","text":serde_json::to_string(&payload)?})];
    if let Some(image) = image {
        content.push(serde_json::json!({"type":"image_url","image_url":{"url":format!("data:image/jpeg;base64,{image}")}}));
    }
    chat_body(settings, &system, Value::Array(content), stage.input)
}

/// Parse one segments object or top-level array, allowing fences and trailing commas.
pub fn parse_segments(content: &str) -> Option<Vec<(usize, String)>> {
    let content = content.trim();
    let content = if content.starts_with("```") {
        content.split_once('\n')?.1.strip_suffix("```")?.trim()
    } else {
        content
    };
    let value: Value = serde_json::from_str(content)
        .or_else(|_| serde_json::from_str(&clean_trailing_commas(content)))
        .ok()?;
    let items = match &value {
        Value::Array(items) => items,
        Value::Object(object) => object.get("segments")?.as_array()?,
        _ => return None,
    };
    parse_items(items)
}

fn parse_items(v: &[serde_json::Value]) -> Option<Vec<(usize, String)>> {
    let mut out = vec![];
    for item in v {
        let id = item.get("id")?.as_u64()? as usize;
        let text = item.get("text")?.as_str()?.to_owned();
        out.push((id, text));
    }
    if out.is_empty() { None } else { Some(out) }
}

pub(crate) fn clean_trailing_commas(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
            out.push(ch);
        } else if ch == ',' && matches!(chars.clone().find(|c| !c.is_whitespace()), Some('}' | ']'))
        {
            // Repair syntax only; commas inside transcript strings are data.
        } else {
            out.push(ch);
        }
    }
    out
}

/// Sends a chat/completions request and returns message.content.
/// Only responses whose content passes `validate` enter the success cache.
///
/// Some OpenAI-compatible endpoints reject `response_format: json_object`;
/// only when the service explicitly rejects it (400/422) is the request retried once without it.
pub(crate) fn send_chat(
    agent: &ureq::Agent,
    s: &LlmSettings,
    body: &serde_json::Value,
    stage: &str,
    validate: &dyn Fn(&str) -> Result<()>,
) -> Result<String> {
    let resp = match request_chat(agent, s, body, validate) {
        Ok(r) => r,
        Err(first) => {
            let degradable = first
                .downcast_ref::<crate::dispatch::Failure>()
                .is_some_and(|failure| failure.unsupported_response_format);
            if !(degradable && body.get("response_format").is_some()) {
                return Err(first);
            }
            let mut relaxed = body.clone();
            if let Some(object) = relaxed.as_object_mut() {
                object.remove("response_format");
            }
            // Try the relaxed request once; the original was already retried MAX_ATTEMPTS times.
            let response = request_chat_once(agent, s, &relaxed, validate)?;
            crate::dispatch::cache_validated_response(&endpoint(&s.base_url), body, &response)?;
            tracing::debug!("{}", "Retry succeeded without response_format");
            response
        }
    };
    // Report token usage as it accumulates (prompt and completion).
    let usage = &resp["usage"];
    if resp[crate::dispatch::CACHED] != true {
        crate::progress::note_tokens(
            stage,
            usage["prompt_tokens"].as_u64().unwrap_or(0),
            usage["completion_tokens"].as_u64().unwrap_or(0),
        );
    }
    // The response cache validates content before reuse.
    Ok(resp["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string())
}

/// One attempt; validation errors omit provider response bodies which may echo credentials.
fn request_chat_once(
    agent: &ureq::Agent,
    s: &LlmSettings,
    body: &serde_json::Value,
    validate: &dyn Fn(&str) -> Result<()>,
) -> std::result::Result<serde_json::Value, crate::dispatch::Failure> {
    let url = endpoint(&s.base_url);
    crate::dispatch::cached_json_request(
        &url,
        body,
        || {
            let mut request = agent.post(&url).set("Content-Type", "application/json");
            if !s.api_key.is_empty() {
                request = request.set("Authorization", &format!("Bearer {}", s.api_key));
            }
            crate::dispatch::receive(request.send_json(body))
        },
        |value| {
            // A gateway may return HTTP 200 with an error object. Missing content is a
            // protocol failure, never an empty success.
            anyhow::ensure!(
                value.get("error").is_none_or(serde_json::Value::is_null),
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

/// Sends the request, retrying retryable errors with cancellable backoff, at most [`MAX_ATTEMPTS`] times.
fn request_chat(
    agent: &ureq::Agent,
    s: &LlmSettings,
    body: &serde_json::Value,
    validate: &dyn Fn(&str) -> Result<()>,
) -> Result<serde_json::Value> {
    let mut attempt = 1;
    loop {
        match request_chat_once(agent, s, body, validate) {
            Ok(resp) => return Ok(resp),
            Err(f) if f.retryable && attempt < MAX_ATTEMPTS => {
                let wait = crate::runtime::retry_backoff(attempt);
                tracing::warn!(
                    attempt,
                    of = MAX_ATTEMPTS,
                    ?wait,
                    "AI request failed; retrying shortly"
                );
                crate::runtime::interruptible_sleep(wait)?;
                attempt += 1;
            }
            Err(f) => return Err(f.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_unusable_service_urls() {
        for base_url in ["", "not a URL", "file:///tmp/service", "https://"] {
            let settings = LlmSettings {
                base_url: base_url.into(),
                model: "test-model".into(),
                stages: vec![Stage::builtin("proofread").unwrap()],
                ..Default::default()
            };
            assert!(validate(&settings).is_err(), "accepted {base_url}");
        }
        let settings = LlmSettings {
            base_url: "http://localhost:8080/v1".into(),
            model: "test-model".into(),
            ..Default::default()
        };
        assert!(validate(&settings).is_ok());
    }

    #[test]
    fn valid_json_string_content_keeps_formatting_and_escaped_formulas() {
        let text = "First\n\tIndented $\\frac{1}{2}$";
        let response = serde_json::json!({"segments":[{"id":0,"text":text}]}).to_string();
        assert_eq!(parse_segments(&response).unwrap(), vec![(0, text.into())]);
    }

    #[test]
    fn trailing_comma_repair_preserves_transcript_literals() {
        let input = r#"{"segments":[{"id":0,"text":"literal ,} and ,] and \"quoted\"", }, ] }"#;
        let parsed = parse_segments(input).unwrap();
        assert_eq!(parsed[0].1, "literal ,} and ,] and \"quoted\"");
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
            stages: vec![Stage::builtin("proofread").unwrap()],
            ..Default::default()
        }
    }

    #[test]
    fn polish_records_raw_text_and_mismatched_ids_are_rejected() {
        let event = |text: &str| TranscriptEvent {
            start: 0.0,
            end: 1.0,
            text: text.into(),
            raw: None,
            below: Vec::new(),
        };
        let (mut a, mut b) = (event("今天讲编译原理"), event("啊"));
        let mut refs = vec![&mut a, &mut b];
        let polished = [(0, "今天讲编译原理".into()), (1, String::new())];
        assert!(segment_ids_match(&polished, refs.len()));
        apply_results(&mut refs, &polished, &Stage::builtin("proofread").unwrap());
        assert_eq!(a.raw, None, "unchanged text keeps no raw copy");
        assert_eq!(b.text, "", "filler is emptied and removed by the caller");
        assert_eq!(b.raw.as_deref(), Some("啊"), "the original is kept in raw");
        assert!(!segment_ids_match(&[(0, "x".into()), (1, "y".into())], 1));
        assert!(!segment_ids_match(&[], 1));
        assert!(!segment_ids_match(&[(0, "x".into()), (0, "y".into())], 2));
    }

    #[test]
    fn parse_pairs_tolerates_trailing_commas() {
        // Trailing commas, common in reasoning models' output
        let got =
            parse_segments("[{\"id\":0,\"text\":\"a\",},{\"id\":1,\"text\":\"b\",},]").unwrap();
        assert_eq!(got, vec![(0, "a".into()), (1, "b".into())]);
    }

    #[test]
    fn chat_body_text_vs_vision() {
        let mut s = test_settings();
        let items = ["hello"];
        let text_only = build_chat_body(&s, &s.stages[0], &items, None).unwrap();
        assert_eq!(text_only["response_format"]["type"], "json_object");
        assert_eq!(text_only["model"], "m");
        assert!(text_only.get("max_tokens").is_none());
        assert!(text_only.get("temperature").is_none());
        let user: &Vec<serde_json::Value> = text_only["messages"][1]["content"].as_array().unwrap();
        assert_eq!(user.len(), 1);
        assert_eq!(user[0]["type"], "text");

        let disabled = build_chat_body(&s, &s.stages[0], &items, Some("aGVsbG8=")).unwrap();
        assert_eq!(disabled, text_only);
        s.stages[0].images = true;
        let vision = build_chat_body(&s, &s.stages[0], &items, Some("aGVsbG8=")).unwrap();
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
            format!("{PROOFREAD_PROMPT} {PARAGRAPHS_CONTRACT}{IMAGE_NOTE}")
        );
        assert!(
            sys.contains("\"segments\""),
            "the system prompt and response_format both ask for the segments object"
        );
    }

    #[test]
    fn builtins_and_custom_stages_have_their_declared_contracts() {
        let settings: LlmSettings = serde_json::from_value(
            serde_json::json!({"stages":[{"name":"proofread"},{"name":"summary"}]}),
        )
        .unwrap();
        assert_eq!(settings.stages[0], Stage::builtin("proofread").unwrap());
        assert_eq!(settings.stages[1], Stage::builtin("summary").unwrap());
        assert_eq!(
            (settings.stages[0].input, settings.stages[0].output),
            (Input::Paragraphs, Output::Replace)
        );
        assert_eq!(
            (settings.stages[1].input, settings.stages[1].output),
            (Input::Transcript, Output::Top)
        );
        let mut custom: Stage = serde_json::from_value(serde_json::json!({"name":"annotate","input":"paragraphs","output":"below","prompt":"CUSTOM"})).unwrap();
        let body = build_chat_body(&settings, &custom, &["Text"], None).unwrap();
        assert_eq!(
            body["messages"][0]["content"],
            format!("CUSTOM {PARAGRAPHS_CONTRACT}")
        );
        assert!(
            !body["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("LaTeX")
        );
        for field in ["input", "output", "prompt"] {
            let mut fields = serde_json::to_value(&custom).unwrap();
            fields.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<Stage>(fields).is_err());
        }
        custom.images = true;
        for name in ["proofread", "annotate"] {
            custom.name = name.into();
            let body = build_chat_body(&settings, &custom, &["Text"], Some("image")).unwrap();
            assert_eq!(
                body["messages"][0]["content"],
                format!("CUSTOM {PARAGRAPHS_CONTRACT}{IMAGE_NOTE}")
            );
            let body = build_chat_body(&settings, &custom, &["Text"], None).unwrap();
            assert_eq!(
                body["messages"][0]["content"],
                format!("CUSTOM {PARAGRAPHS_CONTRACT}")
            );
        }
        for (input, output, images) in [
            ("paragraphs", "top", false),
            ("transcript", "below", false),
            ("transcript", "top", true),
        ] {
            assert!(serde_json::from_value::<Stage>(serde_json::json!({"name":"custom","input":input,"output":output,"prompt":"CUSTOM","images":images})).is_err());
        }
    }

    #[test]
    fn duplicate_stage_names_are_rejected_at_load_and_runtime_validation() {
        let config = "[llm]\n[[llm.stages]]\nname = 'summary'\n[[llm.stages]]\nname = 'summary'\n";
        let error = toml::from_str::<crate::settings::ConfigFile>(config).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Duplicate LLM stage name: summary")
        );
        let mut settings = test_settings();
        settings.stages.push(settings.stages[0].clone());
        assert!(
            validate(&settings)
                .unwrap_err()
                .to_string()
                .contains("Duplicate LLM stage name: proofread")
        );
    }

    #[test]
    fn stage_overrides_merge_requests_and_protected_keys_are_rejected_at_load() {
        let settings = toml::from_str::<crate::settings::ConfigFile>(
            r#"
[llm]
base_url = "invalid unused default"
api_key = "default-key"
model = "default-model"
request = { temperature = 0.2, max_tokens = 99 }
[[llm.stages]]
name = "summary"
base_url = "https://example.test/v1"
api_key = "stage-key"
model = "stage-model"
request = { temperature = 0.7, top_p = 0.8 }
"#,
        )
        .unwrap()
        .llm;
        validate(&settings).unwrap();
        let effective = settings.for_stage(&settings.stages[0]).unwrap();
        assert_eq!(effective.base_url, "https://example.test/v1");
        assert_eq!(effective.api_key, "stage-key");
        assert_eq!(effective.model, "stage-model");
        let body = chat_body(&effective, "System", "User", Input::Transcript).unwrap();
        assert_eq!(body["temperature"], 0.7);
        assert_eq!(body["max_tokens"], 99);
        assert_eq!(body["top_p"], 0.8);
        assert!(body.get("response_format").is_none());
        for key in ["model", "messages", "stream", "response_format"] {
            let request = serde_json::json!({key:"forbidden"});
            assert!(
                serde_json::from_value::<LlmSettings>(serde_json::json!({"request":request}))
                    .is_err()
            );
            assert!(
                serde_json::from_value::<Stage>(
                    serde_json::json!({"name":"summary","request":request})
                )
                .is_err()
            );
        }
        for request in [
            serde_json::json!([]),
            serde_json::json!("text"),
            serde_json::json!(1),
        ] {
            assert!(
                serde_json::from_value::<LlmSettings>(serde_json::json!({"request":request}))
                    .is_err()
            );
            assert!(
                serde_json::from_value::<Stage>(
                    serde_json::json!({"name":"summary","request":request})
                )
                .is_err()
            );
        }
        let unused: LlmSettings =
            serde_json::from_value(serde_json::json!({"base_url":"invalid","model":""})).unwrap();
        validate(&unused).unwrap();
    }

    #[test]
    fn below_outputs_skip_empty_or_equal_text_and_keep_stage_order() {
        let mut stage: Stage = serde_json::from_value(serde_json::json!({"name":"notes","input":"paragraphs","output":"below","prompt":"Annotate"})).unwrap();
        let mut events = ["First", "Second", "Third"].map(|text| TranscriptEvent {
            start: 0.,
            end: 1.,
            text: text.into(),
            raw: None,
            below: Vec::new(),
        });
        let mut refs = events.iter_mut().collect::<Vec<_>>();
        apply_results(
            &mut refs,
            &[
                (0, " First ".into()),
                (1, " ".into()),
                (2, " Different ".into()),
            ],
            &stage,
        );
        stage.name = "more".into();
        apply_results(
            &mut refs,
            &[(0, "".into()), (1, "Second".into()), (2, "Another".into())],
            &stage,
        );
        assert!(events[0].below.is_empty() && events[1].below.is_empty());
        assert_eq!(events[2].text, "Third");
        assert!(events[2].raw.is_none());
        assert_eq!(
            events[2].below,
            vec![
                StageText {
                    stage: "notes".into(),
                    text: " Different ".into()
                },
                StageText {
                    stage: "more".into(),
                    text: "Another".into()
                }
            ]
        );
    }

    #[test]
    fn parse_segments_object_contract() {
        // The contract: a top-level {"segments":[...]} object
        let got =
            parse_segments("{\"segments\":[{\"id\":0,\"text\":\"a\"},{\"id\":1,\"text\":\"b\"}]}")
                .unwrap();
        assert_eq!(got, vec![(0, "a".into()), (1, "b".into())]);
        // Code fences and trailing commas are tolerated
        let got =
            parse_segments("```json\n{\"segments\":[{\"id\":0,\"text\":\"a\"},]}\n```").unwrap();
        assert_eq!(got, vec![(0, "a".into())]);
        // Models that ignore the instruction and return a top-level array
        let got = parse_segments("[{\"id\":0,\"text\":\"a\"}]").unwrap();
        assert_eq!(got, vec![(0, "a".into())]);
        assert!(parse_segments("{\"segments\":[]}").is_none());
        assert!(parse_segments("no JSON here").is_none());
        for content in [
            r#"{"segments":[{"id":0,"text":"a"},{"id":"oops"},{"id":2,"text":"c"}]}"#,
            r#"[{"id":0,"text":"a"},{"id":"oops"},{"id":2,"text":"c"}]"#,
        ] {
            assert!(parse_segments(content).is_none());
        }
    }

    #[test]
    fn response_parsing_rejects_other_shapes_and_surrounding_values() {
        for content in [
            r#"{"segments":null,"other":[{"id":0,"text":"wrong array"}]}"#,
            r#"{"other":[{"id":0,"text":"wrong array"}]}"#,
            r#"{"segments":[],"other":[{"id":0,"text":"wrong array"}]}"#,
            r#"{"segments":{"nested":[{"id":0,"text":"wrong array"}]}}"#,
            r#"[{"id":0,"text":"a"}] [{"id":0,"text":"b"}]"#,
            r#"Prose [{"id":0,"text":"a"}]"#,
            r#""[{\"id\":0,\"text\":\"a\"}]""#,
            "null",
        ] {
            assert!(parse_segments(content).is_none(), "accepted {content}");
        }
    }

    #[test]
    fn top_level_arrays_tolerate_fences() {
        let got =
            parse_segments("```json\n[{\"id\":0,\"text\":\"a\"},{\"id\":1,\"text\":\"b\"}]\n```")
                .unwrap();
        assert_eq!(got, vec![(0, "a".into()), (1, "b".into())]);
        assert!(parse_segments("no array here").is_none());
        assert!(parse_segments("[]").is_none());
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
            let items: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
            let segments = items.as_array().unwrap().iter().map(|item| json!({"id":item["id"],"text":format!("Processed {}",item["text"].as_str().unwrap())})).collect::<Vec<_>>();
            Some((
                200,
                json!({"choices":[{"message":{"content":json!({"segments":segments}).to_string()}}]}),
            ))
        });
        let root = tempfile::tempdir().unwrap();
        let mut sections = ["", "missing.jpg"]
            .into_iter()
            .enumerate()
            .map(|(index, image)| crate::timeline::Section {
                t: index as f64,
                end: index as f64 + 1.,
                image: image.into(),
                speech: (0..21)
                    .map(|number| crate::timeline::TranscriptEvent {
                        start: index as f64,
                        end: index as f64 + 1.,
                        text: format!("Original {number}."),
                        raw: None,
                        below: Vec::new(),
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        let settings = crate::llm::LlmSettings {
            base_url: mock.url.clone(),
            model: "fixture".into(),
            ..Default::default()
        };
        let mut stage = crate::llm::Stage::builtin("proofread").unwrap();
        stage.name = "annotate".into();
        stage.images = true;
        let report = crate::llm::paragraphs(&mut sections, root.path(), &settings, &stage).unwrap();
        assert_eq!((report.attempted, report.failed), (42, 0));
        assert!(report.note.is_none());
        assert_eq!(
            report.warnings.len(),
            1,
            "one warning per section, even across batches"
        );
        assert!(
            report.warnings[0].contains("annotate")
                && report.warnings[0].contains("section 2")
                && report.warnings[0].contains("missing.jpg")
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
                let items: Value = serde_json::from_str(
                    body["messages"][1]["content"][0]["text"].as_str().unwrap(),
                )
                .unwrap();
                items.as_array().unwrap().len()
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
        assert_eq!(mock.calls.load(std::sync::atomic::Ordering::SeqCst), 4);
    }
}
