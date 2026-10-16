//! Explicit, single-request service contract tests. Never call this from save or generation.
//!
//! The shipped samples contain no course data. HTTP success alone is not a test pass; the
//! response must satisfy the actual feature's structure and the known sample. Redirects,
//! speculative protocol fallbacks and automatic retries are deliberately disabled.

use crate::credentials::{CredentialVault, Secret};
use crate::preferences::{
    Authentication, ServiceConfiguration, ServiceProtocol, ServiceTestEvidence, TestOutcome,
    normalize_endpoint, now_seconds,
};
use base64::Engine as _;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

const SPEECH_SAMPLE: &[u8] = include_bytes!("../assets/tests/service-speech.wav");
const BLUE_CARD: &[u8] = include_bytes!("../assets/tests/service-blue-card.png");
const MAX_RESPONSE: u64 = 128 * 1024;

pub const TEST_NOTICE: &str = "检查仅发送内置示例，服务商可能计费";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestKind {
    Speech,
    Proofread,
    Summary,
    Vision,
}

impl TestKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Speech => "语音识别",
            Self::Proofread => "AI 校对",
            Self::Summary => "生成摘要",
            Self::Vision => "截图辅助校对",
        }
    }

    pub fn contract(self) -> &'static str {
        match self {
            Self::Speech => "speech-en-blue-notebook-seven-pages-v1",
            Self::Proofread => "proofread-en-two-segment-ids-v1",
            Self::Summary => "summary-en-tldr-points-outline-v1",
            Self::Vision => "vision-en-blue-card-segment-id-v1",
        }
    }

    fn accepts(self, protocol: ServiceProtocol) -> bool {
        match self {
            Self::Speech => matches!(
                protocol,
                ServiceProtocol::SpeechTranscriptions | ServiceProtocol::SpeechChat
            ),
            // Ollama 与 AiChat 同为 chat/completions 方言；Codex 由 test_codex_blocking 覆盖
            _ => matches!(protocol, ServiceProtocol::AiChat | ServiceProtocol::OllamaChat),
        }
    }
}

/// A recorded, explicit configuration refusal can stop a repair from immediately
/// resending course data. It is not a prerequisite for saving a service or for
/// trying an untested configuration. Authentication applies to the connection;
/// model refusals remain scoped to the capabilities being repaired.
pub fn blocks_reprocessing(
    evidence: &ServiceTestEvidence,
    config: &ServiceConfiguration,
    required_contracts: &[&str],
) -> bool {
    if evidence.fingerprint != config.fingerprint(&evidence.contract) {
        return false;
    }
    // Older stored tests classified model-shaped errors before HTTP status.
    if evidence.details.iter().any(|detail| {
        detail
            .strip_prefix("HTTP ")
            .and_then(|status| status.parse::<u16>().ok())
            .is_some_and(|status| status == 429 || status >= 500)
    }) {
        return false;
    }
    match evidence.outcome {
        TestOutcome::AuthenticationRefused => true,
        TestOutcome::ModelRefused => required_contracts.contains(&evidence.contract.as_str()),
        _ => false,
    }
}

/// Suitable for a GPUI background task or any async executor. Dropping the future cannot
/// promise to retract a request already sent; keep the result receiver alive when closing
/// the editor and only attach evidence if its configuration fingerprint still matches.
pub async fn test_service(
    config: ServiceConfiguration,
    kind: TestKind,
    vault: Arc<dyn CredentialVault>,
    cancelled: Arc<AtomicBool>,
) -> ServiceTestEvidence {
    smol::unblock(move || test_service_blocking(&config, kind, vault.as_ref(), &cancelled)).await
}

pub fn test_service_blocking(
    config: &ServiceConfiguration,
    kind: TestKind,
    vault: &dyn CredentialVault,
    cancelled: &AtomicBool,
) -> ServiceTestEvidence {
    run_test(config, kind, vault, cancelled, &HttpTransport)
}

