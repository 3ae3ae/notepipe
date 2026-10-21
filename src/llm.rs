//! LLM 字幕校对（可选，默认关闭）。
//!
//! 配置文件：`~/.config/notepipe/config.toml`（XDG；Windows 为 `%APPDATA%\notepipe\config.toml`）。
//! 支持 OpenAI 兼容 /chat/completions 端点；所有配置项均可被命令行覆盖。
//!
//! 视觉校对（`vision = true`）：按 Section 分批，每个请求附该节幻灯片截图，
//! 供模型校正技术词汇拼写；请求失败时该批保留原文，不改发纯文本请求。

use crate::timeline::{Section, TranscriptEvent};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

pub const DEFAULT_PROMPT: &str = "你是视频逐字稿校对器。输入的每一项是一段已按自然停顿组织的连续讲解。\
修正明显的语音识别错误（错别字、同音字、专有名词拼写），删除不影响原意的冗余口头填充，\
并修复不自然的断句和标点，使文字自然、书面化。不得概括、扩写、翻译、增删实质内容或改变原意；\
保持原语言。若某条内容仅由语气词、口头禅或无实义片段构成（如单独的\"啊\"、\"对吧\"），\
该条的 text 返回空字符串 \"\"（系统会删除该条）；有实质内容的条目不得删除。\
";

/// 公式书写约定：校对与摘要输出共用，系统自动追加，自定义 prompt 无法覆盖。
/// 阅读器与导出按 `$` / `$$` 识别公式并排版。
pub const MATH_RULE: &str = "数学公式、符号与表达式一律写成 LaTeX：行内公式用 $...$ 包围，\
独立成行的公式用 $$...$$ 包围（如 $f(x)=x^2$、$$\\int_a^b f(x)\\,dx$$）；\
口述的公式（如“x 的平方”“从 a 到 b 积分”）同样改写为 LaTeX；没有公式时不要添加 $。\
JSON 字符串里的反斜杠必须写成 \\\\（如 \\\\frac）。";

/// 每次请求合并的语音段数。
const BATCH: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LlmSettings {
    pub enabled: bool,
    /// OpenAI 兼容 base URL，如 https://api.deepseek.com/v1
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// 自定义校对指令（输出格式约束由系统自动追加，prompt 无法覆盖）
    pub prompt: Option<String>,
    /// 视觉校对：每个请求附对应幻灯片截图，辅助纠正技术词汇（模型须支持图片输入）
    pub vision: bool,
    /// 独立生成视频摘要并写入笔记（不依赖校对 enabled）
    pub summarize: bool,
    /// Bilingual notes: the target language code or name.
    /// Skipped when already in that language, except for Chinese script/region targets.
    pub translate: Option<String>,
    /// 校对并发数（chunk 间相互独立；自建网关/代理可调高）
    pub concurrency: usize,
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            prompt: None,
            vision: false,
            summarize: false,
            translate: None,
            concurrency: DEFAULT_CONCURRENCY,
        }
    }
}

/// 请求用途：请求账本分类与 token 用量归属。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    Proofreading,
    Summary,
    Translation,
}

impl Purpose {
    fn ledger_name(self) -> &'static str {
        match self {
            Self::Proofreading => "proofreading",
            Self::Summary => "summary",
            Self::Translation => "translation",
        }
    }
    fn usage_stage(self) -> &'static str {
        match self {
            Self::Proofreading => "llm",
            Self::Summary => "summary",
            Self::Translation => "translate",
        }
    }
}

/// Standard OpenAI-compatible chat body. Temperature is fixed at 0 because
/// proofreading and summaries require deterministic output.
pub(crate) fn chat_body(
    s: &LlmSettings,
    system: &str,
    user: impl Into<serde_json::Value>,
    max_tokens: u32,
) -> serde_json::Value {
    serde_json::json!({
        "model": s.model,
        "temperature": 0.0,
        "max_tokens": max_tokens,
        "response_format": {"type": "json_object"},
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user.into()},
        ]
    })
}

/// base_url -> 完整 chat/completions URL。
pub fn endpoint(base_url: &str) -> String {
    crate::config::append_endpoint_suffix(base_url.trim(), "/chat/completions")
}

