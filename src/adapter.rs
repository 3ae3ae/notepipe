//! Speech adapters: start one for the run, wait for its health check, and stop its process tree.

use anyhow::{Context, Result};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// First runs may download models before serving /health.
const ADAPTER_READY_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub struct Adapter {
    child: crate::runtime::ManagedChild,
    pub stderr: crate::runtime::StderrTail,
    pub base: String,
    drains: Vec<std::thread::JoinHandle<()>>,
    stopped: bool,
}

/// Shared by request workers; resolving the URL starts the program on the first cache miss.
pub struct Provider {
    name: String,
    argv: Vec<String>,
    adapter: std::sync::Mutex<Option<Adapter>>,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Provider {
    pub fn new(name: String, argv: Vec<String>) -> Self {
        Self {
            name,
            argv,
            adapter: Default::default(),
        }
    }

    pub fn base(&self, stage: &str, control: &dyn Fn() -> Result<()>) -> Result<String> {
        let mut adapter = crate::runtime::lock_recover(&self.adapter);
        control()?;
        if adapter.is_none() {
            *adapter = Some(Adapter::start_with_control(
                &self.argv, &self.name, stage, control,
            )?);
        }
        Ok(format!("{}/v1", adapter.as_ref().unwrap().base))
    }

    pub fn check(&self) -> Result<()> {
        // Startup holds the lock while polling health; it already watches the child.
        if let Ok(mut adapter) = self.adapter.try_lock()
            && let Some(adapter) = adapter.as_mut()
            && let Some(status) = adapter.child.try_wait()
        {
            return Err(adapter.exit_error(&self.name, status));
        }
        Ok(())
    }

    pub fn stop(&self) {
        crate::runtime::lock_recover(&self.adapter).take();
    }
}

/// Bare names prefer the canonical executable's directory, then PATH.
pub fn resolve_executable(program: &str) -> Result<PathBuf> {
    if program.contains(['/', '\\']) {
        return Ok(program.into());
    }
    let executable = std::env::current_exe()?.canonicalize()?;
    resolve_from(program, executable.parent().unwrap_or(Path::new(".")))
}

fn resolve_from(program: &str, directory: &Path) -> Result<PathBuf> {
    which::which_in(program, Some(directory.as_os_str()), directory)
        .ok()
        .or_else(|| crate::runtime::which(program))
        .with_context(|| format!("Adapter executable not found: {}", program))
}

impl Adapter {
    pub(crate) fn start_with_control(
        argv: &[String],
        name: &str,
        stage: &str,
        control: &dyn Fn() -> Result<()>,
    ) -> Result<Self> {
        let program = argv
            .first()
            .context("The adapter command must name an executable.")?;
        let port = crate::runtime::free_port()?;
        let mut command = Command::new(resolve_executable(program)?);
        command
            .args(&argv[1..])
            .args(["--host", "127.0.0.1", "--port", &port.to_string()]);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::progress::stage_label(&format!("{stage}/provider"), "start", "Starting adapter");
        let mut child = crate::runtime::ManagedChild::spawn_tree("adapter", &mut command)?;
        let stderr = crate::runtime::StderrTail::default();
        let mut drains = Vec::new();
        if let Some(pipe) = child.take_stderr() {
            drains.push(stderr.drain(pipe, "adapter stderr"));
        }
        if let Some(pipe) = child.take_stdout() {
            drains.push(stderr.drain(pipe, "adapter stdout"));
        }
        let mut adapter = Self {
            child,
            stderr,
            base: format!("http://127.0.0.1:{port}"),
            drains,
            stopped: false,
        };
        let started = Instant::now();
        if let Err(error) = crate::runtime::wait_ready(
            &adapter.base,
            ADAPTER_READY_TIMEOUT,
            &mut adapter.child,
            Some(control),
        ) {
            let status = adapter.child.try_wait();
            adapter.stop();
            if error.downcast_ref::<crate::dispatch::Failure>().is_some()
                || error.downcast_ref::<crate::runtime::Cancelled>().is_some()
            {
                return Err(error);
            }
            if let Some(status) = status {
                return Err(adapter.exit_error(name, status));
            }
            return Err(adapter.stderr.attach(error, "Could not start the adapter"));
        }
        tracing::info!(secs = started.elapsed().as_secs_f64(), "Adapter ready");
        crate::progress::stage_label(&format!("{stage}/provider"), "done", "Starting adapter");
        Ok(adapter)
    }

    fn exit_error(&mut self, name: &str, status: std::process::ExitStatus) -> anyhow::Error {
        self.stop();
        let code = status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| status.to_string());
        let message = format!("Provider {name} exited with status {code}");
        let tail = self.stderr.tail();
        anyhow::anyhow!(if tail.is_empty() {
            message
        } else {
            format!("{message}\n{tail}")
        })
    }

