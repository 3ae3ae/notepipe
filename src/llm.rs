//! LLM 字幕校对（可选，默认关闭）。
//!
//! 配置文件：`~/.config/course2md/config.toml`（XDG；Windows 为 `%APPDATA%\course2md\config.toml`）。
//! 支持 OpenAI 兼容 /chat/completions 端点；所有配置项均可被命令行覆盖。
//! 关闭时每次任务结束打印开启提示（可用配置项或 `--no-llm-hint` 关闭）。
//!
//! 视觉校对（`vision = true`）：按 Section 分批，每个请求附该节幻灯片截图，
//! 供模型校正技术词汇拼写；请求失败时该批保留原文，不改发纯文本请求。

use crate::timeline::{Section, TranscriptEvent};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::IsTerminal;
use std::path::Path;
use std::time::Duration;

pub const DEFAULT_PROMPT: &str = "你是视频逐字稿校对器。输入的每一项是一段已按自然停顿组织的连续讲解。\
修正明显的语音识别错误（错别字、同音字、专有名词拼写），删除不影响原意的冗余口头填充，\
并修复不自然的断句和标点，使文字自然、书面化。不得概括、扩写、翻译、增删实质内容或改变原意；\
保持原语言。若某条内容仅由语气词、口头禅或无实义片段构成（如单独的\"啊\"、\"对吧\"），\
该条的 text 返回空字符串 \"\"（系统会删除该条）；有实质内容的条目不得删除。\
输出与输入逐条对应的 JSON 对象 {\"segments\":[{\"id\":序号,\"text\":\"校对后的文本\"}]}，不要输出任何其他内容。";

/// 每次请求合并的语音段数。
const BATCH: usize = 20;

/// LLM 服务方言/登录方式。typed enum 取代散落的字符串比较。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LlmProvider {
    /// OpenAI 兼容 /chat/completions 端点
    #[default]
    OpenAiCompatible,
    /// 本地 Ollama 服务（OpenAI 兼容方言，无需密钥）
    Ollama,
    /// OpenAI Codex 订阅登录（ChatGPT 后端 Responses API）
    Codex,
}

impl LlmProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "openai-compatible",
            Self::Ollama => "ollama",
            Self::Codex => "codex",
        }
    }
}

impl std::fmt::Display for LlmProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LlmSettings {
    pub enabled: bool,
    /// 服务方言/登录方式（默认 OpenAI 兼容）
    pub provider: LlmProvider,
    /// OpenAI 兼容 base URL，如 https://api.deepseek.com/v1
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// 自定义校对指令（输出格式约束由系统自动追加，prompt 无法覆盖）
    pub prompt: Option<String>,
    /// 关闭「可开启 LLM」的结束提示
    pub disable_hint: bool,
    /// 视觉校对：每个请求附对应幻灯片截图，辅助纠正技术词汇（模型须支持图片输入）
    pub vision: bool,
    /// 独立生成视频摘要并写入笔记（不依赖校对 enabled）
    pub summarize: bool,
    /// 校对并发数（chunk 间相互独立；自建网关/代理可调高）
    pub concurrency: usize,
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: LlmProvider::default(),
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            prompt: None,
            disable_hint: false,
            vision: false,
            summarize: false,
            concurrency: DEFAULT_CONCURRENCY,
        }
    }
}

/// 请求用途：请求账本分类与 token 用量归属。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    Proofreading,
    Summary,
}

impl Purpose {
    fn ledger_name(self) -> &'static str {
        match self {
            Self::Proofreading => "proofreading",
            Self::Summary => "summary",
        }
    }
    fn usage_stage(self) -> &'static str {
        match self {
            Self::Proofreading => "llm",
            Self::Summary => "summary",
        }
    }
}

/// base_url -> 完整 chat/completions URL。
pub fn endpoint(base_url: &str) -> String {
    crate::config::append_endpoint_suffix(base_url.trim(), "/chat/completions")
}

/// 校验配置可直接使用。
pub fn validate(s: &LlmSettings) -> Result<()> {
    if s.provider != LlmProvider::Codex && s.base_url.trim().is_empty() {
        bail!("未设置 AI 服务地址 / AI service URL is not set. Run: course2md llm setup");
    }
    if s.model.trim().is_empty() {
        bail!("未设置 AI 模型 / AI model is not set. Run: course2md llm setup");
    }
    if !s.base_url.trim().is_empty() {
        crate::config::ensure_http_url(&s.base_url)?;
    }
    Ok(())
}

