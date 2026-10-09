//! Durable external-request receipts. A lost response is never permission to resend.

use crate::{checkpoint::atomic_write, execution};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Control {
    pub intent: String,
    pub stopped_services: Vec<String>,
    /// IDs identify an exact previous attempt, so persistent authorization is consumed once.
    pub resend: Vec<String>,
}
impl Default for Control {
    fn default() -> Self {
        Self {
            intent: "run".into(),
            stopped_services: vec![],
            resend: vec![],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Sending,
    Completed,
    /// DNS or connection establishment failed before the request body could be sent.
    NotSent,
    Rejected,
    Failed,
    Uncertain,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Receipt {
    pub schema: u32,
    pub stable_id: String,
    pub request_id: String,
    pub purpose: String,
    #[serde(default)]
    pub description: String,
    pub service_version: String,
    pub attempt: u32,
    pub state: State,
    pub http_status: Option<u16>,
    pub response: Option<Value>,
    pub message: Option<String>,
    /// A reviewed compatibility rejection; never stores the provider's error body.
    #[serde(default)]
    pub unsupported_response_format: bool,
    /// Explicit component reprocessing permits this exact known-failed attempt once.
    /// Sending/uncertain attempts still require Control::resend authorization.
    #[serde(default)]
    pub retry_authorized: Option<String>,
}

#[derive(Debug)]
pub struct Failure {
    pub status: Option<u16>,
    pub retryable: bool,
    pub uncertain: bool,
    pub message: String,
    pub unsupported_response_format: bool,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Failure {}
impl Failure {
    fn local(error: impl std::fmt::Display) -> Self {
        Self {
            status: None,
            retryable: false,
            uncertain: false,
            message: error.to_string(),
            unsupported_response_format: false,
        }
    }
}

fn rejection_message(status: u16) -> String {
    match status {
        401 => "服务未接受此任务保存的凭据，请检查对应服务的 API Key。 / The service rejected the saved credentials; check the API key for that service.".into(),
        403 => "此任务使用的凭据没有访问该服务或模型的权限。 / The credentials used by this task cannot access that service or model.".into(),
        404 => "服务未找到此任务指定的接口或模型，请检查服务地址和模型。 / The service does not have the endpoint or model this task names; check the base URL and model.".into(),
        429 => "服务请求次数达到限制，请稍后重试。 / The service rate limit was reached; try again later.".into(),
        400 | 422 => {
            format!("服务拒绝了请求参数（HTTP {status}），请检查此任务使用的模型与服务设置。 / The service rejected the request parameters (HTTP {status}); check the model and service settings for this task.")
        }
        _ => format!("服务未完成此请求（HTTP {status}）。 / The service did not complete this request (HTTP {status})."),
    }
}

/// A status alone is not evidence that changing the payload will help. Classify
/// only an explicit rejection of response_format, without retaining error text.
fn rejects_response_format(status: u16, value: Option<&Value>) -> bool {
    if !matches!(status, 400 | 422) {
        return false;
    }
    let Some(error) = value.and_then(|v| v.get("error")) else {
        return false;
    };
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let param = error
        .get("param")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let named = param == "response_format" || message.contains("response_format");
    named
        && (matches!(
            code,
            "unsupported_parameter" | "unknown_parameter" | "unrecognized_parameter"
        ) || [
            "not supported",
            "unsupported",
            "unknown parameter",
            "unrecognized",
            "not recognized",
            "does not support",
            "不支持",
        ]
        .iter()
        .any(|term| message.contains(term)))
}

pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}
pub struct NetworkFailure {
    pub message: String,
    pub definitely_unsent: bool,
}

struct Ledger {
    dir: PathBuf,
    control_path: Option<PathBuf>,
    service_versions: BTreeMap<String, String>,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    emitted: Mutex<HashSet<String>>,
}
static ACTIVE: OnceLock<Mutex<Option<Arc<Ledger>>>> = OnceLock::new();
fn active() -> Option<Arc<Ledger>> {
    ACTIVE.get_or_init(|| Mutex::new(None)).lock().ok()?.clone()
}
pub fn is_active() -> bool {
    active().is_some()
}
pub struct Guard(Arc<Ledger>);
impl Drop for Guard {
    fn drop(&mut self) {
        // 锁中毒也要清掉当前 guard：否则 install 会一直报「同一进程不能同时执行两个任务」
        let mut current = ACTIVE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if current
            .as_ref()
            .is_some_and(|value| Arc::ptr_eq(value, &self.0))
        {
            *current = None;
        }
    }
}

pub fn install(
    work_dir: &Path,
    control_path: Option<&Path>,
    service_versions: &BTreeMap<String, String>,
) -> Result<Guard> {
    let ledger = Arc::new(Ledger::new(work_dir, control_path, service_versions)?);
    let mut current = ACTIVE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| {
            anyhow::anyhow!("任务请求状态锁不可用 / Task request ledger lock unavailable")
        })?;
    anyhow::ensure!(
        current.is_none(),
        "同一进程不能同时执行两个任务 / Another task is already active"
    );
    *current = Some(ledger.clone());
    Ok(Guard(ledger))
}

/// Safe boundaries for local work. Unknown cloud receipts do not prevent saving local work.
pub fn check_control() -> Result<()> {
    if let Some(ledger) = active() {
        ledger.check_intent(None).map_err(anyhow::Error::new)?;
    }
    Ok(())
}

/// 轮询任务控制文件，意图不再是 run 时返回 Err。配合 `tokio::select!` 打断
/// 长时间运行的分支（子进程句谋 kill_on_drop 随分支 dropped 生效）。
/// 无活动任务账本时 check_control 恒 Ok，此 future 永不完成，不影响 CLI 独立调用。
pub async fn watch_control() -> anyhow::Error {
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Err(e) = check_control() {
            return e;
        }
    }
}

/// Exposes unresolved attempts for task state reconciliation even if the process crashed.
pub fn receipts(work_dir: &Path) -> Result<Vec<Receipt>> {
    read_receipts(&work_dir.join("requests"))
}
fn read_receipts(dir: &Path) -> Result<Vec<Receipt>> {
    if !dir.is_dir() {
        return Ok(vec![]);
    }
    let mut result = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "json") {
            result.push(serde_json::from_slice(&std::fs::read(path)?).context("外部请求记录损坏；没有重新发送 / Request receipt is damaged; no request was resent")?);
        }
    }
    Ok(result)
}

