//! yt-dlp 子进程封装：元数据抓取 + 视频下载。

use crate::config::PipelineConfig;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// yt-dlp 调用的 socket 超时（秒）：兜底挂死的连接，三处调用统一
const YTDLP_SOCKET_TIMEOUT: &str = "12";
/// 视频下载尝试次数（首次 + 2 次重试）：网络类错误由外层循环重试
const DOWNLOAD_ATTEMPTS: u32 = 3;
/// 视频下载重试间隔
const DOWNLOAD_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(3);

/// yt-dlp 公共参数基座：--ignore-config（用户全局配置会让同一命令在不同机器上
/// 行为漂移）、socket 超时。差异参数由调用方追加，最后以 [`ytdlp_url`] 收尾。
fn ytdlp_base(cmd: &mut Command) -> &mut Command {
    cmd.args(["--ignore-config", "--socket-timeout", YTDLP_SOCKET_TIMEOUT])
}

/// 确定性错误（重试无意义）：4xx 拒绝、不支持的 URL、私有/不可用视频等。
/// 只匹配 yt-dlp 的错误文本；未列出的错误按网络类处理，仍由外层循环重试。
fn is_deterministic_download_error(error_text: &str) -> bool {
    let lower = error_text.to_ascii_lowercase();
    [
        "http error 401",
        "http error 403",
        "http error 404",
        "http error 410",
        "unsupported url",
        "private video",
        "video unavailable",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// yt-dlp 命令收尾：`--` 分隔符保证 URL 不被当作选项解析。
fn ytdlp_url<'a>(cmd: &'a mut Command, url: &str) -> &'a mut Command {
    cmd.arg("--").arg(url)
}

/// 我们关心的元数据字段子集。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VideoMeta {
    pub title: String,
    #[serde(default)]
    pub uploader: String,
    #[serde(default)]
    pub duration: f64,
    pub webpage_url: String,
    #[serde(default)]
    pub extractor: String,
    #[serde(default)]
    pub id: String,
}

