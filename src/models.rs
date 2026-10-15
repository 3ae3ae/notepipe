//! 模型缓存管理与下载（llama.cpp Qwen3-ASR GGUF）。
//!
//! 默认目录：`~/.cache/course2md/models/`（可用 `--model-dir` / `models download --dir` 覆盖）
//!
//! ```text
//! models/
//!   llama-qwen3-1.7b/
//!     Qwen3-ASR-1.7B-Q8_0.gguf
//!     mmproj-Qwen3-ASR-1.7B-Q8_0.gguf
//! ```

use anyhow::{Context, Result};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[path = "model_status.rs"]
pub mod status;

fn unsupported_model(status: &status::LocalModelStatus) -> anyhow::Error {
    let available = crate::config::local_models(status.provider)
        .iter()
        .map(|m| m.id)
        .collect::<Vec<_>>()
        .join(", ");
    anyhow::anyhow!(
        "所选识别方式不支持模型「{}」。可用：{available} / The {} backend does not support model \"{}\". Available: {available}",
        status.model,
        status.provider,
        status.model
    )
}

/// 下载缺失的模型文件（不加载）。`verify` 时额外用正式加载器验证 Apple/NPU 模型可离线加载。
async fn fetch_model(
    provider: crate::config::AsrProvider,
    model: &str,
    root: &Path,
    cached: bool,
    verify: bool,
) -> Result<()> {
    use crate::config::AsrProvider;
    match provider {
        AsrProvider::Cpu | AsrProvider::Gpu => download_models(root).await,
        AsrProvider::Coreml => {
            #[cfg(apple_native)]
            {
                let model = model.to_owned();
                tokio::task::spawn_blocking(move || {
                    if !cached {
                        crate::apple::prepare_cache(&model)?;
                    }
                    if verify {
                        crate::apple::prepare_model(&model)?;
                    }
                    Ok(())
                })
                .await
                .context("模型准备未完成 / Model preparation did not complete")?
            }
            #[cfg(not(apple_native))]
            {
                let _ = (model, cached, verify);
                Err(anyhow::anyhow!(
                    "此程序未包含 Apple 原生识别运行时 / This build does not include the Apple native transcription runtime"
                ))
            }
        }
        AsrProvider::Npu => {
            let model = crate::npu::resolve_npu_model(Some(model));
            tokio::task::spawn_blocking(move || crate::npu::prepare_npu_model(&model))
                .await
                .context("NPU 模型准备未完成 / NPU model preparation did not complete")?
        }
        AsrProvider::Api => Ok(()),
    }
}

/// Retrieve missing local model files before a large media download. Cached weights are
/// loaded by the later transcription stage; subtitle-only work never calls this function.
pub async fn ensure_cache(
    provider: crate::config::AsrProvider,
    model: &str,
    root: &Path,
) -> Result<()> {
    let status = status::inspect(provider, model, root)?;
    if !status.can_prepare {
        return Err(unsupported_model(&status));
    }
    if matches!(
        status.state,
        status::CacheState::Cached | status::CacheState::Loaded
    ) {
        return Ok(());
    }
    fetch_model(provider, &status.model, root, false, false).await
}

/// Prepare the exact selected backend without sending course content or changing settings.
pub async fn prepare(
    provider: crate::config::AsrProvider,
    model: &str,
    root: &Path,
) -> Result<status::LocalModelStatus> {
    use crate::config::AsrProvider;
    let before = status::inspect(provider, model, root)?;
    if !before.can_prepare {
        return Err(unsupported_model(&before));
    }
    crate::progress::stage("model/prepare", "start");
    let cached = matches!(
        before.state,
        status::CacheState::Cached | status::CacheState::Loaded
    );
    let result = fetch_model(provider, &before.model, root, cached, true).await;
    let after = status::inspect(provider, model, root).unwrap_or_else(|error| {
        tracing::warn!("模型二次检查失败，沿用准备前状态 / Post-prepare inspection failed; keeping the pre-prepare status: {error:#}");
        before
    });
    let loaded = result.is_ok() && matches!(provider, AsrProvider::Coreml | AsrProvider::Npu);
    let error = result.as_ref().err().map(|error| format!("{error:#}"));
    if let Err(error) = status::record_result(&after, loaded, error) {
        tracing::warn!(
            "模型检查结果暂时无法保存 / Could not save the model check result: {error:#}"
        );
    }
    result?;
    crate::progress::stage("model/prepare", "done");
    Ok(status::with_receipt(after))
}

const HF_REPO_PATH: &str = "ggml-org/Qwen3-ASR-1.7B-GGUF/resolve/main";