impl Ledger {
    fn new(
        work_dir: &Path,
        control_path: Option<&Path>,
        versions: &BTreeMap<String, String>,
    ) -> Result<Self> {
        let dir = work_dir.join("requests");
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            control_path: control_path.map(Path::to_path_buf),
            service_versions: versions.clone(),
            locks: Mutex::new(HashMap::new()),
            emitted: Mutex::new(HashSet::new()),
        })
    }
    fn control(&self) -> Result<Control> {
        match &self.control_path {
            Some(path) if path.exists() => serde_json::from_slice(&std::fs::read(path)?)
                .context("任务控制状态无法读取；已停止派发 / Cannot read task control"),
            _ => Ok(Control::default()),
        }
    }
    fn block(&self, reason: &str, request: Option<&Receipt>, message: &str) -> Failure {
        let request_id = request.map(|r| r.request_id.clone());
        let message = request.filter(|r| !r.description.is_empty()).map_or_else(
            || message.to_string(),
            |r| format!("{}：{message}", r.description),
        );
        let signature = format!("{reason}:{request_id:?}:{message}");
        if self
            .emitted
            .lock()
            .map(|mut seen| seen.insert(signature))
            .unwrap_or(true)
        {
            crate::progress::emit(
                serde_json::json!({"type":"blocked","reason":reason,"request_id":request_id,"purpose":request.map(|r|&r.purpose),"message":message,"description":request.map(|r|&r.description)}),
            );
        }
        Failure {
            status: None,
            retryable: false,
            uncertain: reason == "uncertain",
            message,
            unsupported_response_format: false,
        }
    }
    fn check_intent(&self, service_version: Option<&str>) -> std::result::Result<Control, Failure> {
        let control = self.control().map_err(Failure::local)?;
        let reason = match control.intent.as_str() {
            "run" => None,
            "pause" => Some((
                "paused",
                "生成已暂停，进度已保留 / Generation paused; progress retained",
            )),
            "cancel" => Some((
                "cancelled",
                "任务已取消，已有成果已保留 / Task cancelled; saved results retained",
            )),
            "quit" => Some((
                "quit",
                "已保存进度，等待下次继续 / Progress saved; waiting to continue",
            )),
            _ => Some((
                "paused",
                "任务运行意图无效，已停止派发 / Invalid task intent; dispatch stopped",
            )),
        };
        if let Some((reason, message)) = reason {
            return Err(self.block(reason, None, message));
        }
        if service_version
            .is_some_and(|version| control.stopped_services.iter().any(|v| v == version))
        {
            return Err(self.block(
                "service_stopped",
                None,
                "此服务已删除，没有发送新请求 / Service deleted; no new request sent",
            ));
        }
        Ok(control)
    }
    fn save(&self, receipt: &Receipt) -> std::result::Result<(), Failure> {
        atomic_write(
            &self.dir.join(format!("{}.json", receipt.stable_id)),
            &serde_json::to_vec_pretty(receipt).map_err(Failure::local)?,
        )
        .map_err(Failure::local)
    }
    #[cfg(test)]
    fn send(
        &self,
        service: &str,
        purpose: &str,
        endpoint: &str,
        identity: &Value,
        send: impl FnOnce() -> std::result::Result<HttpResponse, NetworkFailure>,
        validate: impl FnOnce(&Value) -> Result<()>,
    ) -> std::result::Result<Value, Failure> {
        self.send_described(
            service, purpose, purpose, endpoint, identity, send, validate,
        )
    }
    // Keep the transport, validation and persisted request identity explicit at the call site.
    #[allow(clippy::too_many_arguments)]
    fn send_described(
        &self,
        service: &str,
        purpose: &str,
        description: &str,
        endpoint: &str,
        identity: &Value,
        send: impl FnOnce() -> std::result::Result<HttpResponse, NetworkFailure>,
        validate: impl FnOnce(&Value) -> Result<()>,
    ) -> std::result::Result<Value, Failure> {
        let service_version = self
            .service_versions
            .get(service)
            .cloned()
            .unwrap_or_else(|| format!("snapshot:{}", execution::digest(endpoint.as_bytes())));
        let stable_id = execution::digest(&serde_json::to_vec(&serde_json::json!({"service_version":service_version,"purpose":purpose,"endpoint":endpoint,"payload":identity})).map_err(Failure::local)?);
        let lock = self
            .locks
            .lock()
            .map_err(Failure::local)?
            .entry(stable_id.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _lock = lock.lock().map_err(Failure::local)?;
        let path = self.dir.join(format!("{stable_id}.json"));
        let old: Option<Receipt> = if path.exists() {
            Some(
                serde_json::from_slice(&std::fs::read(&path).map_err(Failure::local)?)
                    .map_err(Failure::local)?,
            )
        } else {
            None
        };
        if let Some(old) = &old
            && old.state == State::Completed
        {
            let response = old.response.clone().ok_or_else(|| {
                Failure::local(
                    "已完成请求缺少保存结果；没有重新发送 / Saved request result is missing",
                )
            })?;
            validate(&response).map_err(Failure::local)?;
            return Ok(response);
        }
        let control = self.check_intent(Some(&service_version))?;
        let authorized = old.as_ref().is_some_and(|receipt| {
            control.resend.contains(&receipt.request_id)
                || (matches!(receipt.state, State::Failed | State::Rejected)
                    && receipt.retry_authorized.as_ref() == Some(&receipt.request_id))
        });
        if let Some(old) = &old {
            if matches!(old.state, State::Sending | State::Uncertain) && !authorized {
                let mut uncertain = old.clone();
                uncertain.state = State::Uncertain;
                self.save(&uncertain)?;
                return Err(self.block("uncertain", Some(old), "未收到服务的确定结果。服务可能已处理这部分，再次提交可能产生额外费用 / Service result is uncertain; resending may incur additional charges"));
            }
            if old.state == State::Failed && !authorized {
                return Err(Failure {
                    status: old.http_status,
                    retryable: false,
                    uncertain: false,
                    message: old.message.clone().unwrap_or_else(|| {
                        "此请求未完成，已保留响应 / Request failed; response retained".into()
                    }),
                    unsupported_response_format: old.unsupported_response_format,
                });
            }
            if old.state == State::Rejected && old.http_status != Some(429) && !authorized {
                return Err(Failure {
                    status: old.http_status,
                    retryable: false,
                    uncertain: false,
                    message: old.message.clone().unwrap_or_else(|| {
                        "服务拒绝了此请求 / Service rejected this request".into()
                    }),
                    unsupported_response_format: old.unsupported_response_format,
                });
            }
        }
        // An unknown prerequisite cannot be bypassed by splitting/relaxing the payload
        // or by beginning downstream summary requests. Unrelated local work remains possible.
        for unresolved in read_receipts(&self.dir).map_err(Failure::local)? {
            if unresolved.stable_id != stable_id
                && matches!(unresolved.state, State::Sending | State::Uncertain)
            {
                // Live in-flight requests may run concurrently; only stale Sending from another
                // process is unknown. A lock currently held in this context proves it is live.
                let live = self
                    .locks
                    .lock()
                    .map_err(Failure::local)?
                    .get(&unresolved.stable_id)
                    .cloned()
                    .is_some_and(|lock| lock.try_lock().is_err());
                if unresolved.state == State::Uncertain || !live {
                    return Err(self.block("uncertain", Some(&unresolved), "前一步请求结果尚不确定；已保留进度，没有继续发送 / A previous request is unresolved; progress retained"));
                }
            }
        }
        let attempt = old.as_ref().map_or(1, |r| r.attempt + 1);
        let mut receipt = Receipt {
            schema: 1,
            stable_id: stable_id.clone(),
            request_id: format!("{stable_id}.{attempt}"),
            purpose: purpose.into(),
            description: description.into(),
            service_version,
            attempt,
            state: State::Sending,
            http_status: None,
            response: None,
            message: None,
            unsupported_response_format: false,
            retry_authorized: None,
        };
        // This durable write happens before the only call that can send network bytes.
        self.save(&receipt)?;
        let response = match send() {
            Ok(response) => response,
            Err(error) => {
                if error.definitely_unsent {
                    receipt.state = State::NotSent;
                    receipt.message = Some(error.message.clone());
                    self.save(&receipt)?;
                    return Err(Failure {
                        status: None,
                        retryable: true,
                        uncertain: false,
                        message: error.message,
                        unsupported_response_format: false,
                    });
                }
                receipt.state = State::Uncertain;
                receipt.message = Some(error.message);
                self.save(&receipt)?;
                return Err(self.block("uncertain", Some(&receipt), "未收到服务的确定结果。服务可能已处理这部分，再次提交可能产生额外费用 / Service result is uncertain; resending may incur additional charges"));
            }
        };
        receipt.http_status = Some(response.status);
        let value = serde_json::from_slice::<Value>(&response.body);
        if response.status == 429 || (400..500).contains(&response.status) {
            receipt.state = State::Rejected;
            receipt.unsupported_response_format =
                rejects_response_format(response.status, value.as_ref().ok());
            receipt.message = Some(rejection_message(response.status));
            // Error bodies may echo credentials. Keep status, not the untrusted body.
            self.save(&receipt)?;
            return Err(Failure {
                status: Some(response.status),
                // Each caller bounds its retry loop; a later task continuation may retry
                // a confirmed rejection without requiring uncertain-request authorization.
                retryable: response.status == 429,
                uncertain: false,
                message: receipt.message.unwrap(),
                unsupported_response_format: receipt.unsupported_response_format,
            });
        }
        if !(200..300).contains(&response.status) {
            receipt.state = State::Uncertain;
            self.save(&receipt)?;
            return Err(self.block("uncertain", Some(&receipt), "服务返回错误，无法确认是否已处理。再次提交可能产生额外费用 / Service processing is uncertain"));
        }
        let value = match value {
            Ok(value) => value,
            Err(_) => {
                receipt.state = State::Failed;
                receipt.message = Some(
                    "服务已回应，但返回的内容不是有效 JSON / Service returned invalid JSON".into(),
                );
                self.save(&receipt)?;
                return Err(Failure::local(receipt.message.unwrap()));
            }
        };
        if let Err(error) = validate(&value) {
            receipt.state = State::Failed;
            receipt.message = Some(format!("{error:#}"));
            self.save(&receipt)?;
            return Err(Failure::local(receipt.message.unwrap()));
        }
        receipt.state = State::Completed;
        receipt.response = Some(value.clone());
        self.save(&receipt)?;
        Ok(value)
    }
}