/// Codex 订阅服务的检查：复用 CLI 的 Responses/SSE 方言（course2md::llm::test_connection），
/// 桌面自带的 chat/completions 传输不适用于 Codex。证据模型与其他服务一致；
/// 校对结构契约由转换流程在运行时复核（失败保留原文）。
pub fn test_codex_blocking(
    config: &ServiceConfiguration,
    kind: TestKind,
    cancelled: &AtomicBool,
) -> ServiceTestEvidence {
    let mut evidence = ServiceTestEvidence {
        fingerprint: config.fingerprint(kind.contract()),
        contract: kind.contract().into(),
        tested_at: now_seconds(),
        outcome: TestOutcome::NotSent,
        message: String::new(),
        details: vec![
            format!("用途：{}", kind.label()),
            format!("请求模型：{}", config.model),
        ],
    };
    if cancelled.load(Ordering::Acquire) {
        evidence.message = "测试已取消，尚未发送请求".into();
        return evidence;
    }
    if kind == TestKind::Speech || config.model.trim().is_empty() {
        evidence.message = "所选服务不支持这项测试，尚未发送请求".into();
        return evidence;
    }
    let settings = course2md::llm::LlmSettings {
        enabled: true,
        provider: course2md::llm::LlmProvider::Codex,
        model: config.model.clone(),
        vision: kind == TestKind::Vision,
        ..Default::default()
    };
    match course2md::llm::test_connection(&settings) {
        Ok(()) => {
            evidence.outcome = TestOutcome::Passed;
            evidence.message = "连接测试通过：Codex 登录与模型可用".into();
        }
        Err(error) => {
            let outcome = codex_failure_outcome(&error);
            let reason = format!("{error:#}");
            evidence.message = if outcome == TestOutcome::AuthenticationRefused {
                "Codex 登录已失效或尚未连接，请重新连接账号".into()
            } else {
                format!("连接测试未通过：{}", reason.chars().take(240).collect::<String>())
            };
            evidence.outcome = outcome;
        }
    }
    evidence
}

/// Codex 检查失败的分类：登录缺失/失效（含刷新被拒）是配置拒绝，阻断补做；
/// 其余为一般性未通过。识别根 crate 的 [`course2md::login::codex::CodexLoginRequired`] 标记。
fn codex_failure_outcome(error: &anyhow::Error) -> TestOutcome {
    if error
        .chain()
        .any(|cause| cause.downcast_ref::<course2md::login::codex::CodexLoginRequired>().is_some())
    {
        TestOutcome::AuthenticationRefused
    } else {
        TestOutcome::ContractMismatch
    }
}

struct HttpRequest {
    endpoint: String,
    content_type: String,
    body: Vec<u8>,
    authorization: Option<Secret>,
}

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

enum TransportFailure {
    /// The request or response was not fully observed. No retry is performed.
    Unknown,
    ResponseTooLarge(u16),
}

trait Transport {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, TransportFailure>;
}

struct HttpTransport;

impl Transport for HttpTransport {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, TransportFailure> {
        let agent = crate::bounded_http::json_agent(
            Duration::from_secs(10),
            Duration::from_secs(30),
            Some(Duration::from_secs(15)),
            Duration::from_secs(45),
        );
        let call = agent
            .post(&request.endpoint)
            .set("Content-Type", &request.content_type);
        let call = crate::bounded_http::json_call(call, request.authorization.as_ref());
        // A nonempty POST body is non-retryable in ureq 2. Each test creates a fresh agent,
        // so a recycled connection cannot trigger a hidden resend either.
        let response = match call.send_bytes(&request.body) {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(_)) => return Err(TransportFailure::Unknown),
        };
        let status = response.status();
        let body = crate::bounded_http::read_bounded(response, MAX_RESPONSE).map_err(|error| {
            match error {
                crate::bounded_http::BoundedReadError::TooLarge => {
                    TransportFailure::ResponseTooLarge(status)
                }
                crate::bounded_http::BoundedReadError::Network => TransportFailure::Unknown,
            }
        })?;
        Ok(HttpResponse { status, body })
    }
}