/// llama GGUF 模型 slug：模型目录名与 checkpoint 身份都由它派生。
const LLAMA_MODEL_SLUG: &str = crate::config::DEFAULT_ASR_MODEL;
const LLAMA_MODEL_FILE: &str = "Qwen3-ASR-1.7B-Q8_0.gguf";
const LLAMA_MMPROJ_FILE: &str = "mmproj-Qwen3-ASR-1.7B-Q8_0.gguf";

/// 当前 llama GGUF 模型的身份字符串（checkpoint/日志用），与模型目录同源。
pub fn llama_gguf_identity() -> String {
    format!("{LLAMA_MODEL_SLUG}-gguf")
}

/// Hugging Face 端点：尊重 HF_ENDPOINT 镜像（与 CoreML 路径行为一致）。
fn hf_base(endpoint: Option<String>) -> String {
    endpoint
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://huggingface.co".into())
}

fn current_hf_endpoint() -> Option<String> {
    std::env::var("HF_ENDPOINT").ok()
}

#[derive(Debug, Clone)]
pub struct LlamaAsr {
    pub model: PathBuf,
    pub mmproj: PathBuf,
}

pub fn llama_paths(root: &Path) -> LlamaAsr {
    let d = root.join(format!("llama-{LLAMA_MODEL_SLUG}"));
    LlamaAsr {
        model: d.join(LLAMA_MODEL_FILE),
        mmproj: d.join(LLAMA_MMPROJ_FILE),
    }
}

pub fn llama_ready(root: &Path) -> bool {
    let p = llama_paths(root);
    file_complete(&p.model) && file_complete(&p.mmproj)
}

/// 文件完整性：有 manifest（下载完成时记录的精确字节数）时按字节数校验；
/// 无 manifest 的旧缓存检查合法 GGUF 文件头及最小大小；这不等于模型已加载成功。
fn file_complete(path: &Path) -> bool {
    let Ok(md) = fs::metadata(path) else {
        return false;
    };
    // >1MB 启发式：GGUF 合法文件头（magic + 版本 + 张量元数据索引）加上任何
    // 可用权重都远超 1MB；≤1MB 必是截断残留或代理/镜像返回的错误页面。
    if !path.is_file() || md.len() <= 1_000_000 {
        return false;
    }
    let mut header = [0_u8; 8];
    if fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_err()
        || &header[..4] != b"GGUF"
        || !matches!(u32::from_le_bytes(header[4..].try_into().unwrap()), 2 | 3)
    {
        return false;
    }
    let manifest = path.with_extension("manifest.json");
    if let Ok(s) = fs::read_to_string(&manifest)
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&s)
        && let Some(expected) = v.get("size").and_then(|s| s.as_u64())
    {
        return md.len() == expected;
    }
    true
}

/// 没有模型就下载；下载过程请保持进程运行。
pub async fn ensure_llama_or_download(root: &Path) -> Result<LlamaAsr> {
    if !llama_ready(root) {
        if !crate::progress::is_json() && !crate::progress::is_quiet() {
            eprintln!(
                "正在下载识别模型（约 2.4GB）/ Downloading speech models (~2.4GB): {}",
                root.display()
            );
        }
        download_models(root).await?;
    }
    Ok(llama_paths(root))
}

/// 下载失败的附加提示：未设镜像时提示 HF_ENDPOINT（直连 Hugging Face
/// 不稳定是常见失败原因）；已设镜像则回显当前端点便于排查。
fn mirror_hint() -> String {
    match current_hf_endpoint() {
        Some(ep) => format!("下载失败 / Download failed (HF_ENDPOINT={ep})"),
        None => "下载失败，请检查网络或配置镜像 / Download failed; check your connection or configure a mirror: \
                 HF_ENDPOINT=https://hf-mirror.com"
            .into(),
    }
}

/// 下载 llama.cpp Qwen3-ASR GGUF。
pub async fn download_models(root: &Path) -> Result<()> {
    fs::create_dir_all(root)?;
    let _download_lock = crate::runtime::lock_file(&root.join(".download.lock"))?;
    let p = llama_paths(root);
    let base = hf_base(current_hf_endpoint());
    tracing::info!(endpoint = %base, "huggingface endpoint");
    let model_url = format!("{base}/{HF_REPO_PATH}/{LLAMA_MODEL_FILE}");
    let projector_url = format!("{base}/{HF_REPO_PATH}/{LLAMA_MMPROJ_FILE}");
    let (model, projector) = tokio::join!(
        download_file(&model_url, &p.model, LLAMA_MODEL_FILE),
        download_file(&projector_url, &p.mmproj, LLAMA_MMPROJ_FILE),
    );
    model.with_context(mirror_hint)?;
    projector.with_context(mirror_hint)?;
    anyhow::ensure!(
        llama_ready(root),
        "模型文件下载不完整，请重新运行 course2md models download / Model download incomplete; run course2md models download again"
    );
    tracing::info!(path = %root.display(), "models ready");
    Ok(())
}

