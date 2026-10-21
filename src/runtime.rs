//! Process guards, bounded commands and interruptible HTTP readiness polling.
//! - `ManagedChild`: killed and reaped on drop, so no error path (including `?`) leaks a process
//! - `run_bounded`: timeouts and output capture for short commands
//! - `wait_ready`: health polling that fails at once if the child exits
//! - `retry_backoff` / `interruptible_sleep`: cancellable retry delays

use anyhow::{Context, Result};
#[cfg(windows)]
mod windows;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Locks a mutex, taking the data even if another thread panicked while holding it.
/// Every critical section here only updates in-memory state, so a panic cannot leave the data
/// inconsistent, and propagating PoisonError would only bring down unrelated threads.
pub fn lock_recover<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Runs an async block on a temporary Tokio runtime.
pub fn block_on<F: std::future::Future>(f: F) -> Result<F::Output> {
    let runtime = tokio::runtime::Runtime::new()?;
    let result = runtime.block_on(async {
        tokio::select! {
            error = watch_cancelled() => Err(error),
            value = f => Ok(value),
        }
    });
    // Finish adapter cleanup without waiting indefinitely for blocking HTTP reads.
    if is_cancelled() {
        runtime.shutdown_timeout(Duration::from_secs(4));
    }
    result
}

static CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(unix)]
static PROCESS_TREES: Mutex<Vec<u32>> = Mutex::new(Vec::new());
#[cfg(windows)]
static PROCESS_TREES: Mutex<Vec<(u32, windows::Job)>> = Mutex::new(Vec::new());

#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cancelled")
    }
}
impl std::error::Error for Cancelled {}