pub fn json_request_described(
    service: &str,
    purpose: &str,
    description: &str,
    endpoint: &str,
    identity: &Value,
    send: impl FnOnce() -> std::result::Result<HttpResponse, NetworkFailure>,
    validate: impl FnOnce(&Value) -> Result<()>,
) -> std::result::Result<Value, Failure> {
    if let Some(ledger) = active() {
        return ledger.send_described(
            service,
            purpose,
            description,
            endpoint,
            identity,
            send,
            validate,
        );
    }
    // The CLI has no durable task context. Only confirmed unsent/429 are retryable.
    let response = send().map_err(|e| Failure {
        status: None,
        retryable: e.definitely_unsent,
        uncertain: !e.definitely_unsent,
        message: e.message,
        unsupported_response_format: false,
    })?;
    if !(200..300).contains(&response.status) {
        return Err(Failure {
            status: Some(response.status),
            retryable: response.status == 429,
            uncertain: response.status >= 500,
            message: rejection_message(response.status),
            unsupported_response_format: rejects_response_format(
                response.status,
                serde_json::from_slice::<Value>(&response.body)
                    .ok()
                    .as_ref(),
            ),
        });
    }
    let value = serde_json::from_slice(&response.body).map_err(Failure::local)?;
    validate(&value).map_err(Failure::local)?;
    Ok(value)
}