/// 下载重试次数（2.4GB 大文件，网络抖动/代理断流常见）。
const DOWNLOAD_ATTEMPTS: usize = 3;

/// 4xx（除 429 限流）是确定性错误（鉴权失败/路径错/镜像缺文件），退避重试无意义。
fn is_permanent_status(code: u16) -> bool {
    code != 429 && (400..500).contains(&code)
}

/// 确定性 HTTP 错误标记：重试循环见此类型直接失败。
#[derive(Debug)]
struct PermanentHttp(String);

impl std::fmt::Display for PermanentHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PermanentHttp {}

async fn download_file(url: &str, dest: &Path, label: &str) -> Result<()> {
    if dest.is_file() && file_complete(dest) {
        tracing::info!(label, "skip existing");
        return Ok(());
    }
    if dest.is_file() {
        // 校验不过的残留文件（截断/损坏）直接移除，避免"发现坏了却重下不了"
        tracing::warn!(
            label,
            "模型文件不完整，正在重新下载 / Model file incomplete; downloading again"
        );
        let _ = fs::remove_file(dest);
        let _ = fs::remove_file(dest.with_extension("manifest.json"));
    }
    if let Some(p) = dest.parent() {
        fs::create_dir_all(p)?;
    }
    let tmp = dest.with_extension("part");
    let url = url.to_string();
    let dest = dest.to_path_buf();
    let label = label.to_string();
    let stage = format!("model/{label}");
    crate::progress::stage(&stage, "start");
    tokio::task::spawn_blocking(move || -> Result<()> {
        // 只设连接/读超时，不设整体超时：2.4GB 大文件下完为止，
        // 读超时 10 分钟无数据视为挂起
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(30))
            .timeout_read(std::time::Duration::from_secs(600))
            .build();
        let mut last_err: Option<anyhow::Error> = None;
        for attempt in 1..=DOWNLOAD_ATTEMPTS {
            if attempt > 1 {
                let wait = std::time::Duration::from_secs(1 << (attempt - 1)); // 2s、4s 指数退避
                tracing::warn!(
                    label = %label,
                    attempt,
                    of = DOWNLOAD_ATTEMPTS,
                    ?wait,
                    "下载失败，稍后重试 / Download failed; retrying shortly"
                );
                std::thread::sleep(wait);
                crate::dispatch::check_control()?;
            }
            match download_once(&agent, &url, &tmp, &dest, &label) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    // 确定性 HTTP 错误（4xx≠429）与取消不再重试
                    if e.downcast_ref::<PermanentHttp>().is_some()
                        || crate::dispatch::check_control().is_err()
                    {
                        return Err(e);
                    }
                    // 保留 .part：日志给出断点位置；当前实现重跑会从头下载
                    tracing::warn!(
                        label = %label,
                        part = %tmp.display(),
                        "下载失败，重试将从头下载 / Download failed; retrying starts a fresh download: {e:#}"
                    );
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("模型下载失败 / Model download failed")))
    })
    .await
    .context("模型下载任务失败 / Model download task failed")??;
    crate::progress::stage(&stage, "done");
    Ok(())
}

