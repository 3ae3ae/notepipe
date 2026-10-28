//! Validated HTTP requests and a success-only cache for LLM responses.

use crate::{checkpoint::atomic_write, runtime};
use anyhow::Result;
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;
pub const CACHED: &str = "notepipe_cached";

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
fn classify_rejection(status: u16, body: &[u8]) -> Failure {
    Failure {
        retryable: status == 429 || (500..=599).contains(&status),
        unsupported_response_format: rejects_response_format(status, body),
        message: if rejects_output_limit(body) {
            "The model stopped at its output limit.".into()
        } else {
            rejection_message(status)
        },
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

/// Classify the named request field without retaining provider error text.
fn rejects_response_format(status: u16, body: &[u8]) -> bool {
    matches!(status, 400 | 422)
        && body
            .windows(b"response_format".len())
            .any(|part| part.eq_ignore_ascii_case(b"response_format"))
}

/// The adapter contract's code for a result cut off at the model's output limit; provider text is still not kept.
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
}
static ACTIVE: OnceLock<Mutex<Option<Arc<Cache>>>> = OnceLock::new();
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

pub struct Guard(Arc<Cache>);
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
    send: impl FnOnce() -> std::result::Result<HttpResponse, Failure>,
    validate: impl Fn(&Value) -> Result<()>,
) -> std::result::Result<Value, Failure> {
    let cache = runtime::lock_recover(ACTIVE.get_or_init(|| Mutex::new(None))).clone();
    let Some(cache) = cache else {
        return json_request(send, validate);
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
    let value = json_request(send, validate)?;
    runtime::check_cancelled().map_err(Failure::local)?;
    atomic_write(&path, &serde_json::to_vec(&value).map_err(Failure::local)?)
        .map_err(Failure::local)?;
    Ok(value)
}

pub fn json_request(
    send: impl FnOnce() -> std::result::Result<HttpResponse, Failure>,
    validate: impl FnOnce(&Value) -> Result<()>,
) -> std::result::Result<Value, Failure> {
    runtime::check_cancelled().map_err(Failure::local)?;
    let response = send()?;
    runtime::check_cancelled().map_err(Failure::local)?;
    if !(200..300).contains(&response.status) {
        return Err(classify_rejection(response.status, &response.body));
    }
    let value = serde_json::from_slice(&response.body)
        .map_err(|_| Failure::local("The service response is not valid JSON"))?;
    // A validator may return a retryable Failure; any other error is final.
    validate(&value).map_err(|error| error.downcast::<Failure>().unwrap_or_else(Failure::local))?;
    Ok(value)
}

/// Provider error bodies are used only for safe classification and are never persisted.
pub fn receive(
    result: std::result::Result<ureq::Response, ureq::Error>,
) -> std::result::Result<HttpResponse, Failure> {
    use std::io::Read;
    let response = match result {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(error)) => {
            return Err(Failure::new(
                matches!(
                    error.kind(),
                    ureq::ErrorKind::Dns
                        | ureq::ErrorKind::ConnectionFailed
                        | ureq::ErrorKind::Io
                        | ureq::ErrorKind::ProxyConnect
                ),
                "Could not complete the service connection",
            ));
        }
    };
    let status = response.status();
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|_| Failure::new(true, "Service response was interrupted"))?;
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
    fn output_limit_code_uses_a_fixed_message_without_changing_retryability() {
        for status in [400, 422, 429, 500] {
            let error = classify_rejection(
                status,
                br#"{"error":{"code":"output_limit","message":"x"}}"#,
            );
            assert_eq!(error.message, "The model stopped at its output limit.");
            assert_eq!(error.retryable, status == 429 || status >= 500);
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
            let error = classify_rejection(422, body.as_bytes());
            assert_eq!(
                error.message,
                "The service rejected the request parameters (HTTP 422); check the model and service settings for this task."
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
        let first = cached_json_request(url, &body, send, valid).unwrap();
        assert_ne!(first[CACHED], true);
        let cached = cached_json_request(url, &body, send, valid).unwrap();
        assert_eq!(cached[CACHED], true);
        assert_eq!(calls.get(), 1);
        cached_json_request("https://other.test/v1/chat/completions", &body, send, valid).unwrap();
        cached_json_request(url, &serde_json::json!({"model":"other"}), send, valid).unwrap();
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
                || Err(Failure::new(true, "timeout")),
                valid
            )
            .is_err()
        );
        assert!(!root.path().join("llm").exists());
        cached_json_request(
            "https://example.test",
            &body,
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
            || {
                runtime::interruptible_http(
                    &|| {
                        if cancelled.load(Ordering::Relaxed) {
                            return Err(runtime::Cancelled.into());
                        }
                        Ok(())
                    },
                    move || {
                        let response = receive(request.send_json(serde_json::json!({})));
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
        let error = receive(Ok(response)).err().unwrap();
        assert!(!error.retryable);
        assert!(error.to_string().contains("receiving limit"));
        let error = receive(ureq::get("invalid private-test-key").call())
            .err()
            .unwrap();
        assert!(!error.retryable);
        assert!(!error.to_string().contains("private-test-key"));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        assert!(
            receive(
                runtime::http_agent(std::time::Duration::from_secs(1))
                    .get(&format!("http://{address}"))
                    .call()
            )
            .err()
            .unwrap()
            .retryable
        );
    }
}