/// Drain the HTTP body before recording completion; a body read error is still uncertain.
/// Cloud callers must disable redirects: a failed connection after a redirect would not
/// prove that the initial request was never handled.
pub fn receive(
    result: std::result::Result<ureq::Response, ureq::Error>,
) -> std::result::Result<HttpResponse, NetworkFailure> {
    use std::io::Read;
    let response = match result {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(error)) => {
            let definitely_unsent = matches!(
                error.kind(),
                ureq::ErrorKind::Dns | ureq::ErrorKind::ConnectionFailed
            );
            return Err(NetworkFailure {
                message: if definitely_unsent {
                    "无法连接服务，尚未提交内容 / Could not connect; request content was not submitted"
                } else {
                    "未完整收到服务响应 / Did not receive a complete service response"
                }
                .into(),
                definitely_unsent,
            });
        }
    };
    let status = response.status();
    let mut body = Vec::new();
    response
        .into_reader()
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut body)
        .map_err(|_| NetworkFailure {
            message: "服务响应接收中断 / Service response was interrupted".into(),
            definitely_unsent: false,
        })?;
    if body.len() > 32 * 1024 * 1024 {
        return Err(NetworkFailure {
            message: "服务响应超过接收上限 / Service response exceeds the receiving limit".into(),
            definitely_unsent: false,
        });
    }
    Ok(HttpResponse { status, body })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn response_format_fallback_requires_explicit_provider_evidence() {
        for status in [400, 422] {
            assert!(rejects_response_format(
                status,
                Some(
                    &serde_json::json!({"error":{"param":"response_format","code":"unsupported_parameter"}})
                )
            ));
            assert!(rejects_response_format(
                status,
                Some(
                    &serde_json::json!({"error":{"message":"This model does not support response_format"}})
                )
            ));
            for error in [
                serde_json::json!({"error":{"message":"Invalid request parameters"}}),
                serde_json::json!({"error":{"param":"model","code":"unsupported_parameter"}}),
                serde_json::json!({"error":{"param":"response_format","message":"Request quota exhausted"}}),
            ] {
                assert!(!rejects_response_format(status, Some(&error)));
            }
        }
        assert!(!rejects_response_format(
            500,
            Some(&serde_json::json!({"error":{"message":"response_format is unsupported"}}))
        ));
    }

    #[test]
    fn explicit_failed_component_authorization_is_consumed_before_sending() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::new(dir.path(), None, &Default::default()).unwrap();
        let payload = serde_json::json!({"text":"lesson"});
        let calls = AtomicUsize::new(0);
        let reject = || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(HttpResponse {
                status: 400,
                body: br#"{"error":{"message":"invalid model"}}"#.to_vec(),
            })
        };
        assert!(
            ledger
                .send(
                    "llm",
                    "summary",
                    "http://example.test",
                    &payload,
                    reject,
                    |_| Ok(())
                )
                .is_err()
        );
        let mut receipt = receipts(dir.path()).unwrap().remove(0);
        receipt.retry_authorized = Some(receipt.request_id.clone());
        ledger.save(&receipt).unwrap();
        assert!(
            ledger
                .send(
                    "llm",
                    "summary",
                    "http://example.test",
                    &payload,
                    reject,
                    |_| Ok(())
                )
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let mut receipt = receipts(dir.path()).unwrap().remove(0);
        assert!(receipt.retry_authorized.is_none());
        assert!(
            ledger
                .send(
                    "llm",
                    "summary",
                    "http://example.test",
                    &payload,
                    || panic!("same task may not repeat a rejected attempt"),
                    |_| Ok(())
                )
                .is_err()
        );
        // Even a stale/forged known-failure authorization never authorizes an unknown result.
        receipt.state = State::Uncertain;
        receipt.retry_authorized = Some(receipt.request_id.clone());
        ledger.save(&receipt).unwrap();
        assert!(
            ledger
                .send(
                    "llm",
                    "summary",
                    "http://example.test",
                    &payload,
                    || panic!("unknown requires Control::resend"),
                    |_| Ok(())
                )
                .unwrap_err()
                .uncertain
        );
    }
    fn success() -> std::result::Result<HttpResponse, NetworkFailure> {
        Ok(HttpResponse {
            status: 200,
            body: br#"{"text":"saved"}"#.to_vec(),
        })
    }
    #[test]
    fn confirmed_connection_failure_can_resume_without_resend_authorization() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::new(dir.path(), None, &Default::default()).unwrap();
        let payload = serde_json::json!({"text":"lesson"});
        let client = crate::runtime::http_agent(std::time::Duration::from_secs(2));
        let failure = ledger
            .send(
                "llm",
                "summary",
                &endpoint,
                &payload,
                || receive(client.post(&endpoint).send_json(&payload)),
                |_| Ok(()),
            )
            .unwrap_err();
        assert!(failure.retryable);
        assert!(!failure.uncertain);
        assert_eq!(receipts(dir.path()).unwrap()[0].state, State::NotSent);
        ledger
            .send("llm", "summary", &endpoint, &payload, success, |_| Ok(()))
            .unwrap();
        let receipt = &receipts(dir.path()).unwrap()[0];
        assert_eq!(receipt.state, State::Completed);
        assert_eq!(receipt.attempt, 2);
    }
    #[test]
    fn uncertain_attempt_requires_exact_single_use_authorization() {
        let dir = tempfile::tempdir().unwrap();
        let control = dir.path().join("control.json");
        let ledger = Ledger::new(dir.path(), Some(&control), &Default::default()).unwrap();
        let payload = serde_json::json!({"model":"test","audio":"hash"});
        let calls = AtomicUsize::new(0);
        let fail = || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(NetworkFailure {
                message: "lost".into(),
                definitely_unsent: false,
            })
        };
        assert!(
            ledger
                .send(
                    "asr",
                    "transcription",
                    "https://example.test",
                    &payload,
                    fail,
                    |_| Ok(())
                )
                .unwrap_err()
                .uncertain
        );
        let first = receipts(dir.path()).unwrap()[0].request_id.clone();
        assert!(
            ledger
                .send(
                    "asr",
                    "transcription",
                    "https://example.test",
                    &payload,
                    fail,
                    |_| Ok(())
                )
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        atomic_write(
            &control,
            &serde_json::to_vec(&Control {
                resend: vec![first],
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        assert!(
            ledger
                .send(
                    "asr",
                    "transcription",
                    "https://example.test",
                    &payload,
                    fail,
                    |_| Ok(())
                )
                .is_err()
        );
        assert!(
            ledger
                .send(
                    "asr",
                    "transcription",
                    "https://example.test",
                    &payload,
                    fail,
                    |_| Ok(())
                )
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let second = receipts(dir.path()).unwrap()[0].request_id.clone();
        atomic_write(
            &control,
            &serde_json::to_vec(&Control {
                resend: vec![second],
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        let result = ledger
            .send(
                "asr",
                "transcription",
                "https://example.test",
                &payload,
                success,
                |_| Ok(()),
            )
            .unwrap();
        let cached = ledger
            .send(
                "asr",
                "transcription",
                "https://example.test",
                &payload,
                || panic!("completed request must not send again"),
                |_| Ok(()),
            )
            .unwrap();
        assert_eq!(cached, result);
    }
    #[test]
    fn durable_sending_and_protocol_failure_are_not_automatic_retries() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::new(dir.path(), None, &Default::default()).unwrap();
        let payload = serde_json::json!({"text":"lesson"});
        let result = ledger.send(
            "llm",
            "proofreading",
            "https://example.test",
            &payload,
            || {
                assert_eq!(receipts(dir.path()).unwrap()[0].state, State::Sending);
                Ok(HttpResponse {
                    status: 200,
                    body: br#"{"error":"invalid"}"#.to_vec(),
                })
            },
            |_| anyhow::bail!("协议错误"),
        );
        assert!(result.is_err());
        assert!(
            ledger
                .send(
                    "llm",
                    "proofreading",
                    "https://example.test",
                    &payload,
                    || panic!("protocol errors must not resend"),
                    |_| Ok(())
                )
                .is_err()
        );
    }
    #[test]
    fn paused_or_revoked_service_sends_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.json");
        let versions = BTreeMap::from([("llm".into(), "service-v1".into())]);
        let ledger = Ledger::new(dir.path(), Some(&path), &versions).unwrap();
        for control in [
            Control {
                intent: "pause".into(),
                ..Default::default()
            },
            Control {
                stopped_services: vec!["service-v1".into()],
                ..Default::default()
            },
        ] {
            atomic_write(&path, &serde_json::to_vec(&control).unwrap()).unwrap();
            assert!(
                ledger
                    .send(
                        "llm",
                        "summary",
                        "https://example.test",
                        &Value::Null,
                        || panic!("blocked request sent"),
                        |_| Ok(())
                    )
                    .is_err()
            );
        }
        assert!(receipts(dir.path()).unwrap().is_empty());
    }
}