pub fn is_cancelled() -> bool {
    CANCELLED.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn check_cancelled() -> Result<()> {
    if is_cancelled() {
        return Err(Cancelled.into());
    }
    Ok(())
}

pub async fn watch_cancelled() -> anyhow::Error {
    loop {
        if let Err(error) = check_cancelled() {
            return error;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn cancel_or_exit() {
    if CANCELLED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        let mut trees = lock_recover(&PROCESS_TREES);
        #[cfg(unix)]
        for pid in trees.drain(..) {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        trees.clear(); // Closing each job kills its whole process tree.
        std::process::exit(130);
    }
}

pub fn install_signal_handlers() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        tokio::spawn(async move {
            loop {
                tokio::select! { _ = interrupt.recv() => {}, _ = terminate.recv() => {} }
                cancel_or_exit();
            }
        });
    }
    #[cfg(windows)]
    {
        let mut interrupt = tokio::signal::windows::ctrl_c()?;
        tokio::spawn(async move {
            while interrupt.recv().await.is_some() {
                cancel_or_exit();
            }
        });
    }
    Ok(())
}

/// A child process that is killed and reaped when dropped.
pub struct ManagedChild {
    child: Child,
    name: &'static str,
    process_tree: bool,
}

pub(crate) struct CancelOnDrop(pub std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsRawHandle for ManagedChild {
    fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
        self.child.as_raw_handle()
    }
}

impl ManagedChild {
    pub fn spawn(name: &'static str, cmd: &mut Command) -> Result<Self> {
        let child = cmd
            .spawn()
            .with_context(|| format!("Could not start {}", name))?;
        Ok(Self {
            child,
            name,
            process_tree: false,
        })
    }

    /// Own the whole process tree, including workers spawned by the launcher.
    pub(crate) fn spawn_tree(name: &'static str, command: &mut Command) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        let job = {
            use std::os::windows::process::CommandExt;
            command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
            windows::Job::new()?
        };
        // Register under the same lock as forced shutdown, including the spawn window.
        let mut trees = lock_recover(&PROCESS_TREES);
        check_cancelled()?;
        let mut child = Self::spawn(name, command)?;
        #[cfg(windows)]
        {
            job.assign_and_resume(&child)?;
            trees.push((child.id(), job));
        }
        #[cfg(unix)]
        trees.push(child.id());
        child.process_tree = true;
        Ok(child)
    }

    pub(crate) fn stop(&mut self) {
        if self.process_tree {
            #[cfg(unix)]
            {
                let group = -(self.id() as i32);
                unsafe {
                    libc::kill(group, libc::SIGTERM);
                }
                // Allow graceful shutdown even when cancellation has already arrived.
                let start = Instant::now();
                while self.try_wait().is_none() && start.elapsed() < Duration::from_secs(2) {
                    std::thread::sleep(WAIT_POLL_INTERVAL);
                }
                let mut trees = lock_recover(&PROCESS_TREES);
                // The leader may exit before its children; terminate the entire group.
                unsafe {
                    libc::kill(group, libc::SIGKILL);
                }
                trees.retain(|pid| *pid != self.id());
            }
            #[cfg(windows)]
            lock_recover(&PROCESS_TREES).retain(|(pid, _)| *pid != self.id());
            self.process_tree = false;
        }
        self.kill();
        let _ = self.wait();
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Take the stderr pipe for background draining.
    pub fn take_stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child.stderr.take()
    }

    pub fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child.stdout.take()
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Non-blocking reap; `Some` once the process has exited.
    pub fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// Waits up to `timeout` for the process to exit; an error means it is still running (Drop will kill it).
    pub fn wait_within(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let t0 = Instant::now();
        loop {
            check_cancelled()?;
            if let Some(status) = self.try_wait() {
                return Ok(status);
            }
            anyhow::ensure!(
                t0.elapsed() < timeout,
                "{} did not exit within the time limit ({:.1}s)",
                self.name,
                timeout.as_secs_f64()
            );
            std::thread::sleep(WAIT_POLL_INTERVAL);
        }
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }

    pub fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.child.wait()
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Reads a child's stderr in the background so a full pipe never blocks it, keeps the last lines
/// for error messages, and forwards each line to debug logging.
#[derive(Clone, Default)]
pub struct StderrTail {
    lines: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

const STDERR_TAIL_MAX: usize = 100;
/// Polling interval of wait_within.
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(50);

impl StderrTail {
    pub fn tail(&self) -> String {
        self.lines.lock().map(|v| v.join("\n")).unwrap_or_default()
    }

    /// Adds the stderr tail to an error as context, if there is any.
    pub fn attach(&self, e: anyhow::Error, label: &str) -> anyhow::Error {
        let tail = self.tail();
        if tail.is_empty() {
            e
        } else {
            e.context(format!("{label}:\n{tail}"))
        }
    }
}

impl StderrTail {
    pub fn drain(
        &self,
        reader: impl std::io::Read + Send + 'static,
        target: &'static str,
    ) -> std::thread::JoinHandle<()> {
        use std::io::BufRead;
        let shared = self.lines.clone();
        std::thread::spawn(move || {
            let reader = std::io::BufReader::new(reader);
            for line in reader.lines().map_while(Result::ok) {
                // tracing targets must be constants, so the source name goes into the message.
                tracing::debug!("[{target}] {line}");
                if let Ok(mut v) = shared.lock()
                    && !line.trim().is_empty()
                {
                    v.push(line);
                    let overflow = v.len().saturating_sub(STDERR_TAIL_MAX);
                    if overflow > 0 {
                        v.drain(..overflow);
                    }
                }
            }
        })
    }
}

/// Output of a bounded command; stdout and stderr are captured through temporary files, not pipes.
pub struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a short command with stdin closed (ffmpeg can hang on an inherited stdin),
/// kills it at the timeout, and captures output in temporary files so a full pipe cannot stall it.
pub fn run_bounded(
    name: &'static str,
    cmd: &mut Command,
    timeout: Duration,
) -> Result<BoundedOutput> {
    use std::io::{Read, Seek};
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    cmd.stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);
    let mut child = ManagedChild::spawn_tree(name, cmd)?;
    let status = child.wait_within(timeout);
    child.stop();
    stdout.rewind()?;
    stderr.rewind()?;
    let mut out = Vec::new();
    let mut err = Vec::new();
    stdout.take(64 * 1024 * 1024).read_to_end(&mut out)?;
    stderr.take(64 * 1024 * 1024).read_to_end(&mut err)?;
    let out = String::from_utf8_lossy(&out).into_owned();
    let err = String::from_utf8_lossy(&err).into_owned();
    let status = status
        .with_context(|| format!("{name} was stopped:\n{}", crate::error::tail_lines(&err, 5)))?;
    Ok(BoundedOutput {
        status,
        stdout: out,
        stderr: err,
    })
}

/// Polls `{base}/health` until it succeeds; fails at once if the child exits.
/// `control`, when given, runs before each poll; an error from it (cancellation) is returned at once.
/// The child is killed when its ManagedChild is dropped.
pub fn wait_ready(
    base: &str,
    timeout: Duration,
    child: &mut ManagedChild,
    control: Option<&dyn Fn() -> Result<()>>,
) -> Result<()> {
    let t0 = Instant::now();
    let url = format!("{base}/health");
    loop {
        if let Some(check) = control {
            // Return cancellation unchanged so callers can tell it apart from a failed start.
            check()?;
        }
        if let Some(st) = child.try_wait() {
            anyhow::bail!("{} exited during startup ({st})", child.name());
        }
        if t0.elapsed() > timeout {
            anyhow::bail!(
                "{} startup timed out after {:.0}s",
                child.name(),
                timeout.as_secs_f64()
            );
        }
        if let Ok(resp) = http_agent(Duration::from_secs(2)).get(&url).call()
            && resp.status() == 200
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Per-process jitter, mixed with the clock, so concurrent requests do not retry in lockstep.
static JITTER_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Delay after failed attempt `attempt`: 1 s, 2 s, 4 s … plus up to 500 ms of jitter.
pub fn retry_backoff(attempt: u32) -> Duration {
    let base = 1_u64 << attempt.saturating_sub(1).min(6);
    let seq = JITTER_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()))
        .unwrap_or(0);
    let jitter_ns = (nanos ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15)) % 500_000_000;
    Duration::from_nanos(base * 1_000_000_000 + jitter_ns)
}

/// A sleep that checks for cancellation in small steps and returns its error at once.
pub fn interruptible_sleep(wait: Duration) -> Result<()> {
    let deadline = Instant::now() + wait;
    loop {
        crate::runtime::check_cancelled()?;
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        std::thread::sleep(left.min(Duration::from_millis(200)));
    }
}

/// HTTP agent that never follows redirects, with the given overall timeout.
///
/// Shared HTTP policy: callers classify responses and follow no redirects.
pub fn http_agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(timeout)
        .redirects(0)
        .build()
}

