//! Prompt-driven paragraph and transcript stages over OpenAI-compatible HTTP.
use crate::timeline::{Section, StageText, TranscriptEvent};
use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::{path::Path, time::Duration};

pub(crate) const PROOFREAD_PROMPT: &str = r#"You proofread transcripts of spoken recordings. Each input item is a stretch of continuous speech, already split at natural pauses. Fix obvious speech-recognition errors (misheard words, homophones, misspelled names and terms), remove filler that adds nothing, and repair awkward sentence breaks and punctuation so the text reads naturally. Do not summarize, expand, translate, add or remove substantive content, or change the meaning. Keep the original language. If an item contains only filler or meaningless fragments (such as a lone "um" or "you know"), return an empty text for it; never empty an item that has real content. Write mathematical formulas, symbols and expressions as LaTeX: wrap inline math in $...$ and put display math on its own line in $$...$$; rewrite spoken formulas, such as "x squared", as LaTeX too; do not add $ when there is no math."#;
pub(crate) const PROOFREAD_CONTRACT: &str = r#"Output a JSON object {"segments":[{"id":number,"text":proofread text}]} with exactly one entry per input id; an item that contains only filler gets an empty text. Inside JSON strings, escape every backslash as \\."#;
pub(crate) const PROOFREAD_VISION_NOTE: &str = r#" Each request includes the screenshot shown during this passage. Use it only to correct the spelling of terms and proper names; do not describe or comment on the image."#;
pub(crate) const PARAGRAPHS_CONTRACT: &str = r#"Output a JSON object {"segments":[{"id":number,"text":result}]} with exactly one entry per input id. Inside JSON strings, escape every backslash as \\."#;

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

/// 校对/摘要共享的 HTTP agent：整个任务复用同一连接池（clone 共享底层池）。
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

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct StageReport {
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    /// 首个失败原因（截断），用于结果消息可见性；无失败或纯跳过时为 None
    #[serde(default)]
    pub note: Option<String>,
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
    let warned = std::sync::atomic::AtomicBool::new(false);
    // Paths only: the image is read and encoded once per request batch, not all up front.
    let mut image_paths: Vec<Option<std::path::PathBuf>> = Vec::with_capacity(sections.len());
    for sec in sections.iter() {
        crate::runtime::check_cancelled()?;
        image_paths
            .push((stage.images && !sec.speech.is_empty()).then(|| frames_root.join(&sec.image)));
    }
    let mut chunks: Vec<(Option<std::path::PathBuf>, Vec<&mut TranscriptEvent>)> = Vec::new();
    if stage.images {
        for (sec, image) in sections.iter_mut().zip(&image_paths) {
            let Some(image) = image else { continue };
            let mut refs: Vec<_> = sec.speech.iter_mut().collect();
            while !refs.is_empty() {
                let rest = refs.split_off(refs.len().min(BATCH));
                chunks.push((Some(image.clone()), std::mem::replace(&mut refs, rest)));
            }
        }
    } else {
        let mut refs: Vec<_> = sections
            .iter_mut()
            .flat_map(|sec| sec.speech.iter_mut())
            .collect();
        while !refs.is_empty() {
            let rest = refs.split_off(refs.len().min(BATCH));
            chunks.push((None, std::mem::replace(&mut refs, rest)));
        }
    }
    let pb = crate::progress::Bar::new(&stage.name, chunks.len() as u64)
        .with_template("{spinner:.green} llm {pos}/{len} [{bar:32.cyan/blue}] {msg}");
    let agent = chat_agent();
    let first_error = std::sync::Mutex::new(None::<String>);
    let completed = crate::runtime::map_pool(
        chunks,
        s.concurrency.clamp(1, MAX_CONCURRENCY),
        |(image_path, mut chunk)| {
            pb.inc(1);
            let image = image_path
                .as_deref()
                .and_then(|path| section_image_b64(path, &warned));
            Ok(paragraph_chunk(
                &agent,
                s,
                stage,
                &mut chunk,
                image.as_deref(),
                &warned,
                &first_error,
            ))
        },
    )?;
    pb.finish();
    // 删除「成功校对为空串」的纯语气词条目（全部 chunk 完成后统一过滤）
    if stage.output == Output::Replace {
        for sec in sections.iter_mut() {
            sec.speech.retain(|e| !e.text.trim().is_empty());
        }
    }
    let succeeded = completed.into_iter().sum();
    Ok(StageReport {
        attempted,
        succeeded,
        failed: attempted.saturating_sub(succeeded),
        note: crate::runtime::lock_recover(&first_error).take(),
    })
}