    pub(crate) fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.child.stop();
        for drain in self.drains.drain(..) {
            let _ = drain.join();
        }
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(record: &Path, behavior: &str) -> Option<Vec<String>> {
        let python =
            crate::runtime::which("python3").or_else(|| crate::runtime::which("python"))?;
        Some(vec![
            python.display().to_string(),
            "-u".into(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/asr_adapter.py").into(),
            "--record".into(),
            record.display().to_string(),
            "--behavior".into(),
            behavior.into(),
        ])
    }

    #[test]
    fn cancelled_provider_resolution_does_not_start_a_program() {
        let provider = Provider::new("fixture".into(), vec!["must-not-start".into()]);
        let error = provider
            .base("test", &|| Err(crate::runtime::Cancelled.into()))
            .unwrap_err();
        assert!(error.downcast_ref::<crate::runtime::Cancelled>().is_some());
    }

    #[test]
    fn adapter_waits_for_200_drains_both_pipes_and_stops_on_drop() {
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        let dir = tempfile::tempdir().unwrap();
        let Some(argv) = argv(dir.path(), "healthy") else {
            return;
        };
        let adapter = Adapter::start_with_control(
            &argv,
            "fixture",
            "transcript",
            &crate::runtime::check_cancelled,
        )
        .unwrap();
        assert!(
            std::fs::read_to_string(dir.path().join("health-checks"))
                .unwrap()
                .parse::<u32>()
                .unwrap()
                >= 3
        );
        let tail = adapter.stderr.tail();
        assert!(tail.contains("adapter stdout"), "{tail}");
        assert!(tail.contains("adapter stderr"), "{tail}");
        drop(adapter);
        #[cfg(unix)]
        assert!(dir.path().join("terminated").is_file());
    }

    #[test]
    fn adapter_startup_exit_includes_the_output_tail() {
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        let dir = tempfile::tempdir().unwrap();
        let Some(argv) = argv(dir.path(), "exit") else {
            return;
        };
        let error = Adapter::start_with_control(
            &argv,
            "fixture",
            "transcript",
            &crate::runtime::check_cancelled,
        )
        .err()
        .unwrap();
        let error = format!("{error:#}");
        assert!(
            error.starts_with("Provider fixture exited with status 3"),
            "{error}"
        );
        assert!(error.contains("adapter stderr"), "{error}");
        assert!(error.contains("adapter stdout"), "{error}");
    }

    #[test]
    fn adapter_startup_obeys_control_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let Some(argv) = argv(dir.path(), "never") else {
            return;
        };
        let polls = std::cell::Cell::new(0);
        let start = Instant::now();
        let result = Adapter::start_with_control(&argv, "fixture", "transcript", &|| {
            polls.set(polls.get() + 1);
            // Cancel only once the fixture has installed its SIGTERM handler, which it announces with its pid.
            anyhow::ensure!(
                polls.get() < 4 || !dir.path().join("pid").is_file(),
                "test cancellation"
            );
            Ok(())
        });
        assert!(format!("{:#}", result.err().unwrap()).contains("test cancellation"));
        assert!(start.elapsed() < Duration::from_secs(60));
        // A starved runner may not run the SIGTERM handler within the grace period, so check that the process is gone;
        // the drop test above covers the graceful path.
        #[cfg(unix)]
        {
            let pid: i32 = std::fs::read_to_string(dir.path().join("pid"))
                .unwrap()
                .parse()
                .unwrap();
            let start = Instant::now();
            while unsafe { libc::kill(pid, 0) } == 0 && start.elapsed() < Duration::from_secs(30) {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(
                unsafe { libc::kill(pid, 0) },
                -1,
                "adapter survived cancellation"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn adapter_terminates_the_group_and_kills_a_stubborn_leader() {
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        let dir = tempfile::tempdir().unwrap();
        let Some(argv) = argv(dir.path(), "stubborn") else {
            return;
        };
        let adapter = Adapter::start_with_control(
            &argv,
            "fixture",
            "transcript",
            &crate::runtime::check_cancelled,
        )
        .unwrap();
        let pid = adapter.child.id();
        let child: i32 = std::fs::read_to_string(dir.path().join("child-pid"))
            .unwrap()
            .parse()
            .unwrap();
        let start = Instant::now();
        drop(adapter);
        assert!(start.elapsed() < Duration::from_secs(10));
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
        // The killed child is reaped by init, which may take a moment on a busy machine.
        let start = Instant::now();
        while unsafe { libc::kill(child, 0) } == 0 && start.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            unsafe { libc::kill(child, 0) },
            -1,
            "adapter child survived"
        );
    }

    #[cfg(windows)]
    #[test]
    fn adapter_job_terminates_children_after_the_launcher_exits() {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::{
            Foundation::WAIT_OBJECT_0,
            System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
        };
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        let dir = tempfile::tempdir().unwrap();
        let Some(argv) = argv(dir.path(), "launcher") else {
            return;
        };
        let mut adapter = Adapter::start_with_control(
            &argv,
            "fixture",
            "transcript",
            &crate::runtime::check_cancelled,
        )
        .unwrap();
        let pid = std::fs::read_to_string(dir.path().join("child-pid"))
            .unwrap()
            .parse()
            .unwrap();
        let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!process.is_null());
        let process = unsafe { OwnedHandle::from_raw_handle(process) };
        std::fs::write(dir.path().join("release-launcher"), "exit").unwrap();
        let start = Instant::now();
        while adapter.child.try_wait().is_none() {
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "launcher did not exit"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(adapter);
        assert!(start.elapsed() < Duration::from_secs(30));
        assert_eq!(
            unsafe { WaitForSingleObject(process.as_raw_handle(), 1000) },
            WAIT_OBJECT_0
        );
    }

    #[cfg(unix)]
    #[test]
    fn executable_resolution_prefers_sibling_then_path_and_preserves_paths() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let sibling = dir.path().join("sh");
        std::fs::write(&sibling, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&sibling, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(resolve_from("sh", dir.path()).unwrap(), sibling);
        std::fs::remove_file(sibling).unwrap();
        assert_eq!(
            resolve_from("sh", dir.path()).unwrap(),
            crate::runtime::which("sh").unwrap()
        );
        assert_eq!(
            resolve_executable("./my-adapter").unwrap(),
            PathBuf::from("./my-adapter")
        );
    }
}