/// 单次下载尝试：成功时原子落盘 dest 并写 manifest；失败保留 .part 供排查。
fn download_once(
    agent: &ureq::Agent,
    url: &str,
    tmp: &Path,
    dest: &Path,
    label: &str,
) -> Result<()> {
    tracing::info!(label = %label, url = %url, "download");
    let resp = match agent.get(url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, resp)) => {
            // 错误信息携带响应体尾部（服务器返回的错误页/JSON 通常是排查关键）
            let body = resp.into_string().unwrap_or_default();
            let n = body.chars().count();
            let tail: String = body.chars().skip(n.saturating_sub(200)).collect();
            let msg = format!("HTTP {code}: {tail}");
            if is_permanent_status(code) {
                return Err(anyhow::anyhow!(PermanentHttp(msg)));
            }
            return Err(anyhow::anyhow!(
                "模型请求失败 / Model request failed: {msg}"
            ));
        }
        Err(e) => return Err(anyhow::Error::new(e).context("模型请求失败 / Model request failed")),
    };
    let total: u64 = resp
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let pb = crate::progress::Bar::new(format!("model/{label}"), total)
        .with_template("{spinner:.green} {msg} [{bar:32.cyan/blue}] {bytes}/{total_bytes} ({eta})");
    pb.set_message(label.to_string());
    let mut reader = resp.into_reader();
    let mut out = fs::File::create(tmp)?;
    let mut buf = vec![0u8; 1024 * 512];
    let mut done: u64 = 0;
    let mut since_check: u64 = 0;
    loop {
        // 每 32MB 检查一次取消，桌面端取消模型准备时不必等 2.4GB 下完
        if since_check >= 32 << 20 {
            crate::dispatch::check_control()?;
            since_check = 0;
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut out, &buf[..n])?;
        done += n as u64;
        since_check += n as u64;
        pb.set_position(done);
    }
    out.sync_all()?;
    drop(out);
    // 完整性：以服务器 Content-Length 为准（而非"实际收到多少"——截断响应会伪装成功）；
    // 不完整时保留 .part（断点位置见上方日志）
    if total > 0 && done != total {
        pb.finish();
        anyhow::bail!(
            "下载不完整，请重试 / Incomplete download; retry (expected {total} bytes, received {done})"
        );
    }
    fs::rename(tmp, dest)?;
    // manifest 记录 authoritative Content-Length，供后续启动校验（原子写，防半截 JSON）
    let _ = crate::checkpoint::atomic_write(
        &dest.with_extension("manifest.json"),
        serde_json::json!({"size": if total > 0 { total } else { done }})
            .to_string()
            .as_bytes(),
    );
    pb.finish();
    tracing::info!(label = %label, bytes = done, "downloaded");
    Ok(())
}

pub fn list_models(root: &Path) {
    let p = llama_paths(root);
    println!(
        "gpu/cpu 模型目录 / gpu/cpu model directory: {}",
        root.display()
    );
    println!(
        "  model  {} {}",
        if file_complete(&p.model) {
            "就绪 / Ready"
        } else {
            "缺失或不完整 / Missing or incomplete"
        },
        p.model.display()
    );
    println!(
        "  mmproj {} {}",
        if file_complete(&p.mmproj) {
            "就绪 / Ready"
        } else {
            "缺失或不完整 / Missing or incomplete"
        },
        p.mmproj.display()
    );
    if !llama_ready(root) {
        println!(
            "下载或修复 / Download or repair: course2md models download --dir \"{}\"",
            root.display()
        );
    }
    println!(
        "Apple 和 NPU 模型由各自后端管理 / Apple and NPU models are managed by their backends."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_and_unknown_size_downloads_only_publish_complete_files() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for response in [
                "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabc",
                "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nabcdef",
                "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n12345",
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    if stream.read(&mut byte).unwrap() == 0 {
                        break;
                    }
                    request.push(byte[0]);
                }
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        let root = tempfile::tempdir().unwrap();
        let temporary = root.path().join("model.part");
        let destination = root.path().join("model.bin");
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(3))
            .build();
        let url = format!("http://{address}/model");
        assert!(download_once(&agent, &url, &temporary, &destination, "synthetic").is_err());
        assert!(!destination.exists());
        assert_eq!(fs::read(&temporary).unwrap(), b"abc");
        download_once(&agent, &url, &temporary, &destination, "synthetic").unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"abcdef");
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(destination.with_extension("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["size"], 6);
        assert!(!temporary.exists());
        let known = root.path().join("known.bin");
        download_once(&agent, &url, &temporary, &known, "synthetic").unwrap();
        assert_eq!(fs::read(&known).unwrap(), b"12345");
        server.join().unwrap();
        let missing = root.path().join("offline.bin");
        assert!(download_once(&agent, &url, &temporary, &missing, "synthetic").is_err());
        assert!(!missing.exists());
        assert_eq!(fs::read(destination).unwrap(), b"abcdef");
    }

    #[test]
    fn hf_endpoint_mirror_is_honored() {
        assert_eq!(hf_base(None), "https://huggingface.co");
        assert_eq!(
            hf_base(Some("https://hf-mirror.com".into())),
            "https://hf-mirror.com"
        );
        // 尾部斜杠归一；空白视为未设置
        assert_eq!(
            hf_base(Some("https://hf-mirror.com/".into())),
            "https://hf-mirror.com"
        );
        assert_eq!(hf_base(Some("  ".into())), "https://huggingface.co");
    }

    #[test]
    fn permanent_status_classification() {
        // 确定性错误：不重试
        for code in [400, 401, 403, 404, 422] {
            assert!(is_permanent_status(code), "{code} 应直接失败");
        }
        // 429 限流与 5xx 服务端错误：值得退避重试
        for code in [429, 500, 502, 503] {
            assert!(!is_permanent_status(code), "{code} 应重试");
        }
    }
}