/// 读取本节校对截图的 base64；None = 截图不可用（已告警）。
fn section_image_b64(path: &Path, warned: &std::sync::atomic::AtomicBool) -> Option<String> {
    use base64::Engine as _;
    let p = path;
    match std::fs::read(p) {
        Ok(bytes) => Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
        Err(e) => {
            warn_once(
                warned,
                &format!(
                    "Required image could not be read; original text retained: {}: {e:#}",
                    p.display()
                ),
            );
            None
        }
    }
}

/// 校对一个分块；返回成功条数（失败保留原文并记录首个错误）。
fn paragraph_chunk(
    agent: &ureq::Agent,
    s: &LlmSettings,
    stage: &Stage,
    chunk: &mut [&mut TranscriptEvent],
    image_b64: Option<&str>,
    warned: &std::sync::atomic::AtomicBool,
    first_error: &std::sync::Mutex<Option<String>>,
) -> usize {
    if chunk.is_empty() {
        return 0;
    }
    let items: Vec<&str> = chunk.iter().map(|e| e.text.as_str()).collect();
    match chat(agent, s, stage, &items, image_b64) {
        Ok(polished) => {
            apply_results(chunk, &polished, stage);
            chunk.len()
        }
        Err(error) => {
            warn_once(
                warned,
                &format!(
                    "LLM stage {} failed; original text retained: {error:#}",
                    stage.name
                ),
            );
            record_first_error(first_error, format!("{error:#}"));
            0
        }
    }
}

/// 记录首个失败原因（后续失败不覆盖），截断避免超长错误刷屏。
fn record_first_error(slot: &std::sync::Mutex<Option<String>>, msg: String) {
    let mut guard = crate::runtime::lock_recover(slot);
    if guard.is_none() {
        *guard = Some(brief(&msg));
    }
}

fn brief(msg: &str) -> String {
    const MAX: usize = 160;
    let mut s: String = msg.chars().take(MAX).collect();
    if msg.chars().count() > MAX {
        s.push('…');
    }
    s
}

/// 校对结果的 id 集恰好覆盖 0..expected（无缺失/重复/越界）。
fn segment_ids_match(polished: &[(usize, String)], expected: usize) -> bool {
    let ids: std::collections::HashSet<usize> = polished.iter().map(|(id, _)| *id).collect();
    polished.len() == expected && ids.len() == expected && (0..expected).all(|id| ids.contains(&id))
}

fn apply_results(chunk: &mut [&mut TranscriptEvent], results: &[(usize, String)], stage: &Stage) {
    for (id, text) in results {
        let Some(event) = chunk.get_mut(*id) else {
            continue;
        };
        match stage.output {
            Output::Replace if *text != event.text => {
                event.raw.get_or_insert_with(|| event.text.clone());
                event.text = text.clone();
            }
            Output::Below if !text.trim().is_empty() && text.trim() != event.text.trim() => {
                event.below.push(StageText {
                    stage: stage.name.clone(),
                    text: text.trim().into(),
                });
            }
            _ => {}
        }
    }
}

fn warn_once(warned: &std::sync::atomic::AtomicBool, msg: &str) {
    use std::sync::atomic::Ordering;
    if !warned.swap(true, Ordering::Relaxed) {
        tracing::warn!("{msg}");
    } else {
        tracing::debug!("{msg}");
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
    let contract = if stage.name == "proofread" {
        PROOFREAD_CONTRACT
    } else {
        PARAGRAPHS_CONTRACT
    };
    let mut system = format!("{} {contract}", stage.prompt);
    if image.is_some() && stage.name == "proofread" {
        system.push_str(PROOFREAD_VISION_NOTE);
    }
    let mut content =
        vec![serde_json::json!({"type":"text","text":serde_json::to_string(&payload)?})];
    if let Some(image) = image {
        content.push(serde_json::json!({"type":"image_url","image_url":{"url":format!("data:image/jpeg;base64,{image}")}}));
    }
    chat_body(settings, &system, Value::Array(content), stage.input)
}

/// 从模型输出提取校对结果。约定契约为顶层对象 {"segments":[{"id":n,"text":"..."}]}
/// （与 response_format=json_object 一致）；兼容模型无视指令仍返回
/// 顶层数组的情况。两者都容忍代码围栏、前后杂文与尾逗号。
pub fn parse_segments(content: &str) -> Option<Vec<(usize, String)>> {
    // 1) 契约路径：{"segments":[...]}
    if let Some(obj) = extract_json_object(content)
        && let Some(arr) = obj.get("segments").and_then(|v| v.as_array())
        && let Some(items) = parse_items(arr)
    {
        return Some(items);
    }
    // 2) 兼容路径：顶层数组（含个别坏项的宽容扫描）
    parse_id_text_pairs(content)
}

/// 截取首个 { 到末个 } 解析为 JSON 对象；容忍尾逗号。
fn extract_json_object(content: &str) -> Option<serde_json::Value> {
    let start = content.find('{')?;
    let end = content.rfind('}')?;
    if end <= start {
        return None;
    }
    let slice = &content[start..=end];
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(slice) {
        return Some(v);
    }
    let cleaned = clean_trailing_commas(slice);
    if cleaned != slice
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&cleaned)
    {
        return Some(v);
    }
    None
}