impl VideoMeta {
    /// Local-file metadata uses the `local` extractor marker.
    pub fn is_local(&self) -> bool {
        self.extractor == "local"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceCandidate {
    /// The extractor's concrete video URL, never the original playlist URL.
    pub input: String,
    pub title: String,
    pub identity: Option<String>,
    pub duration: Option<f64>,
    /// 预览图 URL（分集首帧或视频封面）；本地缓存由桌面端负责
    #[serde(default)]
    pub thumbnail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OnlineVideo {
    pub meta: VideoMeta,
    pub identity: String,
    pub thumbnail: Option<String>,
    pub original_language: Option<String>,
    pub subtitles: crate::subtitle::SubtitleEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OnlineProbe {
    Video {
        video: OnlineVideo,
    },
    Collection {
        title: String,
        candidates: Vec<SourceCandidate>,
        unavailable_entries: usize,
    },
    Unresolved {
        message: String,
    },
}

/// Preserve the extractor's language order; serde_json::Value maps sort keys
/// unless a crate-wide feature is enabled, which would silently change defaults.
#[derive(Debug, Default)]
struct OrderedTracks(Vec<(String, Vec<serde_json::Value>)>);

impl<'de> Deserialize<'de> for OrderedTracks {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = OrderedTracks;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("subtitle language map")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Vec<serde_json::Value>>()? {
                    entries.push((key, value));
                }
                Ok(OrderedTracks(entries))
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}

#[derive(Debug, Default, Deserialize)]
struct ExtractorInfo {
    #[serde(rename = "_type", default)]
    kind: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    uploader: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    webpage_url: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    extractor: Option<String>,
    #[serde(default)]
    extractor_key: Option<String>,
    #[serde(default)]
    ie_key: Option<String>,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    subtitles: Option<OrderedTracks>,
    #[serde(default)]
    automatic_captions: Option<OrderedTracks>,
    #[serde(default)]
    entries: Option<Vec<Option<ExtractorInfo>>>,
}

fn extractor_name(info: &ExtractorInfo) -> &str {
    info.extractor
        .as_deref()
        .or(info.extractor_key.as_deref())
        .or(info.ie_key.as_deref())
        .unwrap_or_default()
}

/// Including the complete extractor ID preserves multi-part and interactive
/// segment IDs. Display titles and URL tracking parameters are not identity.
pub fn online_identity(extractor: &str, id: &str) -> Option<String> {
    let extractor = extractor.trim().to_ascii_lowercase();
    let id = id.trim();
    if extractor.is_empty() || id.is_empty() {
        return None;
    }
    Some(format!("online:{extractor}:{}:{}", id.len(), id))
}

fn concrete_url(info: &ExtractorInfo) -> Option<String> {
    info.webpage_url
        .as_deref()
        .or(info.url.as_deref())
        .filter(|value| {
            url::Url::parse(value).ok().is_some_and(|url| {
                matches!(url.scheme(), "https" | "http") && url.host_str().is_some()
            })
        })
        .map(str::to_owned)
}

/// Parse one metadata response from a subtitle-enabled, flat-playlist extraction.
/// Successful extraction can still contain explicit subtitle permission warnings.
pub fn parse_online_probe(bytes: &[u8], input: &str, diagnostics: &str) -> Result<OnlineProbe> {
    let info: ExtractorInfo =
        serde_json::from_slice(bytes).context(crate::tr!("Cannot read video info; probe again"))?;
    if info.kind == "multi_video" {
        return Ok(OnlineProbe::Unresolved {
            message: crate::tr!("This video is split into several parts and is not supported yet. Download the full video and import it.")
                .into(),
        });
    }
    if info.kind == "playlist" || info.entries.is_some() {
        let mut candidates = Vec::new();
        let mut unavailable_entries = 0;
        for entry in info.entries.unwrap_or_default() {
            let Some(entry) = entry else {
                unavailable_entries += 1;
                continue;
            };
            let Some(input) = concrete_url(&entry) else {
                unavailable_entries += 1;
                continue;
            };
            if matches!(entry.kind.as_str(), "playlist" | "multi_video") || entry.entries.is_some()
            {
                unavailable_entries += 1;
                continue;
            }
            if candidates
                .iter()
                .any(|candidate: &SourceCandidate| candidate.input == input)
            {
                continue;
            }
            let identity = entry
                .id
                .as_deref()
                .and_then(|id| online_identity(extractor_name(&entry), id));
            candidates.push(SourceCandidate {
                title: entry
                    .title
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| input.clone()),
                input,
                identity,
                duration: entry
                    .duration
                    .filter(|value| value.is_finite() && *value > 0.),
                thumbnail: entry.thumbnail.filter(|url| !url.trim().is_empty()),
            });
        }
        return Ok(OnlineProbe::Collection {
            title: info.title.unwrap_or_default(),
            candidates,
            unavailable_entries,
        });
    }
    if matches!(info.kind.as_str(), "url" | "url_transparent") {
        return Ok(OnlineProbe::Unresolved {
            message: crate::tr!(
                "Cannot determine which video to process yet. Paste the link to a specific video."
            )
            .into(),
        });
    }
    let extractor = extractor_name(&info).to_owned();
    let id = info.id.as_deref().context(crate::tr!(
        "Cannot identify the video for this link; paste the link to a specific video"
    ))?;
    let identity = online_identity(&extractor, id).context(crate::tr!(
        "Cannot identify the video for this link; paste the link to a specific video"
    ))?;
    let input = input.to_owned();
    let subtitles = subtitle_evidence(&info, &identity, diagnostics);
    Ok(OnlineProbe::Video {
        video: OnlineVideo {
            identity,
            meta: VideoMeta {
                title: info
                    .title
                    .filter(|title| !title.trim().is_empty())
                    .context(crate::tr!(
                        "The source returned no video title; probe again"
                    ))?,
                uploader: info.uploader.unwrap_or_default(),
                duration: info
                    .duration
                    .filter(|value| value.is_finite() && *value >= 0.)
                    .unwrap_or_default(),
                webpage_url: input,
                extractor,
                id: id.to_owned(),
            },
            thumbnail: info.thumbnail,
            original_language: info.language,
            subtitles,
        },
    })
}

fn subtitle_evidence(
    info: &ExtractorInfo,
    identity: &str,
    diagnostics: &str,
) -> crate::subtitle::SubtitleEvidence {
    use crate::subtitle::{SubtitleEvidence, SubtitleKind, SubtitleOrigin, SubtitleTrack};
    let warning = diagnostics
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            (lower.contains("subtitle") || lower.contains("caption"))
                && (lower.contains("error")
                    || lower.contains("unable")
                    || lower.contains("failed")
                    || lower.contains("login")
                    || lower.contains("logged in")
                    || lower.contains("sign in"))
        })
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n");
    let warning = (!warning.is_empty()).then_some(warning);
    let mut tracks = Vec::new();
    for (automatic, channel) in [(false, &info.subtitles), (true, &info.automatic_captions)] {
        for (language_key, formats) in channel
            .as_ref()
            .map(|channel| channel.0.as_slice())
            .unwrap_or_default()
        {
            // Timed chat is not a transcript of the video.
            if language_key == "live_chat" {
                continue;
            }
            let readable: Vec<_> = formats
                .iter()
                .filter(|format| {
                    matches!(
                        format["ext"].as_str(),
                        Some(
                            "srt"
                                | "vtt"
                                | "ttml"
                                | "srv1"
                                | "srv2"
                                | "srv3"
                                | "json3"
                                | "ass"
                                | "lrc"
                        )
                    )
                })
                .collect();
            if readable.is_empty() {
                continue;
            }
            let kind = if automatic {
                SubtitleKind::Automatic
            } else {
                SubtitleKind::Manual
            };
            let name = readable
                .iter()
                .find_map(|format| format["name"].as_str())
                .filter(|name| !name.is_empty())
                .map(str::to_owned);
            let inline_text = readable
                .iter()
                .find(|format| matches!(format["ext"].as_str(), Some("srt" | "vtt")))
                .and_then(|format| format["data"].as_str())
                .map(str::to_owned);
            tracks.push(SubtitleTrack {
                id: format!(
                    "{identity}:subtitle:{}:{language_key}",
                    if automatic { "auto" } else { "provided" }
                ),
                language: Some(language_key.clone()),
                name,
                kind,
                origin: SubtitleOrigin::Online {
                    language_key: language_key.clone(),
                    automatic,
                    inline_text,
                },
                source_order: tracks.len(),
            });
        }
    }
    if !tracks.is_empty() {
        SubtitleEvidence::Found { tracks, warning }
    } else if let Some(message) = warning {
        SubtitleEvidence::Failed { message }
    } else if info.subtitles.is_none() && info.automatic_captions.is_none() {
        SubtitleEvidence::Unsupported {
            message: crate::tr!("This source does not support reading subtitles yet; you can transcribe the video audio instead").into(),
        }
    } else {
        SubtitleEvidence::NoneFound
    }
}

impl VideoMeta {
    pub fn save(&self, path: &Path) -> Result<()> {
        crate::checkpoint::atomic_write_shared(path, serde_json::to_string_pretty(self)?.as_bytes())
    }
}

/// Probe once and return the single video; collection/unresolved links are errors.
pub async fn probe_video(cfg: &PipelineConfig) -> Result<OnlineVideo> {
    match probe_online(cfg).await? {
        OnlineProbe::Video { video } => Ok(video),
        OnlineProbe::Collection { .. } => {
            anyhow::bail!(crate::tr!(
                "This link contains multiple videos; choose a specific episode before generating notes"
            ))
        }
        OnlineProbe::Unresolved { message } => anyhow::bail!("{message}"),
    }
}

/// Metadata and all available subtitle channels only; no media/AI requests.
pub async fn probe_online(cfg: &PipelineConfig) -> Result<OnlineProbe> {
    let url = &cfg.url;
    let mut cmd = Command::new(crate::ytdlp::program());
    let _cookies = crate::auth::configure_ytdlp(
        cmd.as_std_mut(),
        cfg.cookies.as_deref(),
        cfg.cookies_from_browser.as_deref(),
    )?;
    ytdlp_base(&mut cmd).args([
        "--simulate",
        "--dump-single-json",
        "--flat-playlist",
        "--write-subs",
        "--write-auto-subs",
        "--sub-langs",
        "all",
        "--retries",
        "1",
    ]);
    let out = run_output(ytdlp_url(&mut cmd, url)).await?;
    parse_online_probe(&out.stdout, url, &String::from_utf8_lossy(&out.stderr))
}

/// 抓取的平台字幕（yt-dlp 产物）。
pub struct SubtitleFetch {
    pub path: PathBuf,
    /// true = 平台自动生成字幕（auto-caption）
    pub auto: bool,
}

/// CLI compatibility path. `video` comes from the run's single probe so discovery
/// stays consistent; select from every language, then fetch that exact track
/// without silent fallback.
pub async fn fetch_subtitle(
    video: &OnlineVideo,
    cfg: &PipelineConfig,
) -> Result<Option<SubtitleFetch>> {
    use crate::subtitle::SubtitleEvidence;
    match &video.subtitles {
        SubtitleEvidence::Found { tracks, .. } => {
            let mut tracks = tracks.clone();
            crate::subtitle::sort_tracks(
                &mut tracks,
                &cfg.asr.language,
                video.original_language.as_deref(),
            );
            let track = tracks
                .first()
                .context(crate::tr!("Subtitle list is empty; probe subtitles again"))?;
            fetch_selected_subtitle(
                &video.meta.webpage_url,
                &video.identity,
                track,
                &cfg.out_dir,
                cfg,
            )
            .await
            .map(Some)
        }
        SubtitleEvidence::Failed { message } => {
            anyhow::bail!("Could not read subtitles: {message}")
        }
        SubtitleEvidence::Unchecked => anyhow::bail!(crate::tr!(
            "Subtitles not checked yet; probe the video info again"
        )),
        SubtitleEvidence::NoneFound | SubtitleEvidence::Unsupported { .. } => Ok(None),
    }
}

/// Escape the complete language key as an exact yt-dlp subtitle regular expression.
pub fn exact_subtitle_language(key: &str) -> String {
    let mut escaped = String::from("^");
    for character in key.chars() {
        if matches!(
            character,
            '.' | '+' | '*' | '?' | '^' | '$' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('$');
    escaped
}

/// Fetch only the confirmed online track, rejecting a changed video identity.
pub async fn fetch_selected_subtitle(
    url: &str,
    source_identity: &str,
    track: &crate::subtitle::SubtitleTrack,
    out_dir: &Path,
    cfg: &PipelineConfig,
) -> Result<SubtitleFetch> {
    use crate::subtitle::SubtitleOrigin;
    let SubtitleOrigin::Online {
        language_key,
        automatic,
        inline_text,
    } = &track.origin
    else {
        anyhow::bail!(crate::tr!(
            "The selected subtitle is not an online subtitle"
        ));
    };
    ensure!(
        track
            .id
            .starts_with(&format!("{source_identity}:subtitle:")),
        crate::tr!(
            "The selected subtitle no longer matches the current video; select the subtitle again"
        )
    );
    let dir = out_dir.join(".subs");
    tokio::fs::create_dir_all(&dir).await?;
    let temp = tempfile::Builder::new()
        .prefix("selected-")
        .tempdir_in(&dir)?;
    let text = if let Some(text) = inline_text {
        text.clone()
    } else {
        let template = temp.path().join("subtitle");
        let mut cmd = Command::new(crate::ytdlp::program());
        let _cookies = crate::auth::configure_ytdlp(
            cmd.as_std_mut(),
            cfg.cookies.as_deref(),
            cfg.cookies_from_browser.as_deref(),
        )?;
        ytdlp_base(&mut cmd).args([
            "--skip-download",
            "--no-simulate",
            "--dump-single-json",
            "--no-playlist",
            "--no-write-auto-subs",
            "--no-write-subs",
            "--convert-subs",
            "srt",
            "--sub-format",
            "srt/vtt/best",
            "--sub-langs",
            &exact_subtitle_language(language_key),
            "--retries",
            "1",
            "-o",
        ]);
        cmd.arg(&template).arg(if *automatic {
            "--write-auto-subs"
        } else {
            "--write-subs"
        });
        let out = run_output(ytdlp_url(&mut cmd, url)).await?;
        match parse_online_probe(&out.stdout, url, &String::from_utf8_lossy(&out.stderr))? {
            OnlineProbe::Video { video } => ensure!(
                video.identity == source_identity,
                crate::tr!("The video source changed; probe again and reselect the subtitle")
            ),
            _ => anyhow::bail!(crate::tr!(
                "Cannot confirm the video for the subtitle; probe again"
            )),
        }
        let path = crate::subtitle::pick_subtitle_file(temp.path())
            .context(crate::tr!("The selected subtitle could not be downloaded; read subtitles again or use speech recognition"))?;
        crate::subtitle::read_subtitle_text(&path)?
    };
    let events = crate::subtitle::parse_subtitle(&text);
    ensure!(
        !events.is_empty(),
        crate::tr!("This subtitle contains no readable text; choose another subtitle")
    );
    let path = temp.path().join("selected.srt");
    crate::checkpoint::atomic_write(&path, crate::subtitle::to_srt(&events).as_bytes())?;
    let _ = temp.keep();
    Ok(SubtitleFetch {
        path,
        auto: track.kind == crate::subtitle::SubtitleKind::Automatic,
    })
}

/// 本地视频的同名字幕 sidecar（lecture.mp4 → lecture.srt/.vtt）。
pub fn sidecar_subtitle(video: &Path, preferred_language: &str) -> Option<SubtitleFetch> {
    crate::subtitle::sidecar_subtitle(video, preferred_language)
        .map(|path| SubtitleFetch { path, auto: false })
}

/// 下载视频到 `dest`（默认 1080p 上限，mp4 合并）。
/// 调用方先处理已有文件（校验或归档）：这里绝不把未校验的旧文件当作下载结果。
pub async fn download(cfg: &PipelineConfig, dest: &Path, verbose: bool) -> Result<()> {
    let url = &cfg.url;
    let max_height = cfg.max_height;
    ensure!(
        !dest.exists(),
        crate::tr!("An unverified file already exists at the video location")
    );
    if let Some(p) = dest.parent() {
        tokio::fs::create_dir_all(p).await?;
    }
    let tmp: PathBuf = dest.with_extension("mp4.part");
    // 瞬时网络错误由外层循环重试（共 DOWNLOAD_ATTEMPTS 次）；
    // 确定性错误（4xx/私有视频等）立即返回；每轮都可被任务控制文件中止。
    let mut last_err = None;
    for attempt in 0..DOWNLOAD_ATTEMPTS {
        crate::dispatch::check_control()?;
        if attempt > 0 {
            tracing::warn!(attempt, "Retrying video download");
            tokio::select! {
                _ = tokio::time::sleep(DOWNLOAD_RETRY_DELAY) => {}
                e = crate::dispatch::watch_control() => return Err(e),
            }
        }
        let mut cmd = Command::new(crate::ytdlp::program());
        let _cookies = crate::auth::configure_ytdlp(
            cmd.as_std_mut(),
            cfg.cookies.as_deref(),
            cfg.cookies_from_browser.as_deref(),
        )?;
        let fmt = format!("bv*[height<={max_height}]+ba/b[height<={max_height}]/b");
        ytdlp_base(&mut cmd).args([
            "-f",
            &fmt,
            "-S",
            "ext:mp4:m4a",
            "--merge-output-format",
            "mp4",
            "--no-playlist",
            "--no-part",
            // 结构化进度（2021.11 起的 yt-dlp 接口）：前缀行由 run_download 解析转发
            "--newline",
            "--progress-template",
            "download:[notepipe] %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s",
            "-o",
        ]);
        cmd.arg(&tmp);
        if verbose {
            cmd.arg("-v");
        }
        let outcome = tokio::select! {
            status = run_download(ytdlp_url(&mut cmd, url)) => status,
            // 取消/暂停：drop run_download 分支即 kill_on_drop 终止当前 yt-dlp
            e = crate::dispatch::watch_control() => return Err(e),
        };
        match outcome {
            Ok(()) => {
                // 新版 yt-dlp 在 merge 时会按 --merge-output-format 再补后缀：
                // -o media.mp4.part 实际产出 media.mp4.part.mp4。两种命名都兼容。
                // OsString 拼接而非 format!("{}", display())：非 UTF-8 路径也能正确处理
                let merged = {
                    let mut s = tmp.clone().into_os_string();
                    s.push(".mp4");
                    PathBuf::from(s)
                };
                let produced = if merged.is_file() {
                    merged
                } else if tmp.is_file() {
                    tmp
                } else {
                    anyhow::bail!(
                        "{}",
                        crate::tr!(
                            "Downloaded video missing (expected {} or {})",
                            tmp.display(),
                            merged.display(),
                            tmp.display(),
                            merged.display()
                        )
                    );
                };
                tokio::fs::rename(&produced, dest).await?;
                return Ok(());
            }
            Err(e) => {
                if is_deterministic_download_error(&format!("{e:#}")) {
                    return Err(e);
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!(crate::tr!("Video download failed"))))
}

async fn run_output(cmd: &mut Command) -> Result<std::process::Output> {
    let out = cmd
        .kill_on_drop(true)
        .output()
        .await
        .context(crate::tr!("Failed to start yt-dlp"))?;
    if out.status.success() {
        Ok(out)
    } else {
        Err(crate::error::cmd_error(
            "yt-dlp",
            out.status.code(),
            &String::from_utf8_lossy(&out.stderr),
        ))
    }
}

/// 解析一行 yt-dlp 进度模板输出：(downloaded, total)。
/// 模板字段：<downloaded> <total> <estimate>，未知值为 NA；
/// 精确总字节未知时回落到估算值，都没有则 total=0（GUI 显示不确定进度）。
fn ytdlp_progress(line: &str) -> Option<(u64, u64)> {
    let rest = line.strip_prefix("[notepipe] ")?;
    let mut fields = rest.split_whitespace();
    let number = |value: Option<&str>| {
        value
            .filter(|value| *value != "NA")
            .and_then(|value| value.parse::<u64>().ok())
    };
    let downloaded = number(fields.next())?;
    let total = number(fields.next())
        .or_else(|| number(fields.next()))
        .unwrap_or(0);
    Some((downloaded, total))
}

fn parse_ytdlp_progress(line: &str) {
    if let Some((downloaded, total)) = ytdlp_progress(line) {
        crate::progress::download_progress(downloaded, total);
    }
}

/// 运行 yt-dlp 下载：流式读取 stdout/stderr 并实时转发字节进度。
/// 错误语义与 media::run_cmd 一致（stderr 尾部进错误）；
/// 三个读取分支同属一个 future，drop 即整体取消（kill_on_drop 杀子进程）。
async fn run_download(cmd: &mut Command) -> Result<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut child = cmd
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context(crate::tr!("Failed to start yt-dlp"))?;
    let mut stdout = BufReader::new(
        child
            .stdout
            .take()
            .expect(crate::tr!("The stdout pipe is configured")),
    )
    .lines();
    let mut stderr_pipe = BufReader::new(
        child
            .stderr
            .take()
            .expect(crate::tr!("The stderr pipe is configured")),
    )
    .lines();
    let wait = child.wait();
    let out = async {
        while let Ok(Some(line)) = stdout.next_line().await {
            parse_ytdlp_progress(&line);
        }
    };
    let err = async {
        use std::collections::VecDeque;
        // Keep the tail, not the head: yt-dlp ends with the actionable error.
        const STDERR_TAIL_BYTES: usize = 64 * 1024;
        let mut tail: VecDeque<String> = VecDeque::new();
        let mut bytes = 0usize;
        while let Ok(Some(line)) = stderr_pipe.next_line().await {
            parse_ytdlp_progress(&line);
            bytes += line.len() + 1;
            tail.push_back(line);
            while bytes > STDERR_TAIL_BYTES {
                let Some(old) = tail.pop_front() else { break };
                bytes = bytes.saturating_sub(old.len() + 1);
            }
        }
        tail.into_iter().collect::<Vec<_>>().join("\n")
    };
    let (status, (), stderr) = tokio::join!(wait, out, err);
    let status = status.context(crate::tr!("Failed waiting for yt-dlp to exit"))?;
    if status.success() {
        if !stderr.trim().is_empty() {
            tracing::debug!("{}", stderr.trim());
        }
        return Ok(());
    }
    Err(crate::error::cmd_error("yt-dlp", status.code(), &stderr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subtitles_prefer_asr_language_then_original_language() {
        let cue = |text| {
            serde_json::json!([{
                "ext": "srt", "data": format!("1\n00:00:00,000 --> 00:00:01,000\n{text}\n")
            }])
        };
        let metadata = serde_json::json!({
            "id": "one", "title": "One", "extractor": "test", "language": "en",
            "subtitles": { "ja": cue("Japanese") },
            "automatic_captions": { "zh": cue("Chinese"), "en": cue("English") }
        });
        let OnlineProbe::Video { video } = parse_online_probe(
            &serde_json::to_vec(&metadata).unwrap(),
            "https://example.invalid/one",
            "",
        )
        .unwrap() else {
            panic!("expected video")
        };
        let dir = tempfile::tempdir().unwrap();
        for (language, expected) in [("", "English"), ("ja", "Japanese")] {
            let mut file = crate::settings::ConfigFile::default();
            file.asr.language = language.into();
            let opts = crate::cli::RunOpts {
                out: Some(dir.path().into()),
                ..Default::default()
            };
            let cfg = crate::options::resolve("video".into(), &opts, &file).unwrap();
            let subtitle = fetch_subtitle(&video, &cfg).await.unwrap().unwrap();
            let events =
                crate::subtitle::parse_subtitle(&std::fs::read_to_string(subtitle.path).unwrap());
            assert_eq!(events[0].text, expected);
            assert_eq!(subtitle.auto, language.is_empty());
        }
    }

    #[test]
    fn deterministic_download_errors_skip_the_retry_loop() {
        for text in [
            "ERROR: HTTP Error 404: Not Found",
            "HTTP Error 403: Forbidden",
            "ERROR: Unsupported URL: https://example.com/x",
            "ERROR: Private video. Use --cookies for authentication",
        ] {
            assert!(is_deterministic_download_error(text), "{text}");
        }
        for text in ["network unreachable", "timed out"] {
            assert!(!is_deterministic_download_error(text), "{text}");
        }
    }

    #[test]
    fn collection_is_never_implicitly_the_first_part() {
        let result = parse_online_probe(
            include_bytes!("../tests/fixtures/source/playlist.json"),
            "https://example.invalid/playlist",
            "",
        )
        .unwrap();
        let OnlineProbe::Collection {
            candidates,
            unavailable_entries,
            ..
        } = result
        else {
            panic!("expected collection")
        };
        assert_eq!(candidates.len(), 2);
        assert_eq!(
            candidates[1].input,
            "https://example.invalid/watch?episode=2"
        );
        assert_eq!(unavailable_entries, 1);
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.identity.is_none())
        );
    }

    #[test]
    fn exact_part_identity_and_all_language_evidence_survive_serialization() {
        let OnlineProbe::Video { video } = parse_online_probe(
            include_bytes!("../tests/fixtures/source/multilingual-video.json"),
            "https://example.invalid/watch?episode=2",
            "",
        )
        .unwrap() else {
            panic!("expected video")
        };
        assert!(video.identity.ends_with("fixture-part-2"));
        assert_eq!(
            video.meta.webpage_url,
            "https://example.invalid/watch?episode=2"
        );
        let tracks = video.subtitles.tracks();
        assert_eq!(
            tracks
                .iter()
                .map(|track| track.language.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["fr", "ja", "zh"]
        );
        assert_eq!(tracks[0].kind, crate::subtitle::SubtitleKind::Manual);
        assert_eq!(tracks[2].kind, crate::subtitle::SubtitleKind::Automatic);
        assert_eq!(
            video,
            serde_json::from_str::<OnlineVideo>(&serde_json::to_string(&video).unwrap()).unwrap()
        );
    }

    #[test]
    fn permission_failure_never_means_no_captions() {
        let bytes = include_bytes!("../tests/fixtures/source/no-captions.json");
        let OnlineProbe::Video { video } =
            parse_online_probe(bytes, "https://example.invalid", "").unwrap()
        else {
            panic!()
        };
        assert!(matches!(
            video.subtitles,
            crate::subtitle::SubtitleEvidence::NoneFound
        ));
        let OnlineProbe::Video { video } = parse_online_probe(
            bytes,
            "https://example.invalid",
            "WARNING: Subtitles are only available when logged in",
        )
        .unwrap() else {
            panic!()
        };
        assert!(matches!(
            video.subtitles,
            crate::subtitle::SubtitleEvidence::Failed { .. }
        ));
        let OnlineProbe::Video { video } = parse_online_probe(
            br#"{"id":"one","title":"One","extractor":"test"}"#,
            "https://example.invalid/one",
            "",
        )
        .unwrap() else {
            panic!()
        };
        assert!(matches!(
            video.subtitles,
            crate::subtitle::SubtitleEvidence::Unsupported { .. }
        ));
    }

    #[test]
    fn fragmented_media_does_not_become_a_selectable_first_fragment() {
        let result = parse_online_probe(br#"{"_type":"multi_video","id":"a","title":"Video","entries":[{"id":"a_0","url":"https://example.invalid/fragment.flv"}]}"#, "https://example.invalid/video", "").unwrap();
        assert!(matches!(result, OnlineProbe::Unresolved { .. }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_errors_retain_stderr() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo 'ERROR: HTTP Error 403: Forbidden' >&2; exit 1"]);
        let error = run_download(&mut cmd).await.unwrap_err();
        assert!(format!("{error:#}").contains("HTTP Error 403: Forbidden"));
    }

    #[test]
    fn ytdlp_progress_lines_parse_bytes_with_estimate_fallback() {
        assert_eq!(
            ytdlp_progress("[notepipe] 120326 10485760 NA"),
            Some((120326, 10485760))
        );
        assert_eq!(
            ytdlp_progress("[notepipe] 120326 NA 20971520"),
            Some((120326, 20971520))
        );
        assert_eq!(ytdlp_progress("[notepipe] 120326 NA NA"), Some((120326, 0)));
        assert_eq!(ytdlp_progress("[notepipe] NA NA NA"), None);
        assert_eq!(ytdlp_progress("[download] 45.3% of 10MiB"), None);
        assert_eq!(ytdlp_progress(""), None);
    }
}
