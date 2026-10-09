//! 长驻子进程生命周期管理（llama-server / NPU worker）。
//!
//! 统一三件此前散落在 asr.rs / npu.rs 的重复实现：
//! - `ManagedChild`：Drop 保证 kill+wait，任何错误路径（含 `?` 早退）不泄漏进程
//! - `wait_ready`：健康轮询期间监视子进程，秒退立即报错而不是傻等 300s 超时
//! - `which` / `free_port` / `TempWorkDir`：单一实现

use anyhow::{Context, Result};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// 获取互斥锁；若锁已中毒（某线程在持锁期间 panic），直接取走数据继续用。
/// 语义：本进程自愿接受中毒锁——所有持锁临界区只做内存内状态更新，
/// 数据本身不会被 panic 路径写坏，传播 PoisonError 反而会让无关线程陪葬。
pub fn lock_recover<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// kill-on-drop 的子进程句柄。
pub struct ManagedChild {
    child: Child,
    name: &'static str,
}

impl ManagedChild {
    pub fn spawn(name: &'static str, cmd: &mut Command) -> Result<Self> {
        let child = cmd
            .spawn()
            .with_context(|| format!("无法启动 {name} / Could not start {name}"))?;
        Ok(Self { child, name })
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// 取出 piped 的 stderr 句柄（配合 [`drain_stderr`] 使用）。
    pub fn take_stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child.stderr.take()
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    /// 非阻塞收割；Some = 已退出。
    pub fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// 轮询等待进程退出，最多 `timeout`；Err = 超时（进程仍由 Drop 兜底 kill+wait）。
    /// 收拢此前 asr.rs / npu.rs 各自手写的「try_wait 轮询 + 截止时间」。
    pub fn wait_within(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let t0 = Instant::now();
        loop {
            if let Some(status) = self.try_wait() {
                return Ok(status);
            }
            anyhow::ensure!(
                t0.elapsed() < timeout,
                "{} 未在 {:.1}s 内退出 / did not exit within the time limit",
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
        // 双 kill 无害（对已退出进程返回 Err 被忽略）
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 后台读取子进程 stderr：避免 pipe 写满阻塞子进程；保留尾部若干行
/// 供失败诊断；verbose(debug) 时逐行转发到 tracing。
/// 返回的尾部缓存与会话同寿命，随时可读取。
#[derive(Clone, Default)]
pub struct StderrTail {
    lines: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

const STDERR_TAIL_MAX: usize = 100;
/// wait_within 的轮询间隔：50ms 足够及时，又不至于空转 CPU
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(50);

impl StderrTail {
    pub fn tail(&self) -> String {
        self.lines.lock().map(|v| v.join("\n")).unwrap_or_default()
    }

    /// 给错误附上 stderr 尾部作为 context（尾部为空则原样返回）。
    /// 收拢 asr.rs / npu.rs 同构的「失败时附加子进程 stderr 尾部」骨架。
    pub fn attach(&self, e: anyhow::Error, label: &str) -> anyhow::Error {
        let tail = self.tail();
        if tail.is_empty() {
            e
        } else {
            e.context(format!("{label}:\n{tail}"))
        }
    }
}

/// 为一个已 spawn 的 stderr pipe 启动 drain 线程。
/// `target` 是 tracing target（如 "llama_server" / "npu_worker"），用于区分日志来源。
pub fn drain_stderr(stderr: std::process::ChildStderr, target: &'static str) -> StderrTail {
    use std::io::BufRead;
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let shared = lines.clone();
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            // tracing 的 target: 只接受常量，来源名放消息前缀
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
    });
    StderrTail { lines }
}

/// 有界命令输出（stdout/stderr 经临时文件捕获，不受 pipe 缓冲与 GUI stdin 影响）。
pub struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// 短命令执行：stdin 关闭（ffmpeg 等工具在 GUI/管道 stdin 上会挂死）、
/// 超时强制 kill（ManagedChild 兜底）、输出经临时文件捕获（防 pipe 写满再超时）。
pub fn run_bounded(name: &'static str, cmd: &mut Command, timeout: Duration) -> Result<BoundedOutput> {
    use std::io::{Read, Seek};
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    cmd.stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);
    let mut child = ManagedChild::spawn(name, cmd)?;
    let status = child.wait_within(timeout).map_err(|e| {
        anyhow::anyhow!(
            "{name} 超过 {:.0}s 未完成，已终止 / did not finish within the time limit and was killed: {e:#}",
            timeout.as_secs_f64()
        )
    })?;
    stdout.rewind()?;
    stderr.rewind()?;
    let mut out = String::new();
    let mut err = String::new();
    stdout.take(64 * 1024 * 1024).read_to_string(&mut out)?;
    stderr.take(64 * 1024 * 1024).read_to_string(&mut err)?;
    Ok(BoundedOutput {
        status,
        stdout: out,
        stderr: err,
    })
}

/// 轮询 `{base}/health` 直到成功；子进程中途退出立即失败（不等满超时）。
/// `expect_body` 非空时还要求响应体包含该子串——防止端口被无关服务
/// 抢占（free_port 存在 TOCTOU 窗口）而误判就绪。
/// `control` 非空时每轮轮询前调用：返回 Err 表示外部要求中止（暂停/取消），
/// 立即报错返回而不等满超时；子进程由 ManagedChild 的 Drop 兜底 kill。
pub fn wait_ready(
    base: &str,
    timeout: Duration,
    child: &mut ManagedChild,
    expect_body: Option<&str>,
    control: Option<&dyn Fn() -> Result<()>>,
) -> Result<()> {
    let t0 = Instant::now();
    let url = format!("{base}/health");
    loop {
        if let Some(check) = control
            && let Err(e) = check()
        {
            anyhow::bail!("{} 启动被中止 / startup aborted: {e:#}", child.name());
        }
        if let Some(st) = child.try_wait() {
            anyhow::bail!(
                "{} 启动失败 / exited during startup ({st}); see error details",
                child.name()
            );
        }
        if t0.elapsed() > timeout {
            anyhow::bail!(
                "{} 启动超时 / startup timed out ({:.0}s)",
                child.name(),
                timeout.as_secs_f64()
            );
        }
        if let Ok(resp) = ureq::get(&url).timeout(Duration::from_secs(2)).call() {
            match expect_body {
                None => return Ok(()),
                Some(needle) => {
                    if let Ok(body) = resp.into_string()
                        && body.contains(needle)
                    {
                        return Ok(());
                    }
                    // 端口上跑着别的服务：继续等（真正要起的服务可能还在绑端口）
                }
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// 无重定向的 HTTP agent：整体超时参数化，内置 `.redirects(0)`。
///
/// 安全纪律（与 dispatch.rs 请求账本注释一致）：请求经 3xx 重定向后失败，
/// 无法证明服务端「未提交」——可能已产生费用/副作用，因此凡走账本的调用
/// 必须禁止跟随重定向。这里在构造处统一内置，不靠每个调用点自觉。
/// 需要 connect/read 细分超时的场景（fetch.rs 元数据、models.rs 大文件下载）
/// 形态不合，保留各自 AgentBuilder，不套用本 helper。
pub fn http_agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(timeout)
        .redirects(0)
        .build()
}

/// 在 PATH 上查找可执行文件（Windows 自动尝试 .exe 后缀）。
pub fn which(cmd: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names: Vec<String> = if cfg!(windows) {
        vec![cmd.to_string(), format!("{cmd}.exe")]
    } else {
        vec![cmd.to_string()]
    };
    std::env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|p| is_executable(p))
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
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
            "目录正被另一任务使用 / Directory is in use: {}. 请等待任务结束 / Wait for it to finish.",
            path.display()
        )
    })?;
    Ok(FileLock { file })
}

/// 让 OS 分配一个空闲端口。
/// 注意 TOCTOU：返回后到真正 bind 之间端口可能被抢占，
/// 调用方需用 `wait_ready` 的响应体校验兜底。
pub fn free_port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// 临时工作目录（chunk / worker 脚本等中间产物）：创建失败立即报错，
/// Drop 时尽力清理整个目录。目录名带 tag 与 pid 区分来源与并发实例。
pub struct TempWorkDir {
    dir: tempfile::TempDir,
}

impl TempWorkDir {
    pub fn new(tag: &str) -> Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix(&format!("course2md-{tag}-"))
            .tempdir()
            .context("无法创建临时工作目录 / Could not create a temporary working directory")?;
        Ok(Self { dir })
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// issue：桌面取消任务后，llama-server 首次加载（最长 300s 超时）不得继续空等——
    /// control 钩子每轮轮询，返回 Err 立即中止，进程由 Drop 兜底。
    #[cfg(unix)]
    #[test]
    fn wait_ready_aborts_when_control_hook_fails() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let mut child = ManagedChild::spawn("sleep", &mut cmd).unwrap();
        let t0 = Instant::now();
        // 永远不就绪的端口 + 60s 超时；control 第二轮即报错，应远早于超时返回
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let result = wait_ready(
            "http://127.0.0.1:9",
            Duration::from_secs(60),
            &mut child,
            None,
            Some(&|| {
                if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 1 {
                    anyhow::bail!("测试取消 / test cancellation")
                } else {
                    Ok(())
                }
            }),
        );
        let err = result.expect_err("control 报错必须中止 wait_ready");
        assert!(err.to_string().contains("启动被中止"), "{err:#}");
        assert!(t0.elapsed() < Duration::from_secs(10), "取消不应等到超时");
    }

    #[test]
    fn which_finds_common_tools() {
        // CI 环境保证 cargo 存在；本测试机器必有 shell 基础工具
        #[cfg(unix)]
        let probe = "ls";
        #[cfg(not(unix))]
        let probe = "cmd";
        assert!(which(probe).is_some());
        assert!(which("definitely-not-a-real-binary-xyz").is_none());
    }

    #[test]
    fn free_port_returns_bindable() {
        let p = free_port().unwrap();
        // 立即再绑同一端口不一定成功（TOCTOU），但必须是合法端口值
        assert!(p > 0);
    }

    #[test]
    fn same_tag_directories_do_not_share_or_delete_each_others_files() {
        let first = TempWorkDir::new("parallel").unwrap();
        let second = TempWorkDir::new("parallel").unwrap();
        assert_ne!(first.path(), second.path());
        std::fs::write(second.path().join("chunk.wav"), b"audio").unwrap();
        drop(first);
        assert_eq!(
            std::fs::read(second.path().join("chunk.wav")).unwrap(),
            b"audio"
        );
    }

    #[test]
    fn temp_workdir_created_and_cleaned() {
        let path;
        {
            let d = TempWorkDir::new("test").unwrap();
            path = d.path().to_path_buf();
            assert!(path.is_dir());
            std::fs::write(path.join("x"), b"y").unwrap();
        }
        assert!(!path.exists(), "Drop 必须清理整个目录");
    }

    /// issue #12：错误路径/panic unwind 也必须 kill+wait llama-server——
    /// 孤儿进程占着 /dev/kfd 会加剧 ROCm 核显问题。这里验证 Drop 的兜底语义。
    #[cfg(unix)]
    #[test]
    fn managed_child_drop_kills_and_reaps() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let child = ManagedChild::spawn("sleep", &mut cmd).unwrap();
        let pid = child.id().to_string();
        drop(child);
        // kill -0 仅探测存在性：Drop 已 kill+wait，进程应不复存在（zombie 也被收割）
        let st = Command::new("kill").args(["-0", &pid]).status().unwrap();
        assert!(!st.success(), "pid {pid} 应已被 Drop 清理");
    }
}