/// 从模型输出提取 [{"id":n,"text":"..."}]（容忍代码围栏、前后杂文、尾逗号与个别坏项）。
pub fn parse_id_text_pairs(content: &str) -> Option<Vec<(usize, String)>> {
    let start = content.find('[')?;
    let end = content.rfind(']')?;
    if end <= start {
        return None;
    }
    let slice = &content[start..=end];
    // 1) 严格解析；个别项 id/text 类型不对时 parse_items 返回 None，
    //    必须继续降级而不是整批丢弃（落入第 2/3 级）
    if let Ok(v) = serde_json::from_str::<Vec<serde_json::Value>>(slice)
        && let Some(items) = parse_items(&v)
    {
        return Some(items);
    }
    // 2) 清除尾逗号后重试
    let cleaned = clean_trailing_commas(slice);
    if cleaned != slice
        && let Ok(v) = serde_json::from_str::<Vec<serde_json::Value>>(&cleaned)
        && let Some(items) = parse_items(&v)
    {
        return Some(items);
    }
    // 3) 宽容扫描：跳过坏项，收集合法 {"id":..,"text":".."}
    lenient_scan(slice)
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

/// 逐个扫描顶层 {...} 对象，坏项跳过；能取到至少一项即返回。
fn lenient_scan(s: &str) -> Option<Vec<(usize, String)>> {
    let bytes = s.as_bytes();
    let mut out: Vec<(usize, String)> = vec![];
    let mut i = 0usize;
    while i < s.len() {
        let Some(rel) = s[i..].find('{') else { break };
        let obj_start = i + rel;
        // 找配对的 }（跳过字符串字面量内的花括号）
        let mut depth = 0usize;
        let mut in_str = false;
        let mut esc = false;
        let mut end = None;
        for (k, &b) in bytes[obj_start..].iter().enumerate() {
            if in_str {
                if esc {
                    esc = false;
                } else if b == b'\\' {
                    esc = true;
                } else if b == b'"' {
                    in_str = false;
                }
                continue;
            }
            match b {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(obj_start + k + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { break };
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s[obj_start..end])
            && let (Some(id), Some(text)) = (
                v.get("id").and_then(|x| x.as_u64()).map(|x| x as usize),
                v.get("text").and_then(|x| x.as_str()).map(str::to_owned),
            )
        {
            out.push((id, text));
        }
        i = end;
    }
    if out.is_empty() { None } else { Some(out) }
}

/// 发 chat/completions 请求并返回 message.content（校对与摘要共用）。
/// Only responses whose content passes `validate` enter the success cache.
///
/// 兼容性降级：部分 OpenAI 兼容端点不支持 `response_format: json_object`，
/// 仅当服务明确拒绝这个字段（400/422）时去掉该字段重试一次。
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
            // 降级请求只试一次：原请求已按 MAX_ATTEMPTS 重试过，这里只验证兼容性
            let response = request_chat_once(agent, s, &relaxed, validate)?;
            crate::dispatch::cache_validated_response(&endpoint(&s.base_url), body, &response)?;
            tracing::debug!("{}", "Retry succeeded without response_format");
            response
        }
    };
    // token 用量实时反馈：GUI 阶段行据此跳动（上传=prompt，下载=completion）。
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

/// 发请求：可重试错误按退避重试（退避期间可取消），总共最多 [`MAX_ATTEMPTS`] 次。
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
        assert_eq!(a.raw, None, "未改动的条目不记 raw");
        assert_eq!(b.text, "", "纯语气词被置空，随后由调用方删除");
        assert_eq!(b.raw.as_deref(), Some("啊"), "原文进 raw 溯源");
        assert!(!segment_ids_match(&[(0, "x".into()), (1, "y".into())], 1));
        assert!(!segment_ids_match(&[], 1));
        assert!(!segment_ids_match(&[(0, "x".into()), (0, "y".into())], 2));
    }

    #[test]
    fn parse_pairs_tolerates_trailing_commas_and_bad_items() {
        // 尾逗号（推理模型常见输出）
        let got = parse_id_text_pairs("[{\"id\":0,\"text\":\"a\",},{\"id\":1,\"text\":\"b\",},]")
            .unwrap();
        assert_eq!(got, vec![(0, "a".into()), (1, "b".into())]);
        // 个别坏项：跳过而不丢弃整批
        let got = parse_id_text_pairs(
            "[{\"id\":0,\"text\":\"a\"},{\"id\":\"oops\"},{\"id\":2,\"text\":\"c\"}]",
        )
        .unwrap();
        assert_eq!(got, vec![(0, "a".into()), (2, "c".into())], "坏项应被跳过");
    }

    #[test]
    fn chat_body_text_vs_vision() {
        let s = test_settings();
        let items = ["hello"];
        let text_only = build_chat_body(&s, &s.stages[0], &items, None).unwrap();
        assert_eq!(text_only["response_format"]["type"], "json_object");
        assert_eq!(text_only["model"], "m");
        assert!(text_only.get("max_tokens").is_none());
        assert!(text_only.get("temperature").is_none());
        let user: &Vec<serde_json::Value> = text_only["messages"][1]["content"].as_array().unwrap();
        assert_eq!(user.len(), 1);
        assert_eq!(user[0]["type"], "text");

        let vision = build_chat_body(&s, &s.stages[0], &items, Some("aGVsbG8=")).unwrap();
        let user: &Vec<serde_json::Value> = vision["messages"][1]["content"].as_array().unwrap();
        assert_eq!(user.len(), 2, "带图时附 image_url 内容块");
        assert_eq!(user[1]["type"], "image_url");
        assert_eq!(
            user[1]["image_url"]["url"].as_str().unwrap(),
            "data:image/jpeg;base64,aGVsbG8="
        );
        let sys = vision["messages"][0]["content"].as_str().unwrap();
        assert_eq!(
            sys,
            format!("{PROOFREAD_PROMPT} {PROOFREAD_CONTRACT}{PROOFREAD_VISION_NOTE}")
        );
        assert!(
            sys.contains("\"segments\""),
            "系统提示与 response_format=json_object 同为 segments 对象契约"
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
        let custom: Stage = serde_json::from_value(serde_json::json!({"name":"annotate","input":"paragraphs","output":"below","prompt":"CUSTOM"})).unwrap();
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
        for (input, output, images) in [
            ("paragraphs", "top", false),
            ("transcript", "below", false),
            ("transcript", "top", true),
        ] {
            assert!(serde_json::from_value::<Stage>(serde_json::json!({"name":"custom","input":input,"output":output,"prompt":"CUSTOM","images":images})).is_err());
        }
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
                    text: "Different".into()
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
        // 契约路径：顶层对象 {"segments":[...]}
        let got =
            parse_segments("{\"segments\":[{\"id\":0,\"text\":\"a\"},{\"id\":1,\"text\":\"b\"}]}")
                .unwrap();
        assert_eq!(got, vec![(0, "a".into()), (1, "b".into())]);
        // 容忍围栏 + 尾逗号
        let got =
            parse_segments("```json\n{\"segments\":[{\"id\":0,\"text\":\"a\"},]}\n```").unwrap();
        assert_eq!(got, vec![(0, "a".into())]);
        // 兼容路径：模型无视指令仍返回顶层数组
        let got = parse_segments("[{\"id\":0,\"text\":\"a\"}]").unwrap();
        assert_eq!(got, vec![(0, "a".into())]);
        assert!(parse_segments("{\"segments\":[]}").is_none());
        assert!(parse_segments("没有 JSON").is_none());
    }

    #[test]
    fn parse_id_pairs_tolerates_fences() {
        let got = parse_id_text_pairs(
            "```json\n[{\"id\":0,\"text\":\"a\"},{\"id\":1,\"text\":\"b\"}]\n```",
        )
        .unwrap();
        assert_eq!(got, vec![(0, "a".into()), (1, "b".into())]);
        assert!(parse_id_text_pairs("没有数组").is_none());
        assert!(parse_id_text_pairs("[]").is_none());
    }
}
