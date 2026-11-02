//! Bounded HTTP requests, shared retries and a success-only response cache.

use crate::{checkpoint::atomic_write, runtime};
use anyhow::Result;
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

pub const HTTP_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_ATTEMPTS: u32 = 3;
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;
pub const CACHED: &str = "notepipe_cached";

pub fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(HTTP_TIMEOUT)
        .timeout_connect(HTTP_TIMEOUT)
        .redirects(0)
        .build()
}

pub struct Item {
    pub label: String,
    pub failure: String,
}
impl Item {
    pub fn speech(start: f64, end: f64) -> Self {
        let range = format!(
            "{}–{}",
            crate::render::fmt_ts(start),
            crate::render::fmt_ts(end)
        );
        Self {
            label: format!("Speech at {range}"),
            failure: format!("Speech recognition failed at {range}"),
        }
    }

    pub fn stage(name: &str, detail: &str) -> Self {
        Self {
            label: format!(
                "Stage {name}{}{detail}",
                if detail.is_empty() { "" } else { ", " }
            ),
            failure: format!(
                "Stage {name} failed{}{detail}",
                if detail.is_empty() {
                    ""
                } else if detail.starts_with("while ") {
                    " "
                } else {
                    " for "
                }
            ),
        }
    }
}

pub fn request<T>(
    item: &Item,
    control: &dyn Fn() -> Result<()>,
    mut attempt_once: impl FnMut() -> std::result::Result<T, Failure>,
) -> std::result::Result<T, Failure> {
    let mut run = || {
        for attempt in 1..=MAX_ATTEMPTS {
            control().map_err(Failure::local)?;
            match attempt_once() {
                Ok(value) => return Ok(value),
                Err(error) if error.retryable && attempt < MAX_ATTEMPTS => {
                    control().map_err(Failure::local)?;
                    let wait = runtime::retry_backoff(attempt);
                    warn(format!(
                        "{}: attempt {attempt} of 3 failed: {}; retrying in {:.1} s",
                        item.label,
                        error.message,
                        wait.as_secs_f64()
                    ));
                    let deadline = Instant::now() + wait;
                    loop {
                        control().map_err(Failure::local)?;
                        let left = deadline.saturating_duration_since(Instant::now());
                        if left.is_zero() {
                            break;
                        }
                        runtime::interruptible_sleep(left.min(Duration::from_millis(200)))
                            .map_err(Failure::local)?;
                    }
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!()
    };
    run().map_err(|mut error| {
        error.message = format!("{}: {}", item.failure, error.message);
        error
    })
}

/// Provider diagnostics are displayed only, with credentials removed before normalization or truncation.
pub fn diagnostic(message: &str, key: &str) -> String {
    let message = if key.is_empty() {
        message.to_owned()
    } else {
        message.replace(key, "[key]")
    };
    let message: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let message = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if message.chars().count() > 300 {
        format!("{}…", message.chars().take(299).collect::<String>())
    } else {
        message
    }
}

#[derive(Debug)]
pub struct Failure {
    pub retryable: bool,
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
    pub fn local(error: impl std::fmt::Display) -> Self {
        Self {
            retryable: false,
            message: error.to_string(),
            unsupported_response_format: false,
        }
    }
    pub(crate) fn new(retryable: bool, message: impl Into<String>) -> Self {
        Self {
            retryable,
            message: message.into(),
            ..Self::local("")
        }
    }
}
fn classify_rejection(status: u16, body: &[u8], key: &str) -> Failure {
    let output_limit = rejects_output_limit(body);
    let mut message = rejection_message(status);
    if let Some(text) = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value["error"]["message"].as_str().map(str::to_owned))
    {
        message.push_str(&format!(" The service said: {}", diagnostic(&text, key)));
    }
    Failure {
        retryable: !output_limit && (status == 429 || (500..=599).contains(&status)),
        unsupported_response_format: !output_limit && rejects_response_format(status, body),
        message,
    }
}

fn rejection_message(status: u16) -> String {
    match status {
        401 => "The service rejected the saved credentials; check the API key for that service.".into(),
        403 => "The credentials used by this task cannot access that service or model.".into(),
        404 => "The service does not have the endpoint or model this task names; check the base URL and model.".into(),
        429 => "The service rate limit was reached; try again later.".into(),
        400 | 422 => {
            format!("The service rejected the request parameters (HTTP {}); check the model and service settings for this task.", status)
        }
        _ => format!("The service did not complete this request (HTTP {}).", status),
    }
}

/// Classify the named request field, including servers that return plain text errors.
fn rejects_response_format(status: u16, body: &[u8]) -> bool {
    matches!(status, 400 | 422)
        && body
            .windows(b"response_format".len())
            .any(|part| part.eq_ignore_ascii_case(b"response_format"))
}

/// The adapter contract's code for a result cut off at the model's output limit.
fn rejects_output_limit(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .is_ok_and(|value| value["error"]["code"] == "output_limit")
}

pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

struct Cache {
    dir: PathBuf,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    warnings: Mutex<Vec<String>>,
}
static ACTIVE: OnceLock<Mutex<Option<Arc<Cache>>>> = OnceLock::new();
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

pub struct Guard(Arc<Cache>);
impl Guard {
    pub fn take_warnings(&self) -> Vec<String> {
        std::mem::take(&mut *runtime::lock_recover(&self.0.warnings))
    }
}

pub fn warn(message: String) {
    tracing::warn!("{message}");
    if let Some(cache) = runtime::lock_recover(ACTIVE.get_or_init(|| Mutex::new(None))).as_ref() {
        runtime::lock_recover(&cache.warnings).push(message);
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        let mut current = runtime::lock_recover(ACTIVE.get_or_init(|| Mutex::new(None)));
        if current
            .as_ref()
            .is_some_and(|value| Arc::ptr_eq(value, &self.0))
        {
            *current = None;
        }
    }
}

pub fn install(work_dir: &Path) -> Result<Guard> {
    let cache = Arc::new(Cache {
        dir: work_dir.join("llm"),
        locks: Mutex::new(HashMap::new()),
        warnings: Mutex::new(Vec::new()),
    });
    let mut current = runtime::lock_recover(ACTIVE.get_or_init(|| Mutex::new(None)));
    anyhow::ensure!(current.is_none(), "Another task is already active");
    *current = Some(cache.clone());
    Ok(Guard(cache))
}

/// Save an already validated fallback response under the original request's key too.
pub(crate) fn cache_validated_response(endpoint: &str, body: &Value, value: &Value) -> Result<()> {
    let cache = runtime::lock_recover(ACTIVE.get_or_init(|| Mutex::new(None))).clone();
    if let Some(cache) = cache {
        runtime::check_cancelled()?;
        let key = runtime::digest(format!("{endpoint}\n{body}").as_bytes());
        atomic_write(
            &cache.dir.join(format!("{key}.json")),
            &serde_json::to_vec(value)?,
        )?;
    }
    Ok(())
}

pub fn cached_json_request(
    endpoint: &str,
    body: &Value,
    api_key: &str,
    stage: &str,
    send: impl FnOnce() -> std::result::Result<HttpResponse, Failure>,
    validate: impl Fn(&Value) -> Result<()>,
) -> std::result::Result<Value, Failure> {
    let cache = runtime::lock_recover(ACTIVE.get_or_init(|| Mutex::new(None))).clone();
    let Some(cache) = cache else {
        return json_request(api_key, stage, send, validate);
    };
    let key = runtime::digest(format!("{endpoint}\n{body}").as_bytes());
    let lock = runtime::lock_recover(&cache.locks)
        .entry(key.clone())
        .or_default()
        .clone();
    let _lock = runtime::lock_recover(&lock);
    runtime::check_cancelled().map_err(Failure::local)?;
    let path = cache.dir.join(format!("{key}.json"));
    if path.is_file() {
        let cached = std::fs::File::open(&path)
            .and_then(|file| {
                use std::io::Read;
                let mut bytes = Vec::new();
                file.take(MAX_RESPONSE_BYTES + 1).read_to_end(&mut bytes)?;
                Ok(bytes)
            })
            .map_err(Failure::local)?;
        if cached.len() as u64 <= MAX_RESPONSE_BYTES
            && let Ok(mut value) = serde_json::from_slice::<Value>(&cached)
            && validate(&value).is_ok()
        {
            value[CACHED] = true.into();
            return Ok(value);
        }
    }
    let value = json_request(api_key, stage, send, validate)?;
    runtime::check_cancelled().map_err(Failure::local)?;
    atomic_write(&path, &serde_json::to_vec(&value).map_err(Failure::local)?)
        .map_err(Failure::local)?;
    Ok(value)
}

pub fn json_request(
    key: &str,
    stage: &str,
    send: impl FnOnce() -> std::result::Result<HttpResponse, Failure>,
    validate: impl FnOnce(&Value) -> Result<()>,
) -> std::result::Result<Value, Failure> {
    runtime::check_cancelled().map_err(Failure::local)?;
    let response = send()?;
    runtime::check_cancelled().map_err(Failure::local)?;
    if !(200..300).contains(&response.status) {
        return Err(classify_rejection(response.status, &response.body, key));
    }
    let value: Value = serde_json::from_slice(&response.body)
        .map_err(|_| Failure::new(true, "The service response is not valid JSON"))?;
    if response.status == 200 {
        let usage = &value["usage"];
        crate::progress::note_tokens(
            stage,
            usage["prompt_tokens"].as_u64().unwrap_or(0),
            usage["completion_tokens"].as_u64().unwrap_or(0),
        );
    }
    validate(&value).map_err(|error| {
        error
            .downcast::<Failure>()
            .unwrap_or_else(|error| Failure::new(true, error.to_string()))
    })?;
    Ok(value)
}

pub fn check_finish_reason(value: &Value) -> Result<()> {
    if value["error"]["code"] == "output_limit" {
        return Err(Failure::local("the model stopped at its output limit").into());
    }
    let message = match value["choices"][0]["finish_reason"].as_str() {
        Some("length") => "the model stopped at its output limit",
        Some("content_filter") => "the model's content filter stopped the answer",
        _ => return Ok(()),
    };
    Err(Failure::local(message).into())
}

/// The control deadline also bounds DNS and body reads that the HTTP library cannot interrupt itself.
pub fn send_http(
    control: &dyn Fn() -> Result<()>,
    request: ureq::Request,
    body: Vec<u8>,
    key: &str,
) -> std::result::Result<HttpResponse, Failure> {
    send_with_timeout(control, request, body, key, HTTP_TIMEOUT)
}

fn send_with_timeout(
    control: &dyn Fn() -> Result<()>,
    request: ureq::Request,
    body: Vec<u8>,
    key: &str,
    timeout: Duration,
) -> std::result::Result<HttpResponse, Failure> {
    let started = Instant::now();
    let url = request.url().to_owned();
    let (worker_url, key) = (url.clone(), key.to_owned());
    runtime::interruptible_http(
        &|| {
            control()?;
            if started.elapsed() >= timeout {
                return Err(
                    Failure::new(true, connection_message(&url, "no answer within 600 s")).into(),
                );
            }
            Ok(())
        },
        move || {
            receive(
                request.timeout(timeout).send_bytes(&body),
                &worker_url,
                &key,
            )
        },
    )
    .map_err(|error| error.downcast::<Failure>().unwrap_or_else(Failure::local))?
}

fn connection_message(url: &str, kind: &str) -> String {
    let url = url::Url::parse(url).expect("HTTP request URL was validated");
    format!(
        "could not reach {}:{}: {kind}",
        url.host_str().unwrap_or_default(),
        url.port_or_known_default().unwrap_or(0)
    )
}

fn io_kind(error: &(dyn std::error::Error + 'static)) -> Option<std::io::ErrorKind> {
    if let Some(error) = error.downcast_ref::<std::io::Error>() {
        return Some(error.kind());
    }
    error.source().and_then(io_kind)
}

/// Provider error bodies are classified before validation and never persisted.
pub fn receive(
    result: std::result::Result<ureq::Response, ureq::Error>,
    url: &str,
    key: &str,
) -> std::result::Result<HttpResponse, Failure> {
    use std::io::Read;
    let response = match result {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(error)) => {
            let short = error.kind().to_string();
            let kind = match (error.kind(), io_kind(&error)) {
                (_, Some(std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock)) => {
                    "no answer within 600 s"
                }
                (_, Some(std::io::ErrorKind::ConnectionRefused)) => "connection refused",
                (ureq::ErrorKind::Dns, _) => "name not resolved",
                _ => error.message().unwrap_or(&short),
            };
            return Err(Failure::new(
                true,
                connection_message(url, &diagnostic(kind, key)),
            ));
        }
    };
    let status = response.status();
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|error| {
            let kind = match error.kind() {
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
                    "no answer within 600 s".into()
                }
                _ => diagnostic(&error.to_string(), key),
            };
            Failure::new(true, connection_message(url, &kind))
        })?;
    if body.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(Failure::local(
            "Service response exceeds the receiving limit",
        ));
    }
    Ok(HttpResponse { status, body })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_diagnostics_are_redacted_normalized_and_bounded_without_being_cached() {
        let _serial = runtime::lock_recover(&TEST_LOCK);
        let root = tempfile::tempdir().unwrap();
        let cache = install(root.path()).unwrap();
        let message = format!("key-123\n\tkey-123\0 {}", "界".repeat(400));
        let diagnostic = diagnostic(&message, "key-123");
        assert!(diagnostic.starts_with("[key] [key] "));
        assert_eq!(diagnostic.chars().count(), 300);
        assert!(diagnostic.ends_with('…'));
        let error = request(&Item::stage("test", ""), &|| Ok(()), || {
            cached_json_request(
                "http://localhost",
                &serde_json::json!({}),
                "key-123",
                "test",
                || {
                    Ok(response(
                        401,
                        serde_json::json!({"error":{"message":message}}),
                    ))
                },
                valid,
            )
        })
        .unwrap_err();
        assert_eq!(
            error.message,
            format!(
                "Stage test failed: {} The service said: {diagnostic}",
                rejection_message(401)
            )
        );
        assert!(!error.retryable);
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
        assert!(cache.take_warnings().is_empty());
    }

    #[test]
    fn http_attempt_deadline_bounds_blocked_dns_and_receiving_the_body() {
        use std::{
            io::{Read, Write},
            sync::mpsc,
        };
        assert_eq!(HTTP_TIMEOUT, Duration::from_secs(600));
        for dns in [false, true] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let (release, gate) = mpsc::channel();
            let (entered, ready) = mpsc::channel();
            let (finished, done) = mpsc::channel();
            let agent = if dns {
                let gate = Mutex::new(gate);
                ureq::AgentBuilder::new()
                    .resolver(move |_: &str| {
                        entered.send(()).unwrap();
                        runtime::lock_recover(&gate)
                            .recv_timeout(Duration::from_secs(5))
                            .unwrap();
                        finished.send(()).unwrap();
                        Ok(vec![address])
                    })
                    .build()
            } else {
                std::thread::spawn(move || {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut buffer = [0; 4096];
                    let _ = stream.read(&mut buffer).unwrap();
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{",
                        )
                        .unwrap();
                    entered.send(()).unwrap();
                    gate.recv_timeout(Duration::from_secs(5)).unwrap();
                    let _ = stream.write_all(b"}");
                    finished.send(()).unwrap();
                });
                http_agent()
            };
            let url = format!("http://localhost:{}", address.port());
            let start = Instant::now();
            let error = send_with_timeout(
                &|| Ok(()),
                agent.post(&url),
                Vec::new(),
                "",
                Duration::from_millis(100),
            )
            .err()
            .unwrap();
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(start.elapsed() < Duration::from_secs(1));
            assert!(error.retryable);
            assert_eq!(
                error.message,
                format!(
                    "could not reach localhost:{}: no answer within 600 s",
                    address.port()
                )
            );
            release.send(()).unwrap();
            done.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }

    #[test]
    fn malformed_speech_json_and_missing_text_are_retryable() {
        let _serial = runtime::lock_recover(&TEST_LOCK);
        for body in [b"not JSON".as_slice(), br#"{"text":null}"#] {
            let root = tempfile::tempdir().unwrap();
            let cache = install(root.path()).unwrap();
            let mut attempts = 0;
            let value = request(&Item::speech(61., 62.), &|| Ok(()), || {
                attempts += 1;
                json_request(
                    "",
                    "speech",
                    || {
                        Ok(if attempts == 1 {
                            HttpResponse {
                                status: 200,
                                body: body.to_vec(),
                            }
                        } else {
                            response(200, serde_json::json!({"text":""}))
                        })
                    },
                    valid,
                )
            })
            .unwrap();
            assert_eq!(value["text"], "");
            assert_eq!(attempts, 2);
            let warnings = cache.take_warnings();
            assert_eq!(warnings.len(), 1);
            assert!(warnings[0].starts_with("Speech at 01:01–01:02: attempt 1 of 3 failed: "));
        }
    }

    #[test]
    fn cancellation_during_backoff_stops_without_another_attempt() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let _serial = runtime::lock_recover(&TEST_LOCK);
        let cancelled = AtomicBool::new(false);
        let (entered, started) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let stop = &cancelled;
            scope.spawn(move || {
                started.recv_timeout(Duration::from_secs(5)).unwrap();
                std::thread::sleep(Duration::from_millis(50));
                stop.store(true, Ordering::Relaxed);
            });
            let mut attempts = 0;
            let start = Instant::now();
            let error = request(
                &Item::stage("cancelled", ""),
                &|| {
                    if cancelled.load(Ordering::Relaxed) {
                        return Err(runtime::Cancelled.into());
                    }
                    Ok(())
                },
                || {
                    attempts += 1;
                    entered.send(()).unwrap();
                    Err::<(), _>(Failure::new(true, "temporary failure"))
                },
            )
            .unwrap_err();
            assert_eq!(error.message, "Stage cancelled failed: Cancelled");
            assert!(!error.retryable);
            assert_eq!(attempts, 1);
            assert!(start.elapsed() < Duration::from_secs(1));
        });
    }

    #[test]
    fn stopped_answers_are_final_even_when_their_content_looks_complete() {
        for (value, reason) in [
            (
                serde_json::json!({"choices":[{"finish_reason":"length","message":{"content":"Complete"}}]}),
                "the model stopped at its output limit",
            ),
            (
                serde_json::json!({"choices":[{"finish_reason":"content_filter","message":{"content":"Complete"}}]}),
                "the model's content filter stopped the answer",
            ),
            (
                serde_json::json!({"error":{"code":"output_limit"}}),
                "the model stopped at its output limit",
            ),
        ] {
            let error = json_request(
                "",
                "stopped",
                || Ok(response(200, value)),
                check_finish_reason,
            )
            .unwrap_err();
            assert!(!error.retryable);
            assert_eq!(error.message, reason);
        }
    }

    #[test]
    fn output_limit_code_is_final_even_for_retryable_http_statuses() {
        for status in [400, 422, 429, 500] {
            let error = classify_rejection(
                status,
                br#"{"error":{"code":"output_limit","message":"x"}}"#,
                "",
            );
            assert_eq!(
                error.message,
                format!("{} The service said: x", rejection_message(status))
            );
            assert!(!error.retryable);
            assert!(!error.unsupported_response_format);
        }
    }
    #[test]
    fn rejections_without_output_limit_code_keep_the_existing_message() {
        for body in [
            r#"{"error":{"message":"output_limit"}}"#,
            r#"{"error":{"code":"other","message":"x"}}"#,
            "output_limit",
        ] {
            let error = classify_rejection(422, body.as_bytes(), "");
            assert!(
                error.message.starts_with(
                "The service rejected the request parameters (HTTP 422); check the model and service settings for this task.")
            );
            assert!(!error.retryable);
            assert!(!error.unsupported_response_format);
        }
    }
    #[test]
    fn response_format_fallback_uses_only_status_and_the_named_field() {
        for status in [400, 422] {
            for body in [
                r#"{"error":{"param":"response_format"}}"#,
                r#"{"detail":"response_format: format de sortie invalide"}"#,
                "response_format 不支持",
                "Invalid field: RESPONSE_FORMAT",
            ] {
                assert!(rejects_response_format(status, body.as_bytes()), "{body}");
            }
            for body in ["", "Invalid request parameters", "model is unsupported"] {
                assert!(!rejects_response_format(status, body.as_bytes()));
            }
        }
        for status in [200, 401, 403, 404, 429, 500] {
            assert!(!rejects_response_format(status, b"response_format"));
        }
    }
    fn response(status: u16, value: Value) -> HttpResponse {
        HttpResponse {
            status,
            body: serde_json::to_vec(&value).unwrap(),
        }
    }

    fn valid(value: &Value) -> Result<()> {
        anyhow::ensure!(value["text"].is_string(), "Missing text");
        Ok(())
    }

    #[test]
    fn cache_keys_include_the_url_and_body_and_only_store_valid_successes() {
        let _serial = runtime::lock_recover(&TEST_LOCK);
        let root = tempfile::tempdir().unwrap();
        let _cache = install(root.path()).unwrap();
        let body = serde_json::json!({"model":"test","messages":["Hello"]});
        let calls = std::cell::Cell::new(0);
        let send = || {
            calls.set(calls.get() + 1);
            Ok(response(200, serde_json::json!({"text":"response"})))
        };
        let url = "https://example.test/v1/chat/completions";
        let first = cached_json_request(url, &body, "", "test", send, valid).unwrap();
        assert_ne!(first[CACHED], true);
        let cached = cached_json_request(url, &body, "", "test", send, valid).unwrap();
        assert_eq!(cached[CACHED], true);
        assert_eq!(calls.get(), 1);
        cached_json_request(
            "https://other.test/v1/chat/completions",
            &body,
            "",
            "test",
            send,
            valid,
        )
        .unwrap();
        cached_json_request(
            url,
            &serde_json::json!({"model":"other"}),
            "",
            "test",
            send,
            valid,
        )
        .unwrap();
        assert_eq!(calls.get(), 3);
        let key = runtime::digest(format!("{url}\n{body}").as_bytes());
        let saved: Value = serde_json::from_slice(
            &std::fs::read(root.path().join("llm").join(format!("{key}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(saved, first);
        assert!(saved.get(CACHED).is_none());
        assert!(!root.path().join("requests").exists());
    }

    #[test]
    fn failed_or_invalid_responses_never_enter_the_cache_and_do_not_block_a_retry() {
        let _serial = runtime::lock_recover(&TEST_LOCK);
        let root = tempfile::tempdir().unwrap();
        let _cache = install(root.path()).unwrap();
        let body = serde_json::json!({"model":"test"});
        for status in [302, 400, 401, 422, 429, 500, 503, 599] {
            let error = cached_json_request(
                "https://example.test",
                &body,
                "private-test-key",
                "test",
                || {
                    Ok(response(
                        status,
                        serde_json::json!({"error":{"message":"private-test-key"}}),
                    ))
                },
                valid,
            )
            .unwrap_err();
            assert_eq!(error.retryable, status == 429 || status >= 500);
            assert!(!error.to_string().contains("private-test-key"));
            assert!(!root.path().join("llm").exists());
        }
        for value in [
            serde_json::json!({"error":"private-test-key"}),
            serde_json::json!({"text":null}),
        ] {
            assert!(
                cached_json_request(
                    "https://example.test",
                    &body,
                    "private-test-key",
                    "test",
                    || Ok(response(200, value)),
                    valid
                )
                .is_err()
            );
            assert!(!root.path().join("llm").exists());
        }
        assert!(
            cached_json_request(
                "https://example.test",
                &body,
                "private-test-key",
                "test",
                || Err(Failure::new(true, "timeout")),
                valid
            )
            .is_err()
        );
        assert!(!root.path().join("llm").exists());
        cached_json_request(
            "https://example.test",
            &body,
            "private-test-key",
            "test",
            || Ok(response(200, serde_json::json!({"text":"valid"}))),
            valid,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_dir(root.path().join("llm")).unwrap().count(),
            1
        );
    }

    #[test]
    fn an_abandoned_http_response_never_enters_the_cache() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};
        let _serial = runtime::lock_recover(&TEST_LOCK);
        let root = tempfile::tempdir().unwrap();
        let _cache = install(root.path()).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let (release, gate) = std::sync::mpsc::channel();
        let mock = crate::test_support::MockAi::respond_with(move |_, _| {
            cancel.store(true, Ordering::Relaxed);
            gate.recv_timeout(Duration::from_secs(5)).unwrap();
            Some((200, serde_json::json!({"text":"Late success"})))
        });
        let (received, late) = std::sync::mpsc::channel();
        let request = ureq::post(&mock.url);
        let start = Instant::now();
        let result = cached_json_request(
            &mock.url,
            &serde_json::json!({}),
            "",
            "test",
            || {
                runtime::interruptible_http(
                    &|| {
                        if cancelled.load(Ordering::Relaxed) {
                            return Err(runtime::Cancelled.into());
                        }
                        Ok(())
                    },
                    move || {
                        let response = receive(
                            request.send_json(serde_json::json!({})),
                            "http://localhost",
                            "",
                        );
                        let _ = received.send(());
                        response
                    },
                )
                .map_err(Failure::local)?
            },
            valid,
        );
        assert!(result.unwrap_err().to_string().contains("Cancelled"));
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!root.path().join("llm").exists());
        release.send(()).unwrap();
        late.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!root.path().join("llm").exists());
    }

    #[test]
    fn receiving_preserves_the_response_cap_and_safe_transport_errors() {
        let oversized = "x".repeat(MAX_RESPONSE_BYTES as usize + 1);
        let response = ureq::Response::new(200, "OK", &oversized).unwrap();
        let error = receive(Ok(response), "http://localhost", "").err().unwrap();
        assert!(!error.retryable);
        assert!(error.to_string().contains("receiving limit"));
        let error = receive(
            ureq::get("invalid private-test-key").call(),
            "http://localhost",
            "private-test-key",
        )
        .err()
        .unwrap();
        assert!(error.retryable);
        assert!(!error.to_string().contains("private-test-key"));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let error = receive(
            runtime::http_agent(std::time::Duration::from_secs(1))
                .get(&format!("http://{address}"))
                .call(),
            &format!("http://{address}"),
            "",
        )
        .err()
        .unwrap();
        assert!(error.retryable);
        assert_eq!(
            error.message,
            format!("could not reach {address}: connection refused")
        );
        let agent = ureq::AgentBuilder::new()
            .resolver(|_: &str| Err(std::io::Error::from(std::io::ErrorKind::NotFound)))
            .build();
        let error = receive(
            agent.get("http://provider.invalid:1234").call(),
            "http://provider.invalid:1234",
            "",
        )
        .err()
        .unwrap();
        assert!(error.retryable);
        assert_eq!(
            error.message,
            "could not reach provider.invalid:1234: name not resolved"
        );
    }
}