/// Finds an executable on PATH.
/// Uses the `which` crate, which handles Windows PATHEXT (.bat, .cmd, …).
pub fn which(cmd: &str) -> Option<PathBuf> {
    which::which(cmd).ok()
}

/// Own an OS lock separately from the lifetime of duplicated file descriptors.
pub struct FileLock {
    file: std::fs::File,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Unix flock locks belong to the open file description. A concurrently
        // forked child may still hold that description until exec, so closing
        // our descriptor alone need not release the lock at the end of its scope.
        let _ = self.file.unlock();
    }
}

/// Hold an OS file lock until the returned guard is dropped. The lock file
/// remains in place so another process cannot open a different inode at its path.
pub fn lock_file(path: &Path) -> Result<FileLock> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.try_lock().with_context(|| {
        format!(
            "Directory is in use: {}. Wait for the task to finish.",
            path.display()
        )
        .to_string()
    })?;
    Ok(FileLock { file })
}

/// Asks the operating system for a free port.
/// Another process may take the port before the caller binds it,
/// Readiness polling detects a failed child bind.
pub fn free_port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// Runs `work` for every item on a bounded number of OS threads, preserving
/// input order. Workers stop taking new items after the first error, but every
/// started item is joined so confirmed side effects can still be saved.
pub fn map_pool<T, R>(
    items: Vec<T>,
    workers: usize,
    work: impl Fn(T) -> Result<R> + Sync,
) -> Result<Vec<R>>
where
    T: Send,
    R: Send,
{
    let total = items.len();
    let queue = std::sync::Mutex::new(items.into_iter().enumerate());
    let results: std::sync::Mutex<Vec<Option<R>>> =
        std::sync::Mutex::new(std::iter::repeat_with(|| None).take(total).collect());
    let first_error = std::sync::Mutex::new(None::<anyhow::Error>);
    std::thread::scope(|scope| {
        for _ in 0..workers.max(1).min(total.max(1)) {
            scope.spawn(|| {
                loop {
                    if let Err(error) = crate::runtime::check_cancelled() {
                        let mut guard = lock_recover(&first_error);
                        if guard.is_none() {
                            *guard = Some(error);
                        }
                        break;
                    }
                    let next = lock_recover(&queue).next();
                    let Some((index, item)) = next else { break };
                    if lock_recover(&first_error).is_some() {
                        break;
                    }
                    match work(item) {
                        Ok(value) => lock_recover(&results)[index] = Some(value),
                        Err(error) => {
                            let mut guard = lock_recover(&first_error);
                            if guard.is_none() {
                                *guard = Some(error);
                            }
                            break;
                        }
                    }
                }
            });
        }
    });
    if let Some(error) = lock_recover(&first_error).take() {
        return Err(error);
    }
    Ok(lock_recover(&results)
        .iter_mut()
        .map(|value| value.take().expect("worker filled every index"))
        .collect())
}

pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

pub fn file_digest(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file =
        std::fs::File::open(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn bounded_timeout_stops_the_process_tree_and_keeps_the_stderr_tail() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("child-pid");
        let error = run_bounded(
            "on_complete",
            Command::new("sh")
                .args([
                    "-c",
                    r#"trap 'wait; exit 0' TERM
sleep 30 &
printf '%s' "$!" > "$1"
printf 'hook-timeout-diagnostic\n' >&2
wait
"#,
                    "hook",
                ])
                .arg(&pid_file),
            Duration::from_millis(300),
        )
        .err()
        .expect("the command must time out");
        let message = format!("{error:#}");
        assert!(message.contains("was stopped") && message.contains("time limit"));
        assert!(message.contains("hook-timeout-diagnostic"));
        let pid = std::fs::read_to_string(pid_file)
            .unwrap()
            .parse::<i32>()
            .unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "hook child was orphaned");
    }

    #[test]
    fn workspace_lock_rejects_a_second_writer_and_releases_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".lock");
        let first = lock_file(&path).unwrap();
        assert!(lock_file(&path).is_err());
        drop(first);
        assert!(lock_file(&path).is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn workspace_lock_releases_while_an_inherited_descriptor_remains_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".lock");
        let first = lock_file(&path).unwrap();
        // dup and fork both retain the same open file description. Keeping a
        // duplicate makes the fork-before-exec window deterministic, without
        // forking the multithreaded test runner or depending on scheduler timing.
        let inherited = first.file.try_clone().unwrap();
        assert!(lock_file(&path).is_err());
        drop(first);
        let second = lock_file(&path).expect("scope exit must release the original lock");
        drop(inherited);
        assert!(
            lock_file(&path).is_err(),
            "the new owner must keep its lock"
        );
        drop(second);
        assert!(lock_file(&path).is_ok());
    }

    /// A cancelled run must not keep waiting for an adapter to start.
    #[cfg(unix)]
    #[test]
    fn wait_ready_aborts_when_control_hook_fails() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let mut child = ManagedChild::spawn("sleep", &mut cmd).unwrap();
        let t0 = Instant::now();
        // A port that never becomes ready and a 60 s timeout; control fails on the second poll, long before.
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let result = wait_ready(
            "http://127.0.0.1:9",
            Duration::from_secs(60),
            &mut child,
            Some(&|| {
                if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 1 {
                    anyhow::bail!("test cancellation")
                } else {
                    Ok(())
                }
            }),
        );
        let err = result.expect_err("a control error stops wait_ready");
        assert_eq!(
            err.to_string(),
            "test cancellation",
            "the control error is returned unchanged"
        );
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "cancellation does not wait for the timeout"
        );
    }

    #[test]
    fn retry_backoff_grows_exponentially_with_bounded_jitter() {
        for (attempt, base) in [(1, 1.0), (2, 2.0), (3, 4.0)] {
            let wait = retry_backoff(attempt).as_secs_f64();
            assert!((base..base + 0.5).contains(&wait), "{attempt}: {wait}");
        }
    }

    /// Error paths and panics must also kill and reap the child.
    #[cfg(unix)]
    #[test]
    fn managed_child_drop_kills_and_reaps() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let child = ManagedChild::spawn("sleep", &mut cmd).unwrap();
        let pid = child.id().to_string();
        drop(child);
        // kill -0 only checks existence: Drop has killed and reaped the process.
        let st = Command::new("kill").args(["-0", &pid]).status().unwrap();
        assert!(!st.success(), "pid {pid} was cleaned up by Drop");
    }
}