/// LLM 校对默认并发数（chunk 间相互独立；可经 [llm] concurrency 调整）。
const DEFAULT_CONCURRENCY: usize = 8;
/// 校对并发上限：再高对端点限流没有好处，只放大 429 风险
const MAX_CONCURRENCY: usize = 16;
/// LLM 请求最大尝试次数（1 次原始 + 重试）。
const MAX_ATTEMPTS: u32 = 3;

/// 校对/摘要共享的 HTTP agent：整个任务复用同一连接池（clone 共享底层池）。
pub(crate) fn chat_agent() -> ureq::Agent {
    crate::runtime::http_agent(Duration::from_secs(300))
}

/// chat/completions 公共参数：温度固定 0（校对/摘要都要求确定性输出）。
pub(crate) const CHAT_TEMPERATURE: f64 = 0.0;
/// 单次请求输出 token 上限（校对与摘要共用）。
pub(crate) const CHAT_MAX_TOKENS: u32 = 16384;

/// 构造标准 chat/completions 请求体（temperature=0、json_object 结构化输出）。
/// 校对与摘要共用，避免两处各自拼 body 参数漂移；
/// `user` 传 &str 为纯文本消息，传 `serde_json::Value::Array` 为多模态内容块。
pub(crate) fn chat_body(
    model: &str,
    system: &str,
    user: impl Into<serde_json::Value>,
    max_tokens: u32,
) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "temperature": CHAT_TEMPERATURE,
        "max_tokens": max_tokens,
        "response_format": {"type": "json_object"},
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user.into()},
        ]
    })
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct PolishReport {
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    /// 首个失败原因（截断），用于结果消息可见性；无失败或纯跳过时为 None
    #[serde(default)]
    pub note: Option<String>,
}

/// 校对 worker 池骨架（非 vision / vision 两条路径原先各复制一份，此处置于一处）：
/// `Mutex` 队列 + `std::thread::scope` workers 抢占式取活，每轮先 `check_control`，
/// 中止（或锁中毒）记 `aborted` 并由首个错误胜出；队列取走即计进度，
/// `work` 返回本批成功条数（0 = 该批被跳过/失败，与成功语义一致地不累加）。
/// Err = 任务被中止，携带首个 check_control 错误。
fn run_pool<T: Send>(
    queue: &std::sync::Mutex<impl Iterator<Item = T> + Send>,
    workers: usize,
    succeeded: &std::sync::atomic::AtomicUsize,
    pb: &crate::progress::Bar,
    work: &(dyn Fn(T) -> usize + Sync),
) -> Result<()> {
    let aborted = std::sync::Mutex::new(None::<anyhow::Error>);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    if let Err(e) = crate::dispatch::check_control() {
                        let mut guard = crate::runtime::lock_recover(&aborted);
                        if guard.is_none() {
                            *guard = Some(e);
                        }
                        break;
                    }
                    let next = queue.lock().map(|mut it| it.next());
                    match next {
                        Ok(Some(item)) => {
                            // 从队列取走即计入进度：被跳过的批次也不再卡住进度条
                            pb.inc(1);
                            let count = work(item);
                            succeeded.fetch_add(count, std::sync::atomic::Ordering::Relaxed);
                        }
                        Ok(None) | Err(_) => break,
                    }
                }
            });
        }
    });
    if let Some(e) = crate::runtime::lock_recover(&aborted).take() {
        return Err(e);
    }
    Ok(())
}