/// 校验配置可直接使用。
pub fn validate(s: &LlmSettings) -> Result<()> {
    if let Some(target) = &s.translate {
        crate::language::target(target).map_err(anyhow::Error::msg)?;
    }
    if s.base_url.trim().is_empty() {
        bail!(crate::tr!(
            "AI service URL is not set. Set [llm].base_url or pass --llm-base-url."
        ));
    }
    if s.model.trim().is_empty() {
        bail!(crate::tr!(
            "AI model is not set. Set [llm].model or pass --llm-model."
        ));
    }
    crate::config::ensure_http_url(&s.base_url)?;
    Ok(())
}

/// LLM 校对默认并发数（chunk 间相互独立；可经 [llm] concurrency 调整）。
const DEFAULT_CONCURRENCY: usize = 8;
/// 校对并发上限：再高对端点限流没有好处，只放大 429 风险
const MAX_CONCURRENCY: usize = 16;
/// LLM 请求最大尝试次数（1 次原始 + 重试）。
const MAX_ATTEMPTS: u32 = 3;

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

/// 单次请求输出 token 上限（校对与摘要共用）。
pub(crate) const CHAT_MAX_TOKENS: u32 = 16384;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct PolishReport {
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    /// 首个失败原因（截断），用于结果消息可见性；无失败或纯跳过时为 None
    #[serde(default)]
    pub note: Option<String>,
}

/// 校对 worker 池骨架：
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
    // Paths only: the image is read and encoded once per request batch, not all up front.
    let mut image_paths: Vec<Option<std::path::PathBuf>> = Vec::with_capacity(sections.len());
    for sec in sections.iter() {
        crate::dispatch::check_control()?;
        image_paths
            .push((s.vision && !sec.speech.is_empty()).then(|| frames_root.join(&sec.image)));
    }
    let mut chunks: Vec<(Option<std::path::PathBuf>, Vec<&mut TranscriptEvent>)> = Vec::new();
    if s.vision {
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
        &|(image_path, mut chunk)| {
            let image = image_path
                .as_deref()
                .and_then(|path| section_image_b64(path, &warned));
            polish_chunk(
                &agent,
                s,
                &mut chunk,
                image.as_deref(),
                &warned,
                &first_error,
            )
        },
    )?;
    pb.finish();
    // A formula spoken across a subtitle boundary comes back in two pieces.
    for sec in sections.iter_mut() {
        join_split_formulas(&mut sec.speech);
    }
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

impl LlmSettings {
    /// Whether any requested step sends text to the AI service.
    pub fn uses_service(&self) -> bool {
        self.enabled || self.summarize || self.translation_target().is_some()
    }

    /// The bilingual target, if one is set; validated during preflight.
    pub fn translation_target(&self) -> Option<&str> {
        self.translate.as_deref().map(str::trim)
    }
}

/// Translates every paragraph into `target`, kept beside the original.
/// Failed batches stay untranslated; the note is published either way.
pub fn translate_sections_report(
    sections: &mut [Section],
    s: &LlmSettings,
    target: &str,
) -> Result<PolishReport> {
    let mut chunks: Vec<Vec<&mut TranscriptEvent>> = Vec::new();
    let mut refs: Vec<_> = sections
        .iter_mut()
        .flat_map(|section| section.speech.iter_mut())
        .filter(|event| !event.text.trim().is_empty())
        .collect();
    let attempted = refs.len();
    if attempted == 0 {
        return Ok(PolishReport::default());
    }
    while !refs.is_empty() {
        let rest = refs.split_off(refs.len().min(BATCH));
        chunks.push(std::mem::replace(&mut refs, rest));
    }
    let pb = crate::progress::Bar::new("translate", chunks.len() as u64)
        .with_template("{spinner:.green} translate {pos}/{len} [{bar:32.cyan/blue}] {msg}");
    let agent = chat_agent();
    let warned = std::sync::atomic::AtomicBool::new(false);
    let succeeded = std::sync::atomic::AtomicUsize::new(0);
    let first_error = std::sync::Mutex::new(None::<String>);
    let queue = std::sync::Mutex::new(chunks.into_iter());
    run_pool(
        &queue,
        s.concurrency.clamp(1, MAX_CONCURRENCY),
        &succeeded,
        &pb,
        &|mut chunk| translate_chunk(&agent, s, target, &mut chunk, &warned, &first_error),
    )?;
    pb.finish();
    let succeeded = succeeded.load(std::sync::atomic::Ordering::Relaxed);
    Ok(PolishReport {
        attempted,
        succeeded,
        failed: attempted.saturating_sub(succeeded),
        note: crate::runtime::lock_recover(&first_error).take(),
    })
}