fn run_test(
    config: &ServiceConfiguration,
    kind: TestKind,
    vault: &dyn CredentialVault,
    cancelled: &AtomicBool,
    transport: &dyn Transport,
) -> ServiceTestEvidence {
    let mut evidence = ServiceTestEvidence {
        fingerprint: config.fingerprint(kind.contract()),
        contract: kind.contract().into(),
        tested_at: now_seconds(),
        outcome: TestOutcome::NotSent,
        message: String::new(),
        details: vec![
            format!("用途：{}", kind.label()),
            format!("请求模型：{}", config.model),
        ],
    };
    if cancelled.load(Ordering::Acquire) {
        evidence.message = "测试已取消，尚未发送请求".into();
        return evidence;
    }
    if !kind.accepts(config.protocol) {
        evidence.message = "所选接口类型不支持这项测试，尚未发送请求".into();
        return evidence;
    }
    if normalize_endpoint(&config.endpoint, config.protocol)
        .ok()
        .as_deref()
        != Some(config.endpoint.as_str())
        || config.model.trim().is_empty()
    {
        evidence.message = "请先填写有效的服务地址和模型 ID，尚未发送请求".into();
        return evidence;
    }
    let authorization = if config.authentication == Authentication::ApiKey {
        let Some(reference) = &config.credential else {
            evidence.message = "此认证方式需要 API Key，尚未发送请求".into();
            return evidence;
        };
        match vault.resolve(reference) {
            Ok(secret) => Some(secret),
            Err(error) => {
                evidence.message = format!("{error}；尚未发送请求");
                return evidence;
            }
        }
    } else {
        None
    };
    let (content_type, body) = request_body(config, kind);
    let request = HttpRequest {
        endpoint: config.endpoint.clone(),
        content_type,
        body,
        authorization,
    };
    if cancelled.load(Ordering::Acquire) {
        evidence.message = "测试已取消，尚未发送请求".into();
        return evidence;
    }
    let response = match transport.send(&request) {
        Ok(response) => response,
        Err(TransportFailure::Unknown) => {
            evidence.outcome = TestOutcome::OutcomeUnknown;
            evidence.message = "未收到确定结果，测试可能已产生费用".into();
            return evidence;
        }
        Err(TransportFailure::ResponseTooLarge(status)) => {
            evidence.outcome = TestOutcome::ContractMismatch;
            evidence.message = "已收到回应，但测试响应超出合理大小".into();
            evidence
                .details
                .push(format!("HTTP {status}；响应超过 128 KiB，未读取其余内容"));
            return evidence;
        }
    };
    evidence.details.push(format!("HTTP {}", response.status));
    let json = serde_json::from_slice::<Value>(&response.body).ok();
    if response.status == 401 || response.status == 403 {
        evidence.outcome = TestOutcome::AuthenticationRefused;
        evidence.message = "服务拒绝凭据，请检查 API Key 或此模型的使用权限".into();
        return evidence;
    }
    if !(200..300).contains(&response.status)
        || json
            .as_ref()
            .is_some_and(|value| value.get("error").is_some())
    {
        let code = json
            .as_ref()
            .and_then(|value| {
                value
                    .pointer("/error/code")
                    .or_else(|| value.pointer("/error/type"))
            })
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let model_error = [
            "model_not_found",
            "invalid_model",
            "model_not_available",
            "unsupported_model",
            "model_not_supported",
        ]
        .contains(&code.as_str());
        evidence.outcome = if response.status >= 500 {
            TestOutcome::OutcomeUnknown
        } else if response.status == 429 {
            TestOutcome::ContractMismatch
        } else if model_error {
            TestOutcome::ModelRefused
        } else {
            TestOutcome::ContractMismatch
        };
        evidence.message = if response.status >= 500 {
            "服务返回错误，未收到确定结果，测试可能已产生费用"
        } else if response.status == 429 {
            "服务暂时无法接受测试请求，请查看服务的额度或速率限制"
        } else if model_error {
            "服务不接受所选模型，请检查模型 ID 和使用权限"
        } else if (300..400).contains(&response.status) {
            "服务要求跳转到其他地址；尚未向新地址发送请求，请核对服务地址"
        } else {
            "已收到回应，但服务未接受这项测试请求"
        }
        .into();
        return evidence;
    }
    let Some(json) = json else {
        evidence.outcome = TestOutcome::ContractMismatch;
        evidence.message = "已收到回应，但返回内容不是所选接口需要的 JSON".into();
        return evidence;
    };
    let content = if config.protocol == ServiceProtocol::SpeechTranscriptions {
        json.get("text").and_then(Value::as_str)
    } else {
        json.pointer("/choices/0/message/content")
            .and_then(Value::as_str)
    };
    let Some(content) = content.filter(|content| !content.trim().is_empty()) else {
        evidence.outcome = TestOutcome::ContractMismatch;
        evidence.message = "已收到回应，但缺少此用途需要的有效正文".into();
        return evidence;
    };
    match verify_sample(kind, content) {
        Ok(()) => {
            evidence.outcome = TestOutcome::Passed;
            evidence.message = format!("{}用途测试通过", kind.label());
        }
        Err((outcome, detail)) => {
            evidence.outcome = outcome;
            evidence.message = if evidence.outcome == TestOutcome::ContractMismatch {
                "返回格式不符合要求".into()
            } else {
                "样例未通过，请查看详情".into()
            };
            evidence.details.push(detail);
            let excerpt = if let Some(secret) = &request.authorization {
                content.replace(secret.expose(), "[已隐藏凭据]")
            } else {
                content.to_owned()
            };
            evidence.details.push(format!(
                "样例回应：{}",
                excerpt.chars().take(240).collect::<String>()
            ));
        }
    }
    evidence
}

