//! Progress output. With --json, stdout carries one NDJSON event per line for other programs (see docs/cli.md).
//!
//! The only event types are `log`, `stage`, `progress`, `tokens`, `done`, `error`.
//! - `log`: level, message
//! - `stage`: stage, status (start/done)
//! - `progress`: stage, current, total, optional message
//! - `tokens`: stage, prompt, completion
//! - `done`: note_dir, markdown, json, warnings
//! - `error`: message

use indicatif::{ProgressBar, ProgressStyle};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static QUIET_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_quiet(quiet: bool) {
    QUIET_MODE.store(quiet, Ordering::Relaxed);
}
pub fn is_quiet() -> bool {
    QUIET_MODE.load(Ordering::Relaxed)
}

static JSON_MODE: AtomicBool = AtomicBool::new(false);

/// Switches the process to NDJSON events; called once after parsing the command line.
pub fn set_json_mode() {
    JSON_MODE.store(true, Ordering::Relaxed);
}

pub fn is_json() -> bool {
    JSON_MODE.load(Ordering::Relaxed)
}

/// Writes one event line to stdout and flushes it; does nothing outside JSON mode.
pub fn emit(ev: serde_json::Value) {
    if !is_json() {
        return;
    }
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = emit_to(&mut lock, &ev);
}

pub fn done(note_dir: &std::path::Path, warnings: &[String]) {
    emit(
        serde_json::json!({"type":"done","note_dir":note_dir,"markdown":note_dir.join("note.md"),"json":note_dir.join("note.json"),"warnings":warnings}),
    );
}

fn emit_to(mut w: impl std::io::Write, ev: &serde_json::Value) -> std::io::Result<()> {
    writeln!(w, "{ev}")?;
    w.flush()
}

/// Emits a stage boundary. Token counts restart with each stage,
/// so they always describe the current stage.
pub fn stage(name: &str, status: &str) {
    if status == "start" {
        reset_tokens();
    }
    emit(stage_event(name, status));
    if !is_json() && !is_quiet() && status == "start" {
        eprintln!("Running {name}…");
    }
}

pub fn stage_label(name: &str, status: &str, label: &str) {
    if status == "start" {
        reset_tokens();
    }
    emit(stage_event(name, status));
    if !is_json() && !is_quiet() && status == "start" {
        eprintln!("{label}…");
    }
}

fn stage_event(name: &str, status: &str) -> serde_json::Value {
    serde_json::json!({"type": "stage", "stage": name, "status": status})
}

fn progress_event(stage: &str, current: u64, total: u64, message: &str) -> serde_json::Value {
    let mut ev = serde_json::json!({
        "type": "progress",
        "stage": stage,
        "current": current,
        "total": total,
    });
    if !message.is_empty() {
        ev["message"] = serde_json::Value::String(message.to_string());
    }
    ev
}

pub(crate) fn log_event(level: &str, message: &str) -> serde_json::Value {
    serde_json::json!({"type": "log", "level": level, "message": message})
}

/// Token usage of the current stage, summed across concurrent workers,
/// so the reported value only grows; [`stage`] resets it.
static TOKENS: Mutex<(u64, u64)> = Mutex::new((0, 0));

fn reset_tokens() {
    *crate::runtime::lock_recover(&TOKENS) = (0, 0);
}

fn tokens_event(stage: &str, prompt: u64, completion: u64) -> serde_json::Value {
    serde_json::json!({"type": "tokens", "stage": stage, "prompt": prompt, "completion": completion})
}

/// Adds one response's token usage and emits the running total in JSON mode.
pub fn note_tokens(stage: &str, prompt: u64, completion: u64) {
    if prompt == 0 && completion == 0 {
        return;
    }
    let mut guard = crate::runtime::lock_recover(&TOKENS);
    guard.0 += prompt;
    guard.1 += completion;
    emit(tokens_event(stage, guard.0, guard.1));
}

/// Emit throttled source-adapter download progress.
pub fn download_progress(stage: &str, current: u64, total: u64) {
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let mut last = crate::runtime::lock_recover(&LAST);
    if current != total && last.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(200))
    {
        return;
    }
    *last = Some(std::time::Instant::now());
    drop(last);
    emit(progress_event(stage, current, total, ""));
}

/// Shared progress bar style; the templates are constants, so a parse failure is a bug.
fn progress_style(template: &str) -> ProgressStyle {
    ProgressStyle::with_template(template)
        .expect("Static progress bar template")
        .progress_chars("##-")
}