/// 对已合并的 Section 做校对（在 merge 之后调用；配置已由调用方校验）。
/// - 失败批次保留原文（校对失败不阻断转换），不拆分、不改发另一种请求
/// - vision=true 时附对应截图（每节只读盘 + base64 一次）；截图不可用的节保留原文
/// - 模型对纯语气词条目返回空 text → 该条被删除
/// - 队列单元为 chunk：纯文本跨 Section 展平后每 BATCH 条一批；vision 按节分批
///   （同一请求只能附一张截图）。worker 池抢占式取活，已确认响应由请求账本复用。
pub fn polish_sections_report(
    sections: &mut [Section],
    frames_root: &Path,
    s: &LlmSettings,
) -> Result<PolishReport> {
    let attempted = sections.iter().map(|section| section.speech.len()).sum();
    if attempted == 0 {
        return Ok(PolishReport::default());
    }
    let warned = std::sync::atomic::AtomicBool::new(false);
    // 逐节读图并检查任务控制文件：大课程读图期间暂停/取消也能及时生效。
    // None = 该节截图不可用，整节保留原文。
    let mut images: Vec<Option<Option<String>>> = Vec::with_capacity(sections.len());
    for sec in sections.iter() {
        crate::dispatch::check_control()?;
        images.push(if s.vision && !sec.speech.is_empty() {
            section_image_b64(frames_root, sec, &warned).map(Some)
        } else {
            Some(None)
        });
    }
    let mut chunks: Vec<(Option<&str>, Vec<&mut TranscriptEvent>)> = Vec::new();
    if s.vision {
        for (sec, image) in sections.iter_mut().zip(&images) {
            let Some(image) = image else { continue };
            let mut refs: Vec<_> = sec.speech.iter_mut().collect();
            while !refs.is_empty() {
                let rest = refs.split_off(refs.len().min(BATCH));
                chunks.push((image.as_deref(), std::mem::replace(&mut refs, rest)));
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
    let pb = crate::progress::Bar::new("llm", chunks.len() as u64)
        .with_template("{spinner:.green} llm {pos}/{len} [{bar:32.cyan/blue}] {msg}");
    let agent = chat_agent();
    let succeeded = std::sync::atomic::AtomicUsize::new(0);
    let first_error = std::sync::Mutex::new(None::<String>);
    let queue = std::sync::Mutex::new(chunks.into_iter());
    run_pool(
        &queue,
        s.concurrency.clamp(1, MAX_CONCURRENCY),
        &succeeded,
        &pb,
        &|(image, mut chunk)| polish_chunk(&agent, s, &mut chunk, image, &warned, &first_error),
    )?;
    pb.finish();
    // 删除「成功校对为空串」的纯语气词条目（全部 chunk 完成后统一过滤）
    for sec in sections.iter_mut() {
        sec.speech.retain(|e| !e.text.trim().is_empty());
    }
    let succeeded = succeeded.load(std::sync::atomic::Ordering::Relaxed);
    Ok(PolishReport {
        attempted,
        succeeded,
        failed: attempted.saturating_sub(succeeded),
        note: crate::runtime::lock_recover(&first_error).take(),
    })
}

/// 读取本节校对截图的 base64；None = 截图不可用（已告警）。
fn section_image_b64(
    frames_root: &Path,
    sec: &Section,
    warned: &std::sync::atomic::AtomicBool,
) -> Option<String> {
    use base64::Engine as _;
    let p = frames_root.join(&sec.image);
    match std::fs::read(&p) {
        Ok(bytes) => Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
        Err(e) => {
            warn_once(
                warned,
                &format!(
                    "无法读取校对所需截图，已保留原文 / Required image could not be read; original text retained: {}: {e:#}",
                    p.display()
                ),
            );
            None
        }
    }
}

/// 校对一个分块；返回成功条数（失败保留原文并记录首个错误）。
fn polish_chunk(
    agent: &ureq::Agent,
    s: &LlmSettings,
    chunk: &mut [&mut TranscriptEvent],
    image_b64: Option<&str>,
    warned: &std::sync::atomic::AtomicBool,
    first_error: &std::sync::Mutex<Option<String>>,
) -> usize {
    let (Some(first), Some(last)) = (chunk.first(), chunk.last()) else {
        return 0;
    };
    let description = format!(
        "校对 {0}–{1} 的文字 / Proofread transcript {0}–{1}",
        crate::render::fmt_ts(first.start),
        crate::render::fmt_ts(last.end)
    );
    let items: Vec<&str> = chunk.iter().map(|e| e.text.as_str()).collect();
    match chat(agent, s, &items, image_b64, &description) {
        Ok(polished) => {
            apply_polish(chunk, &polished);
            chunk.len()
        }
        Err(error) => {
            warn_once(
                warned,
                &format!(
                    "校对未完成，已保留原文 / Proofreading incomplete; original text retained: {error:#}"
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

/// 把 (id, 新文本) 应用到一批事件上；空字符串 = 删除该条（由调用方 retain）。
/// 调用前 id 集已由请求账本校验为恰好覆盖本批（[`segment_ids_match`]）。
fn apply_polish(chunk: &mut [&mut TranscriptEvent], polished: &[(usize, String)]) {
    for (id, new) in polished {
        let Some(ev) = chunk.get_mut(*id) else {
            continue;
        };
        if *new != ev.text {
            ev.raw.get_or_insert_with(|| ev.text.clone());
            ev.text = new.clone();
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

/// 发一批（id, 文本）给 LLM，返回校对后的 (id, 文本) 列表。
/// `image_b64` 提供时在用户消息中附上该幻灯片截图（OpenAI 兼容 image_url 协议）。
/// 失败由持久请求账本记录；此层保留完整原文。
fn chat(
    agent: &ureq::Agent,
    s: &LlmSettings,
    items: &[&str],
    image_b64: Option<&str>,
    description: &str,
) -> Result<Vec<(usize, String)>> {
    let body = build_chat_body(s, items, image_b64)?;
    let expected = items.len();
    let content = send_chat_described(
        agent,
        s,
        &body,
        Purpose::Proofreading,
        description,
        &|content| {
            let parsed = parse_segments(content)
                .context("服务返回的校对结构无效 / Invalid proofreading structure")?;
            anyhow::ensure!(
                segment_ids_match(&parsed, expected),
                "校对结果与原文段落不对应，已保留原文 / Proofread segments do not match the input"
            );
            Ok(())
        },
    )?;
    parse_segments(&content).context("服务返回的校对结构无效 / Invalid proofreading structure")
}

/// 构造 /chat/completions 请求体（独立出来便于单测覆盖视觉路径）。
fn build_chat_body(
    s: &LlmSettings,
    items: &[&str],
    image_b64: Option<&str>,
) -> Result<serde_json::Value> {
    let payload: Vec<serde_json::Value> = items
        .iter()
        .enumerate()
        .map(|(i, t)| serde_json::json!({"id": i, "text": t}))
        .collect();
    let vision_note = if image_b64.is_some() {
        " 消息附带该段对应的课件截图，仅用于校正术语拼写与专有名词，不要描述或评论图片本身。"
    } else {
        ""
    };
    let system = format!(
        "{} 输出为 JSON 对象 {{\"segments\":[{{\"id\":序号,\"text\":校对后的文本}}]}}，id 必须与输入一一对应；纯语气词条目的 text 为空字符串。{vision_note}",
        effective_prompt(s)
    );
    let mut content = vec![serde_json::json!({
        "type": "text",
        "text": serde_json::to_string(&payload)?,
    })];
    if let Some(b64) = image_b64 {
        content.push(serde_json::json!({
            "type": "image_url",
            "image_url": {"url": format!("data:image/jpeg;base64,{b64}")},
        }));
    }
    Ok(crate::provider::chat_body(
        s,
        &system,
        serde_json::Value::Array(content),
        CHAT_MAX_TOKENS,
    ))
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
        let text = item.get("text")?.as_str()?.to_string();
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
                v.get("text")
                    .and_then(|x| x.as_str())
                    .map(|t| t.to_string()),
            )
        {
            out.push((id, text));
        }
        i = end;
    }
    if out.is_empty() { None } else { Some(out) }
}

/// 发 chat/completions 请求并返回 message.content（校对与摘要共用）。
/// `validate` 校验正文结构，在请求账本里执行：无效响应记为失败，不会被当成已完成复用。
///
/// 兼容性降级：部分 OpenAI 兼容端点不支持 `response_format: json_object`，
/// 仅当服务明确拒绝这个字段（400/422）时去掉该字段重试一次。
pub(crate) fn send_chat_described(
    agent: &ureq::Agent,
    s: &LlmSettings,
    body: &serde_json::Value,
    purpose: Purpose,
    description: &str,
    validate: &dyn Fn(&str) -> Result<()>,
) -> Result<String> {
    let resp = match request_chat(agent, s, body, purpose, description, validate) {
        Ok(r) => r,
        Err(first) => {
            let degradable = first
                .downcast_ref::<crate::dispatch::Failure>()
                .is_some_and(|failure| failure.unsupported_response_format);
            if !(degradable && crate::provider::has_structured_format(s, body)) {
                return Err(first);
            }
            let mut relaxed = body.clone();
            crate::provider::strip_structured_format(s, &mut relaxed);
            // 降级请求只试一次：原请求已按 MAX_ATTEMPTS 重试过，这里只验证兼容性
            let response = request_chat_once(agent, s, &relaxed, purpose, description, validate)?;
            tracing::debug!("端点不支持 response_format，降级重试成功");
            response
        }
    };
    // token 用量实时反馈：GUI 阶段行据此跳动（上传=prompt，下载=completion）。
    // Codex SSE 已在 provider::sse_to_chat_json 归一成 chat/completions 同一形状。
    let usage = &resp["usage"];
    crate::progress::note_tokens(
        purpose.usage_stage(),
        usage["prompt_tokens"].as_u64().unwrap_or(0),
        usage["completion_tokens"].as_u64().unwrap_or(0),
    );
    // 账本已校验正文存在且结构有效
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
    purpose: Purpose,
    description: &str,
    validate: &dyn Fn(&str) -> Result<()>,
) -> std::result::Result<serde_json::Value, crate::dispatch::Failure> {
    let url = crate::provider::endpoint(s);
    let headers = crate::provider::auth_headers(s).map_err(crate::dispatch::Failure::local)?;
    let sse = crate::provider::is_sse(s);
    crate::dispatch::json_request_described(
        "llm",
        purpose.ledger_name(),
        description,
        &url,
        body,
        || {
            let mut request = agent.post(&url).set("Content-Type", "application/json");
            for (name, value) in &headers {
                request = request.set(name, value);
            }
            let response = crate::dispatch::receive(request.send_json(body))?;
            // Codex Responses API 只提供 SSE 流：聚合为 chat/completions 形状，
            // 下游校验与解析不变；非 2xx 原样上交（重试/降级逻辑要看状态码）
            if sse && (200..300).contains(&response.status) {
                let canonical = crate::provider::sse_to_chat_json(&response.body).map_err(|e| {
                    crate::dispatch::NetworkFailure {
                        message: format!("{e:#}"),
                        definitely_unsent: false,
                    }
                })?;
                return Ok(crate::dispatch::HttpResponse {
                    status: response.status,
                    body: serde_json::to_vec(&canonical).unwrap_or_default(),
                });
            }
            Ok(response)
        },
        |value| {
            // A gateway may return HTTP 200 with an error object. Missing content is a
            // protocol failure, never an empty success.
            anyhow::ensure!(
                value.get("error").is_none_or(serde_json::Value::is_null),
                "AI 服务返回了错误 / The AI service returned an error"
            );
            let content = value["choices"][0]["message"]["content"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .context("AI 服务没有返回内容 / The AI service returned no content")?;
            validate(content)
        },
    )
}

/// 发请求：可重试错误按退避重试（退避期间可取消），总共最多 [`MAX_ATTEMPTS`] 次。
fn request_chat(
    agent: &ureq::Agent,
    s: &LlmSettings,
    body: &serde_json::Value,
    purpose: Purpose,
    description: &str,
    validate: &dyn Fn(&str) -> Result<()>,
) -> Result<serde_json::Value> {
    let mut attempt = 1;
    loop {
        match request_chat_once(agent, s, body, purpose, description, validate) {
            Ok(resp) => return Ok(resp),
            Err(f) if f.retryable && attempt < MAX_ATTEMPTS => {
                let wait = crate::runtime::retry_backoff(attempt);
                tracing::warn!(
                    attempt,
                    of = MAX_ATTEMPTS,
                    ?wait,
                    "AI 请求失败，稍后重试 / AI request failed; retrying shortly"
                );
                crate::runtime::interruptible_sleep(wait)?;
                attempt += 1;
            }
            Err(f) => return Err(f.into()),
        }
    }
}

/// 用户自定义校对指令；空白视为未设置，回落到内置提示词。
/// 注意：输出格式约束（{"segments":[...]} 契约 / id 对应）由系统在构造
/// 请求体时自动追加，自定义 prompt 无法覆盖（见 build_chat_body）。
fn effective_prompt(s: &LlmSettings) -> &str {
    s.prompt
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or(DEFAULT_PROMPT)
}

/// 1×1 PNG 测试图：vision=true 时连接测试附带，
/// 避免「文本请求通了但实际图片请求不可用」的假阳性。
const TEST_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

/// 用最小请求验证端点与凭据可用；vision=true 时附带测试图片，
/// 同时验证图片输入路径（失败会明确报出，不会显示为已验证可用）。
pub fn test_connection(s: &LlmSettings) -> Result<()> {
    validate(s)?;
    let user: serde_json::Value = if s.vision {
        serde_json::json!([
            {"type": "text", "text": "只回复两个字符：ok"},
            {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{TEST_PNG_B64}")}},
        ])
    } else {
        serde_json::Value::String("只回复两个字符：ok".into())
    };
    let body = crate::provider::test_body(s, user);
    let fail_hint = if s.vision {
        "连接失败。请确认服务地址、密钥及模型，并检查图片输入支持。 / Connection failed. Check the URL, API key, model, and image input support."
    } else {
        "连接失败。请检查服务地址、密钥和模型。 / Connection failed. Check the URL, API key, and model."
    };
    // 与生产路径同一纪律：禁止跟随重定向；空密钥不发送 Bearer 头（本地无鉴权服务）
    let agent = crate::runtime::http_agent(Duration::from_secs(60));
    let mut request = agent.post(&crate::provider::endpoint(s));
    for (name, value) in crate::provider::auth_headers(s)? {
        request = request.set(&name, &value);
    }
    let resp = request.send_json(body).context(fail_hint)?;
    let v: serde_json::Value = if crate::provider::is_sse(s) {
        let mut bytes = Vec::new();
        use std::io::Read as _;
        resp.into_reader()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .context("服务响应接收中断 / Service response was interrupted")?;
        crate::provider::sse_to_chat_json(&bytes)?
    } else {
        resp.into_json()
            .context("无法解析响应 / Cannot parse response")?
    };
    let text = v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim();
    anyhow::ensure!(
        !text.is_empty(),
        "服务返回空响应；请确认模型可用 / The service returned an empty response; make sure the model works"
    );
    Ok(())
}

/// `llm setup`：交互式补齐缺失项并写盘。
/// 使用 dialoguer（console 行编辑）：支持左右箭头/Home/End 移动、
/// 退格/删除等标准编辑键（裸 read_line 无法处理方向键转义序列）。
pub fn setup_interactive(
    mut cfg: crate::settings::ConfigFile,
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    disable_hint: bool,
) -> Result<crate::settings::ConfigFile> {
    let interactive = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let cur_or = |v: String, cur: &str| {
        if v.trim().is_empty() {
            cur.to_string()
        } else {
            v.trim().to_string()
        }
    };
    if let Some(v) = base_url {
        cfg.llm.base_url = v.trim().to_string();
    } else if interactive {
        let v: String = dialoguer::Input::new()
            .with_prompt(
                "服务地址 / Base URL (OpenAI-compatible, e.g. https://api.deepseek.com/v1)",
            )
            .with_initial_text(&cfg.llm.base_url)
            .allow_empty(true)
            .interact_text()?;
        cfg.llm.base_url = cur_or(v, &cfg.llm.base_url);
    }
    if let Some(v) = api_key {
        cfg.llm.api_key = v;
    } else if interactive {
        let keep_hint = if cfg.llm.api_key.is_empty() {
            "（输入隐藏；无密钥的本地服务可留空） / API key (hidden; leave blank for a local service without authentication)"
        } else {
            "（输入隐藏；回车保留已保存的密钥） / API key (hidden; Enter keeps the saved key)"
        };
        let v = dialoguer::Password::new()
            .with_prompt(format!("API Key{keep_hint}"))
            .allow_empty_password(true)
            .interact()?;
        cfg.llm.api_key = cur_or(v, &cfg.llm.api_key);
    }
    if let Some(v) = model {
        cfg.llm.model = v.trim().to_string();
    } else if interactive {
        let v: String = dialoguer::Input::new()
            .with_prompt("模型名 / Model name (e.g. deepseek-chat)")
            .with_initial_text(&cfg.llm.model)
            .allow_empty(true)
            .interact_text()?;
        cfg.llm.model = cur_or(v, &cfg.llm.model);
    }
    anyhow::ensure!(
        !cfg.llm.base_url.trim().is_empty() && !cfg.llm.model.trim().is_empty(),
        "需要填写服务地址和模型名。在终端运行 course2md llm setup，或传入 --base-url 和 --model / A base URL and model are required. Run course2md llm setup in a terminal, or pass --base-url and --model."
    );
    // 容错：没写 scheme 时补 https://
    if !cfg.llm.base_url.is_empty() && !cfg.llm.base_url.contains("://") {
        cfg.llm.base_url = format!("https://{}", cfg.llm.base_url.trim());
    }
    // 视觉能力仅交互式终端询问（脚本化调用全部传参时不阻塞）
    if interactive {
        cfg.llm.vision = dialoguer::Select::new()
            .with_prompt("校对时附上幻灯片截图？需支持图片的模型。 / Attach slide images when polishing? Requires an image-capable model.")
            .items([
                "仅发送文字 / Send text only",
                "发送文字和截图 / Send text and slide images",
            ])
            .default(if cfg.llm.vision { 1 } else { 0 })
            .interact_opt()?
            .ok_or_else(|| anyhow::anyhow!("已取消设置，未保存配置。 / Setup cancelled; no configuration saved."))? == 1;
    }
    // setup 面向 OpenAI 兼容端点；codex/ollama 登录态由此显式退出
    cfg.llm.provider = LlmProvider::OpenAiCompatible;
    validate(&cfg.llm)?;
    cfg.llm.disable_hint |= disable_hint;
    cfg.llm.enabled = true;
    Ok(cfg)
}

pub fn print_status(cfg: &crate::settings::ConfigFile) {
    let s = &cfg.llm;
    let state = |enabled| {
        if enabled {
            "开启 / enabled"
        } else {
            "关闭 / disabled"
        }
    };
    println!(
        "配置文件 / Configuration: {}",
        crate::settings::config_path().display()
    );
    println!("  LLM 校对 / Transcript polish: {}", state(s.enabled));
    println!("  服务类型 / Provider: {}", s.provider);
    println!(
        "  服务地址 / Base URL: {}",
        if s.base_url.is_empty() {
            "—"
        } else {
            &s.base_url
        }
    );
    println!(
        "  API key: {}",
        if s.api_key.is_empty() {
            "未设置 / not set"
        } else {
            "已设置（隐藏） / set (hidden)"
        }
    );
    println!(
        "  模型 / Model: {}",
        if s.model.is_empty() { "—" } else { &s.model }
    );
    println!(
        "  截图辅助校对 / Slide-assisted polish: {}",
        state(s.vision)
    );
    println!("  自动摘要 / Automatic summary: {}", state(s.summarize));
    println!("  并发请求 / Concurrent requests: {}", s.concurrency);
    println!("  使用提示 / Usage hint: {}", state(!s.disable_hint));
    if !s.enabled {
        println!("开启校对 / Enable transcript polish: course2md llm setup");
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
            enabled: true,
            provider: LlmProvider::OpenAiCompatible,
            base_url: "https://api.x.com/v1".into(),
            api_key: "k".into(),
            model: "m".into(),
            prompt: None,
            disable_hint: false,
            vision: false,
            summarize: false,
            concurrency: 8,
        }
    }

    #[test]
    fn polish_records_raw_text_and_mismatched_ids_are_rejected() {
        let event = |text: &str| TranscriptEvent {
            start: 0.0,
            end: 1.0,
            text: text.into(),
            raw: None,
        };
        let (mut a, mut b) = (event("今天讲编译原理"), event("啊"));
        let mut refs = vec![&mut a, &mut b];
        let polished = [(0, "今天讲编译原理".into()), (1, String::new())];
        assert!(segment_ids_match(&polished, refs.len()));
        apply_polish(&mut refs, &polished);
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
        let text_only = build_chat_body(&s, &items, None).unwrap();
        let user: &Vec<serde_json::Value> = text_only["messages"][1]["content"].as_array().unwrap();
        assert_eq!(user.len(), 1);
        assert_eq!(user[0]["type"], "text");

        let vision = build_chat_body(&s, &items, Some("aGVsbG8=")).unwrap();
        let user: &Vec<serde_json::Value> = vision["messages"][1]["content"].as_array().unwrap();
        assert_eq!(user.len(), 2, "带图时附 image_url 内容块");
        assert_eq!(user[1]["type"], "image_url");
        assert_eq!(
            user[1]["image_url"]["url"].as_str().unwrap(),
            "data:image/jpeg;base64,aGVsbG8="
        );
        let sys = vision["messages"][0]["content"].as_str().unwrap();
        assert!(sys.contains("课件截图"), "带图时系统提示说明截图用途");
        assert!(
            sys.contains("\"segments\""),
            "系统提示与 response_format=json_object 同为 segments 对象契约"
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