fn request_body(config: &ServiceConfiguration, kind: TestKind) -> (String, Vec<u8>) {
    if config.protocol == ServiceProtocol::SpeechTranscriptions {
        let boundary = format!("course2md-test-{}", uuid::Uuid::new_v4());
        let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\njson\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"course2md-speech-test.wav\"\r\nContent-Type: audio/wav\r\n\r\n", config.model).into_bytes();
        body.extend_from_slice(SPEECH_SAMPLE);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        return (format!("multipart/form-data; boundary={boundary}"), body);
    }
    let (system, user) = match kind {
        TestKind::Speech => (
            "Transcribe the audio verbatim in its original language. Return only the transcript.",
            json!([
                {"type":"text", "text":"Transcribe this short audio."},
                {"type":"input_audio", "input_audio":{"data":base64::engine::general_purpose::STANDARD.encode(SPEECH_SAMPLE), "format":"wav"}}
            ]),
        ),
        TestKind::Proofread => (
            "Correct grammar and punctuation while preserving meaning and language. Return only JSON: {\"segments\":[{\"id\":0,\"text\":\"corrected text\"}]}. Preserve each input ID exactly once.",
            json!(
                "[{\"id\":0,\"text\":\"The student have three book.\"},{\"id\":1,\"text\":\"Water freezes at zero degrees Celsius.\"}]"
            ),
        ),
        TestKind::Summary => (
            "Summarize the transcript in its original language. Return only JSON with tldr (string), key_points (array of strings), and outline (array of {t: numeric seconds, title: string, detail: string}). Preserve the concrete facts.",
            json!(
                "[0s] Plants use sunlight to make food through photosynthesis. [8s] The process uses water and carbon dioxide and releases oxygen."
            ),
        ),
        TestKind::Vision => (
            "Correct the transcript using the attached image. Preserve the input ID. Return only JSON: {\"segments\":[{\"id\":0,\"text\":\"corrected text\"}]}.",
            json!([
                {"type":"text", "text":"[{\"id\":0,\"text\":\"The card shown is red.\"}]"},
                {"type":"image_url", "image_url":{"url":format!("data:image/png;base64,{}",base64::engine::general_purpose::STANDARD.encode(BLUE_CARD))}}
            ]),
        ),
    };
    let mut body = json!({
        "model":config.model, "temperature":0, "max_tokens":512,
        "messages":[{"role":"system","content":system},{"role":"user","content":user}]
    });
    if kind != TestKind::Speech {
        body["response_format"] = json!({"type":"json_object"});
    }
    (
        "application/json".into(),
        serde_json::to_vec(&body).expect("sample is serializable"),
    )
}