fn translate_chunk(
    agent: &ureq::Agent,
    s: &LlmSettings,
    target: &str,
    chunk: &mut [&mut TranscriptEvent],
    warned: &std::sync::atomic::AtomicBool,
    first_error: &std::sync::Mutex<Option<String>>,
) -> usize {
    let (Some(first), Some(last)) = (chunk.first(), chunk.last()) else {
        return 0;
    };
    let description = crate::tr!(
        "Translate transcript {}–{}",
        crate::render::fmt_ts(first.start),
        crate::render::fmt_ts(last.end)
    );
    let items: Vec<&str> = chunk.iter().map(|e| e.text.as_str()).collect();
    let result = translation_body(s, target, &items).and_then(|body| {
        let expected = items.len();
        let content = send_chat_described(
            agent,
            s,
            &body,
            Purpose::Translation,
            &description,
            &|content| {
                let parsed =
                    parse_segments(content).context(crate::tr!("Invalid translation structure"))?;
                anyhow::ensure!(
                    segment_ids_match(&parsed, expected),
                    crate::tr!("Translated segments do not match the input")
                );
                Ok(())
            },
        )?;
        parse_segments(&content).context(crate::tr!("Invalid translation structure"))
    });
    match result {
        Ok(translated) => {
            for (id, text) in translated {
                if let Some(event) = chunk.get_mut(id) {
                    let text = text.trim();
                    event.translation = (!text.is_empty()).then(|| text.to_owned());
                }
            }
            chunk.len()
        }
        Err(error) => {
            warn_once(
                warned,
                &crate::tr!("Translation incomplete: {}", format!("{error:#}")),
            );
            record_first_error(first_error, format!("{error:#}"));
            0
        }
    }
}

fn translation_body(s: &LlmSettings, target: &str, items: &[&str]) -> Result<serde_json::Value> {
    let payload: Vec<serde_json::Value> = items
        .iter()
        .enumerate()
        .map(|(i, t)| serde_json::json!({"id": i, "text": t}))
        .collect();
    // The instruction stays in English: it names the target for any service.
    let system = format!(
        "You translate lecture transcripts. Translate each item into {}. Keep the meaning, \
         terminology and tone; do not summarize, add or omit content. Keep names, code and \
         formulas as they are. Output a JSON object {{\"segments\":[{{\"id\":number,\"text\":translation}}]}} \
         with exactly one entry per input id. {MATH_RULE}",
        crate::language::english_name(target)
    );
    Ok(chat_body(
        s,
        &system,
        serde_json::Value::String(serde_json::to_string(&payload)?),
        CHAT_MAX_TOKENS,
    ))
}

/// Joins an inline formula that a segment ends unfinished (`$0.$`, `$a+$`)
/// with the formula the next segment opens with, so `加上 $0.$` + `$2\cos 2x$`
/// reads `加上 $0.2\cos 2x$`. The next segment keeps the rest of its text.
fn join_split_formulas(events: &mut [TranscriptEvent]) {
    for index in 1..events.len() {
        let (before, after) = events.split_at_mut(index);
        let (previous, next) = (&mut before[index - 1].text, &mut after[0].text);
        let Some(head) = unfinished_tail_formula(previous) else {
            continue;
        };
        let rest = next.trim_start();
        let Some(body) = rest.strip_prefix('$').filter(|body| !body.starts_with('$')) else {
            continue;
        };
        let Some(close) = body.find('$') else {
            continue;
        };
        let joined = format!("{head}{}$", &body[..close]);
        let remainder = body[close + 1..].to_owned();
        let keep = previous.trim_end().len() - 1;
        previous.truncate(keep - (head.len() + 1));
        previous.push('$');
        previous.push_str(&joined);
        *next = remainder;
    }
}