/// A progress bar: visible in a terminal; in JSON mode it is hidden
/// and its updates become progress events.
pub struct Bar {
    stage: String,
    bar: ProgressBar,
    current: AtomicU64,
    total: u64,
    message: Mutex<String>,
    last_emit: Mutex<std::time::Instant>,
}

impl Bar {
    pub fn new(stage: impl Into<String>, len: u64) -> Self {
        let bar = if is_json() || is_quiet() {
            ProgressBar::hidden()
        } else {
            ProgressBar::new(len)
        };
        Self {
            stage: stage.into(),
            bar,
            current: AtomicU64::new(0),
            total: len,
            message: Mutex::new(String::new()),
            last_emit: Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(1)),
        }
    }

    /// Applies the terminal template; hidden bars in JSON mode skip it.
    pub fn with_template(self, template: &str) -> Self {
        if !is_json() {
            self.bar.set_style(progress_style(template));
        }
        self
    }

    pub fn inc(&self, n: u64) {
        self.bar.inc(n);
        if is_json() {
            let cur = self.current.fetch_add(n, Ordering::Relaxed) + n;
            self.emit_progress(cur, false);
        }
    }

    pub fn set_position(&self, pos: u64) {
        self.bar.set_position(pos);
        if is_json() {
            self.current.store(pos, Ordering::Relaxed);
            self.emit_progress(pos, false);
        }
    }

    pub fn set_message(&self, msg: impl Into<String>) {
        let msg = msg.into();
        if is_json() {
            *crate::runtime::lock_recover(&self.message) = msg;
            let cur = self.current.load(Ordering::Relaxed);
            self.emit_progress(cur, false);
        } else {
            self.bar.set_message(msg);
        }
    }

    pub fn finish(&self) {
        if is_json() {
            self.emit_progress(self.current.load(Ordering::Relaxed), true);
        }
        self.bar.finish_and_clear();
    }

    fn emit_progress(&self, current: u64, force: bool) {
        let mut last = crate::runtime::lock_recover(&self.last_emit);
        if !force
            && current != 0
            && current != self.total
            && last.elapsed() < std::time::Duration::from_millis(100)
        {
            return;
        }
        *last = std::time::Instant::now();
        let msg = self.message.lock().map(|m| m.clone()).unwrap_or_default();
        emit(progress_event(&self.stage, current, self.total, &msg));
    }
}

/// tracing layer that writes log records as `{"type":"log"}` lines to stdout.
/// Installed only in JSON mode; levels map to error, warn, info and debug (trace counts as debug).
pub struct JsonLogLayer;

pub fn json_log_layer() -> JsonLogLayer {
    JsonLogLayer
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for JsonLogLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = LogVisitor::default();
        event.record(&mut visitor);
        let level = match *event.metadata().level() {
            tracing::Level::ERROR => "error",
            tracing::Level::WARN => "warn",
            tracing::Level::INFO => "info",
            _ => "debug",
        };
        emit(log_event(level, &visitor.into_message()));
    }
}

/// The event's message, or its other fields as `k=v` text when it has none.
#[derive(Default)]
struct LogVisitor {
    message: Option<String>,
    fields: Vec<String>,
}

impl LogVisitor {
    fn push(&mut self, field: &tracing::field::Field, value: String) {
        if field.name() == "message" {
            self.message = Some(value);
        } else {
            self.fields.push(format!("{}={value}", field.name()));
        }
    }

    fn into_message(self) -> String {
        self.message.unwrap_or_else(|| self.fields.join(" "))
    }
}

impl tracing::field::Visit for LogVisitor {
    // Display, Debug and numeric values all arrive through record_debug;
    // the Debug output of fmt::Arguments is the formatted text without quotes.
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.push(field, format!("{value:?}"));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.push(field, value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visitor_message_fallback_to_fields() {
        let v = LogVisitor {
            message: Some("hello".into()),
            fields: vec!["k=v".into()],
        };
        assert_eq!(v.into_message(), "hello");

        let mut v = LogVisitor::default();
        v.fields.push("a=1".into());
        v.fields.push("b=2".into());
        assert_eq!(v.into_message(), "a=1 b=2");

        assert_eq!(LogVisitor::default().into_message(), "");
    }

    #[test]
    fn emit_to_writes_single_ndjson_line() {
        let mut buf: Vec<u8> = Vec::new();
        let ev = serde_json::json!({"type": "log", "level": "info", "message": "hi"});
        emit_to(&mut buf, &ev).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.ends_with('\n'));
        assert_eq!(s.trim().parse::<serde_json::Value>().unwrap(), ev);
    }
}