fn words(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn verify_sample(kind: TestKind, content: &str) -> Result<(), (TestOutcome, String)> {
    let mismatch = |message: &str| (TestOutcome::SampleMismatch, message.to_owned());
    let contract = |message: &str| (TestOutcome::ContractMismatch, message.to_owned());
    match kind {
        TestKind::Speech => {
            let text = words(content).replace("7", "seven");
            if !["blue", "notebook", "seven", "pages"]
                .iter()
                .all(|word| text.split_whitespace().any(|actual| actual == *word))
            {
                return Err(mismatch(
                    "内置音频：The blue notebook contains seven pages.；回应未保留其中的关键信息",
                ));
            }
        }
        TestKind::Proofread | TestKind::Vision => {
            let pairs = course2md::llm::parse_segments(content)
                .ok_or_else(|| contract("需要 segments 列表，每项有 id 与 text"))?;
            let expected: &[usize] = if kind == TestKind::Proofread {
                &[0, 1]
            } else {
                &[0]
            };
            let mut ids = pairs.iter().map(|(id, _)| *id).collect::<Vec<_>>();
            ids.sort_unstable();
            if ids != expected || pairs.iter().any(|(_, text)| text.trim().is_empty()) {
                return Err(contract(
                    "返回段落 ID 必须与内置输入一一对应，且每段包含有效文字",
                ));
            }
            let first = words(&pairs.iter().find(|(id, _)| *id == 0).unwrap().1);
            if kind == TestKind::Vision {
                if !first.split_whitespace().any(|word| word == "blue")
                    || first.split_whitespace().any(|word| word == "red")
                {
                    return Err(mismatch(
                        "内置图片为蓝色卡片，校对后应将原文的 red 改为 blue",
                    ));
                }
            } else {
                let second = words(&pairs.iter().find(|(id, _)| *id == 1).unwrap().1);
                if !(first.contains("has three books") || first.contains("has 3 books"))
                    || !second.contains("water")
                    || !second.contains("celsius")
                {
                    return Err(mismatch(
                        "第一段应纠正 have / book 的语法；第二段应保留水在零摄氏度结冰的原意",
                    ));
                }
            }
        }
        TestKind::Summary => {
            let start = content
                .find('{')
                .ok_or_else(|| contract("需要摘要 JSON 对象"))?;
            let end = content
                .rfind('}')
                .ok_or_else(|| contract("需要摘要 JSON 对象"))?;
            let value: Value = serde_json::from_str(&content[start..=end])
                .map_err(|_| contract("摘要 JSON 无法解析"))?;
            let tldr = value
                .get("tldr")
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty());
            let points = value
                .get("key_points")
                .and_then(Value::as_array)
                .filter(|points| {
                    !points.is_empty()
                        && points
                            .iter()
                            .all(|point| point.as_str().is_some_and(|text| !text.trim().is_empty()))
                });
            let outline = value
                .get("outline")
                .and_then(Value::as_array)
                .filter(|items| {
                    !items.is_empty()
                        && items.iter().all(|item| {
                            item.get("t")
                                .is_some_and(|time| time.is_number() || time.is_string())
                                && item
                                    .get("title")
                                    .and_then(Value::as_str)
                                    .is_some_and(|text| !text.trim().is_empty())
                                && item
                                    .get("detail")
                                    .and_then(Value::as_str)
                                    .is_some_and(|text| !text.trim().is_empty())
                        })
                });
            if tldr.is_none() || points.is_none() || outline.is_none() {
                return Err(contract("需要有效的 tldr、key_points 和带时间的 outline"));
            }
            let text = words(content);
            if !["photosynthesis", "sunlight", "oxygen"]
                .iter()
                .all(|word| text.contains(word))
            {
                return Err(mismatch(
                    "短文讲述光合作用使用阳光并释放氧气，摘要未保留这些关键信息",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::MemoryCredentialVault;
    use std::sync::atomic::AtomicUsize;

    struct FakeTransport {
        status: u16,
        body: Vec<u8>,
        fail: bool,
        calls: AtomicUsize,
    }
    impl Transport for FakeTransport {
        fn send(&self, _request: &HttpRequest) -> Result<HttpResponse, TransportFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(TransportFailure::Unknown)
            } else {
                Ok(HttpResponse {
                    status: self.status,
                    body: self.body.clone(),
                })
            }
        }
    }

    fn config() -> ServiceConfiguration {
        ServiceConfiguration {
            name: "Test fixture".into(),
            protocol: ServiceProtocol::AiChat,
            endpoint: "https://example.test/v1/chat/completions".into(),
            model: "fixture-model".into(),
            authentication: Authentication::None,
            credential: None,
            credential_source: None,
        }
    }

    fn transport(status: u16, body: Value) -> FakeTransport {
        FakeTransport {
            status,
            body: serde_json::to_vec(&body).unwrap(),
            fail: false,
            calls: AtomicUsize::new(0),
        }
    }

    fn chat(content: &str) -> Value {
        json!({"choices":[{"message":{"content":content}}]})
    }

    fn run(fake: &FakeTransport) -> ServiceTestEvidence {
        run_test(
            &config(),
            TestKind::Proofread,
            &MemoryCredentialVault::new(),
            &AtomicBool::new(false),
            fake,
        )
    }

    #[test]
    fn http_200_html_error_json_and_missing_segment_ids_are_not_success() {
        let html = FakeTransport {
            status: 200,
            body: b"<html>login</html>".to_vec(),
            fail: false,
            calls: AtomicUsize::new(0),
        };
        assert_eq!(run(&html).outcome, TestOutcome::ContractMismatch);
        assert_ne!(
            run(&transport(
                200,
                json!({"error":{"message":"not available"}})
            ))
            .outcome,
            TestOutcome::Passed
        );
        assert_eq!(
            run(&transport(
                200,
                chat("{\"segments\":[{\"id\":9,\"text\":\"Something\"}]}")
            ))
            .outcome,
            TestOutcome::ContractMismatch
        );
    }

    #[test]
    fn structurally_valid_but_uncorrected_sample_is_distinct_from_bad_contract() {
        let evidence = run(&transport(
            200,
            chat(
                "{\"segments\":[{\"id\":0,\"text\":\"The student have three book.\"},{\"id\":1,\"text\":\"Water freezes at zero degrees Celsius.\"}]}",
            ),
        ));
        assert_eq!(evidence.outcome, TestOutcome::SampleMismatch);
        let evidence = run(&transport(
            200,
            chat(
                "{\"segments\":[{\"id\":0,\"text\":\"The student has three books.\"},{\"id\":1,\"text\":\"Water freezes at zero degrees Celsius.\"}]}",
            ),
        ));
        assert_eq!(evidence.outcome, TestOutcome::Passed);
    }

    #[test]
    fn unknown_response_is_never_automatically_resent() {
        let fake = FakeTransport {
            status: 0,
            body: vec![],
            fail: true,
            calls: AtomicUsize::new(0),
        };
        assert_eq!(run(&fake).outcome, TestOutcome::OutcomeUnknown);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cancel_before_dispatch_and_missing_credentials_send_nothing() {
        let fake = transport(200, chat("ignored"));
        let vault = MemoryCredentialVault::new();
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            run_test(&config(), TestKind::Proofread, &vault, &cancelled, &fake).outcome,
            TestOutcome::NotSent
        );
        let mut config = config();
        config.authentication = Authentication::ApiKey;
        assert_eq!(
            run_test(
                &config,
                TestKind::Proofread,
                &vault,
                &AtomicBool::new(false),
                &fake
            )
            .outcome,
            TestOutcome::NotSent
        );
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn auth_and_model_refusal_are_specific_and_do_not_expose_raw_error() {
        let evidence = run(&transport(
            401,
            json!({"error":{"message":"secret-bearing backend error"}}),
        ));
        assert_eq!(evidence.outcome, TestOutcome::AuthenticationRefused);
        assert!(!format!("{evidence:?}").contains("secret-bearing"));
        assert_eq!(
            run(&transport(404, json!({"error":{"code":"model_not_found"}}))).outcome,
            TestOutcome::ModelRefused
        );
    }

    #[test]
    fn repair_blocks_only_matching_explicit_configuration_refusals() {
        let original = config();
        let required = [TestKind::Proofread.contract()];
        let refusal = run(&transport(
            401,
            json!({"error": {"code": "invalid_api_key"}}),
        ));
        assert!(blocks_reprocessing(&refusal, &original, &required));
        let mut renamed = original.clone();
        renamed.name = "Renamed service".into();
        assert!(blocks_reprocessing(&refusal, &renamed, &required));
        for changed in [
            ServiceConfiguration {
                endpoint: "https://other.test/v1/chat/completions".into(),
                ..original.clone()
            },
            ServiceConfiguration {
                model: "another-model".into(),
                ..original.clone()
            },
            ServiceConfiguration {
                authentication: Authentication::ApiKey,
                credential: Some("credential-new".into()),
                ..original.clone()
            },
        ] {
            assert!(!blocks_reprocessing(&refusal, &changed, &required));
        }
        let model_refusal = run(&transport(
            404,
            json!({"error": {"code": "model_not_found"}}),
        ));
        assert!(blocks_reprocessing(&model_refusal, &original, &required));
        assert!(!blocks_reprocessing(
            &model_refusal,
            &original,
            &[TestKind::Summary.contract()]
        ));
        for outcome in [
            TestOutcome::Passed,
            TestOutcome::NotSent,
            TestOutcome::OutcomeUnknown,
            TestOutcome::ContractMismatch,
            TestOutcome::SampleMismatch,
        ] {
            let mut evidence = refusal.clone();
            evidence.outcome = outcome;
            assert!(!blocks_reprocessing(&evidence, &original, &required));
        }
    }

    #[test]
    fn retryable_http_status_is_not_a_configuration_refusal() {
        for status in [429, 500, 503] {
            let evidence = run(&transport(
                status,
                json!({"error": {"code": "model_not_found"}}),
            ));
            assert!(!matches!(
                evidence.outcome,
                TestOutcome::AuthenticationRefused | TestOutcome::ModelRefused
            ));
            assert!(!blocks_reprocessing(
                &evidence,
                &config(),
                &[TestKind::Proofread.contract()]
            ));
            // The guard also tolerates persisted results written before this fix.
            let mut older = evidence;
            older.outcome = TestOutcome::ModelRefused;
            assert!(!blocks_reprocessing(
                &older,
                &config(),
                &[TestKind::Proofread.contract()]
            ));
        }
        let forbidden = run(&transport(403, json!({"error": {"code": "forbidden"}})));
        assert!(blocks_reprocessing(
            &forbidden,
            &config(),
            &[TestKind::Proofread.contract()]
        ));
    }

    #[test]
    fn codex_login_required_maps_to_configuration_refusal() {
        let expired = anyhow::Error::new(course2md::login::codex::CodexLoginRequired);
        assert_eq!(
            codex_failure_outcome(&expired),
            TestOutcome::AuthenticationRefused,
            "登录缺失/刷新被拒必须阻断补做"
        );
        let wrapped = anyhow::Error::new(course2md::login::codex::CodexLoginRequired)
            .context("连接失败。请检查服务地址、密钥和模型。");
        assert_eq!(
            codex_failure_outcome(&wrapped),
            TestOutcome::AuthenticationRefused,
            "错误链中的标记同样生效"
        );
        assert_eq!(
            codex_failure_outcome(&anyhow::anyhow!("网络中断")),
            TestOutcome::ContractMismatch
        );
        // 标记与 CLI 修复文案都不含凭据
        assert!(!format!("{expired:#}").contains("--login"));
    }

    #[test]
    fn speech_and_vision_verify_sample_content_not_just_nonempty_text() {
        assert!(verify_sample(TestKind::Speech, "The blue notebook contains 7 pages.").is_ok());
        assert!(verify_sample(TestKind::Speech, "error unavailable").is_err());
        assert!(
            verify_sample(
                TestKind::Vision,
                "{\"segments\":[{\"id\":0,\"text\":\"The card shown is blue.\"}]}"
            )
            .is_ok()
        );
        assert!(
            verify_sample(
                TestKind::Vision,
                "{\"segments\":[{\"id\":0,\"text\":\"The card shown is red.\"}]}"
            )
            .is_err()
        );
        assert!(SPEECH_SAMPLE.starts_with(b"RIFF"));
        assert!(SPEECH_SAMPLE.len() > 16000);
        assert!(BLUE_CARD.starts_with(b"\x89PNG"));
    }
}