/// The content of a closing inline formula that cannot end where it does.
fn unfinished_tail_formula(text: &str) -> Option<String> {
    let trimmed = text.trim_end().strip_suffix('$')?;
    let open = trimmed.rfind('$')?;
    let content = &trimmed[open + 1..];
    if content.is_empty() || trimmed[..open].ends_with('$') {
        return None;
    }
    let tail = content.trim_end();
    let unfinished = tail.ends_with(['+', '-', '=', '(', '[', '{', '^', '_', ',', '\\'])
        || (tail.ends_with('.') && tail[..tail.len() - 1].ends_with(|c: char| c.is_ascii_digit()));
    unfinished.then(|| content.to_owned())
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
    let description = crate::tr!(
        "Proofread transcript {}–{}",
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
                &crate::tr!(
                    "Proofreading has not been completed, the original text has been retained: {}",
                    format!("{error:#}")
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
            let parsed =
                parse_segments(content).context(crate::tr!("Invalid proofreading structure"))?;
            anyhow::ensure!(
                segment_ids_match(&parsed, expected),
                crate::tr!("Proofread segments do not match the input")
            );
            Ok(())
        },
    )?;
    parse_segments(&content).context(crate::tr!("Invalid proofreading structure"))
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
        crate::tr!(
            " 消息附带该段对应的课件截图，仅用于校正术语拼写与专有名词，不要描述或评论图片本身。"
        )
    } else {
        ""
    };
    let system = format!(
        "{} {MATH_RULE}",
        crate::tr!(
            "{} 输出为 JSON 对象 {{\"segments\":[{{\"id\":序号,\"text\":校对后的文本}}]}}，id 必须与输入一一对应；纯语气词条目的 text 为空字符串。{}",
            effective_prompt(s),
            vision_note
        )
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
    Ok(chat_body(
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

/// A model that writes `\frac` or `\times` unescaped in JSON gets a form feed
/// or tab back from the parser. Those characters never belong in a transcript,
/// so before a letter they are restored to the LaTeX command they came from;
/// a newline is restored only inside a `$` formula, where it cannot be prose.
pub(crate) fn restore_latex(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_math = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let letter_follows = chars.peek().is_some_and(char::is_ascii_alphabetic);
        let escape = match c {
            '\u{8}' => Some('b'),
            '\u{c}' => Some('f'),
            '\t' => Some('t'),
            '\r' => Some('r'),
            '\n' if in_math => Some('n'),
            _ => None,
        };
        match escape {
            Some(letter) if letter_follows => {
                out.push('\\');
                out.push(letter);
            }
            _ => {
                if c == '$' && !out.ends_with('\\') {
                    // `$$` opens or closes one display formula.
                    if chars.peek() == Some(&'$') {
                        out.push(chars.next().unwrap_or('$'));
                    }
                    in_math = !in_math;
                }
                out.push(c);
            }
        }
    }
    out
}

fn parse_items(v: &[serde_json::Value]) -> Option<Vec<(usize, String)>> {
    let mut out = vec![];
    for item in v {
        let id = item.get("id")?.as_u64()? as usize;
        let text = restore_latex(item.get("text")?.as_str()?);
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
                v.get("text").and_then(|x| x.as_str()).map(restore_latex),
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
            if !(degradable && body.get("response_format").is_some()) {
                return Err(first);
            }
            let mut relaxed = body.clone();
            if let Some(object) = relaxed.as_object_mut() {
                object.remove("response_format");
            }
            // 降级请求只试一次：原请求已按 MAX_ATTEMPTS 重试过，这里只验证兼容性
            let response = request_chat_once(agent, s, &relaxed, purpose, description, validate)?;
            tracing::debug!("{}", crate::tr!("Retry succeeded without response_format"));
            response
        }
    };
    // token 用量实时反馈：GUI 阶段行据此跳动（上传=prompt，下载=completion）。
    let usage = &resp["usage"];
    if resp[crate::dispatch::REPLAYED] != true {
        crate::progress::note_tokens(
            purpose.usage_stage(),
            usage["prompt_tokens"].as_u64().unwrap_or(0),
            usage["completion_tokens"].as_u64().unwrap_or(0),
        );
    }
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
    let url = endpoint(&s.base_url);
    crate::dispatch::json_request_described(
        "llm",
        purpose.ledger_name(),
        description,
        &url,
        body,
        crate::dispatch::Resend::AskUser,
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
                crate::tr!("The AI service returned an error")
            );
            let content = value["choices"][0]["message"]["content"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .context(crate::tr!("The AI service returned no content"))?;
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
                    "AI request failed; retrying shortly"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formulas_split_across_segments_are_joined() {
        let event = |text: &str| TranscriptEvent {
            start: 0.,
            end: 1.,
            text: text.into(),
            raw: None,
            translation: None,
        };
        let mut events = vec![
            event("加上 $0.$"),
            event("$2\\cos 2x$，等等。"),
            event("这是 $x$"),
            event("$y$ 不变"),
        ];
        join_split_formulas(&mut events);
        assert_eq!(events[0].text, "加上 $0.2\\cos 2x$");
        assert_eq!(events[1].text, "，等等。");
        // A complete formula followed by another stays two formulas.
        assert_eq!(events[2].text, "这是 $x$");
        assert_eq!(events[3].text, "$y$ 不变");
    }

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
    fn unescaped_latex_commands_survive_json_parsing() {
        // `{"text":"$\frac{a}{b}\times c$"}` written without escaping the backslashes.
        let content = "{\"segments\":[{\"id\":0,\"text\":\"面积 $\\frac{a}{b}\\times c$ 与 $$\\neq 0$$\\n下一句\"}]}";
        let parsed = parse_segments(content).unwrap();
        assert_eq!(
            parsed[0].1,
            "面积 $\\frac{a}{b}\\times c$ 与 $$\\neq 0$$\n下一句"
        );
        // Properly escaped LaTeX is left as written.
        assert_eq!(restore_latex("$\\frac{1}{2}$"), "$\\frac{1}{2}$");
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
            base_url: "https://api.x.com/v1".into(),
            api_key: "k".into(),
            model: "m".into(),
            prompt: None,
            vision: false,
            summarize: false,
            translate: None,
            concurrency: 8,
        }
    }

    #[test]
    fn translation_keeps_originals_and_adds_each_translation() {
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            // Read the head, then the body announced by Content-Length.
            let body_start = loop {
                let n = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..n]);
                if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    break at + 4;
                }
            };
            let head = String::from_utf8_lossy(&request[..body_start]).to_ascii_lowercase();
            let length: usize = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse().ok())
                .unwrap();
            while request.len() < body_start + length {
                let n = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..n]);
            }
            let content = serde_json::json!({"segments":[{"id":0,"text":"Today: compilers."},{"id":1,"text":"Next, parsing."}]});
            let body = serde_json::json!({"choices":[{"message":{"content":content.to_string()}}]})
                .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            String::from_utf8_lossy(&request[body_start..]).into_owned()
        });
        let event = |text: &str| TranscriptEvent {
            start: 0.0,
            end: 1.0,
            text: text.into(),
            raw: None,
            translation: None,
        };
        let mut sections = vec![Section {
            t: 0.0,
            end: 1.0,
            image: String::new(),
            speech: vec![event("今天讲编译原理。"), event("接下来是语法分析。")],
        }];
        let mut settings = test_settings();
        settings.base_url = format!("http://{address}/v1");
        settings.concurrency = 1;
        let report = translate_sections_report(&mut sections, &settings, "en").unwrap();
        let request = server.join().unwrap();
        assert!(request.contains("English"), "{request}");
        assert_eq!((report.attempted, report.succeeded), (2, 2));
        let speech = &sections[0].speech;
        assert_eq!(speech[0].text, "今天讲编译原理。");
        assert_eq!(speech[0].translation.as_deref(), Some("Today: compilers."));
        assert_eq!(speech[1].translation.as_deref(), Some("Next, parsing."));
    }

    #[test]
    fn polish_records_raw_text_and_mismatched_ids_are_rejected() {
        let event = |text: &str| TranscriptEvent {
            start: 0.0,
            end: 1.0,
            text: text.into(),
            raw: None,
            translation: None,
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
        assert_eq!(text_only["response_format"]["type"], "json_object");
        assert_eq!(text_only["model"], "m");
        assert_eq!(text_only["max_tokens"], CHAT_MAX_TOKENS);
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
