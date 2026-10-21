//! Screenshot detection, modeled on yt-slide-mark.
//! A frame is sampled every `sample_interval` seconds and compared with the last kept frame (SSIM);
//! below `similarity` it becomes a screenshot, and the next `cooldown` seconds are skipped.

use crate::config::{PipelineConfig, Roi};
use crate::media;
use crate::timeline::FrameEvent;
use anyhow::{Context, Result};
use image::GrayImage;
use image_compare::Algorithm;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::Path;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// Width of sampled frames: enough to see layout changes while keeping decoding and SSIM cheap.
const SAMPLE_WIDTH: u32 = 640;
/// Parallel full-resolution extractions; each is one ffmpeg process.
const EXTRACT_CONCURRENCY: usize = 4;

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum Output {
    Frames(Vec<FrameEvent>),
    NoVideo,
}

fn scaled_wh(ow: u32, oh: u32, target_w: u32) -> (u32, u32) {
    let h = ((oh as u64 * target_w as u64) / ow.max(1) as u64) as u32;
    (target_w, (h & !1).max(2))
}

/// Borrows the frame when there is no region of interest; otherwise returns the cropped copy.
fn crop_roi(img: &GrayImage, roi: Option<Roi>) -> Cow<'_, GrayImage> {
    let Some(r) = roi else {
        return Cow::Borrowed(img);
    };
    let (w, h) = img.dimensions();
    let (x1, y1, x2, y2) = r.pixels(w, h);
    Cow::Owned(
        image::imageops::crop_imm(img, x1, y1, (x2 - x1).max(1), (y2 - y1).max(1)).to_image(),
    )
}

fn ssim(a: &GrayImage, b: &GrayImage) -> f64 {
    image_compare::gray_similarity_structure(&Algorithm::MSSIMSimple, a, b)
        .map(|s| s.score)
        .unwrap_or(0.0)
}

/// Extracts one full-resolution JPEG at an exact time.
async fn extract_frame(media: &Path, t: f64, dest: &Path) -> Result<()> {
    let ss = format!("{t:.3}");
    // -q:v 2 is a high JPEG quality (2–31, lower is better), so text on screen stays readable.
    media::run_cmd(
        Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-ss", &ss])
            .arg("-i")
            .arg(media)
            .args(["-map", "0:V:0", "-frames:v", "1", "-q:v", "2"])
            .arg(dest),
        "ffmpeg",
    )
    .await?;
    anyhow::ensure!(
        dest.is_file(),
        "ffmpeg did not produce screenshot {0}",
        dest.display()
    );
    Ok(())
}

/// File name of the n-th screenshot, counting from 1.
fn slide_name(n: usize) -> String {
    format!("slide_{n:04}.jpg")
}

/// Samples a grayscale stream every `sample_interval` and returns the times of new screenshots.
async fn sample_timestamps(
    cfg: &PipelineConfig,
    media: &Path,
    info: &media::VideoInfo,
) -> Result<Vec<(f64, f64)>> {
    let interval = cfg.sample_interval;
    let (tw, th) = scaled_wh(info.width, info.height, SAMPLE_WIDTH);
    let fps = 1.0 / interval;
    let vf = format!("fps={fps:.6},scale={tw}:{th},format=gray");

    tracing::info!(
        w = info.width,
        h = info.height,
        duration = format_args!("{:.0}s", info.duration),
        interval,
        sensitivity = cfg.similarity,
        min_gap = cfg.cooldown,
        "screenshot sample"
    );

    let mut child = Command::new("ffmpeg")
        .kill_on_drop(true)
        .args(["-hide_banner", "-loglevel", "error", "-an"])
        .arg("-i")
        .arg(media)
        .args(["-map", "0:V:0", "-vf", &vf, "-f", "rawvideo", "pipe:1"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("Could not start ffmpeg frame sampling")?;
    let mut stdout = child.stdout.take().context("ffmpeg stdout")?;
    // Drain stderr continuously: an unread pipe fills its 64 KB buffer and deadlocks ffmpeg.
    // Its tail is kept for error messages.
    let mut stderr_buf: Vec<u8> = Vec::new();
    let mut stderr = child.stderr.take().context("ffmpeg stderr")?;
    let stderr_task = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let _ = stderr.read_to_end(&mut stderr_buf).await;
        stderr_buf
    });
    let frame_len = (tw as usize) * (th as usize);
    let mut buf = vec![0u8; frame_len];
    // Duration/interval gives an estimate, not the number ffmpeg will actually
    // decode (especially for VFR input). The bar reports this estimate as the
    // denominator so users see forward progress; actual samples may land one
    // or two frames off, which the label tolerates. Candidate selection
    // happens within this scan.
    let estimate = (info.duration / interval).round().max(1.0) as u64;
    let pb = crate::progress::Bar::new("scenes/scan", estimate)
        .with_template("{spinner:.green} sample {pos}/{len} frames {msg}");
    pb.set_position(0);

    // Detection never pauses; cooldown only limits how often a screenshot is taken, so nothing is missed.
    //   last_emitted  the picture of the last screenshot
    //   candidate     the new picture being watched, with the time it first appeared
    // A screenshot is taken when the candidate differs from the last one, has been stable long enough and cooldown has passed.
    let stable_for = if matches!(cfg.screenshot_mode, crate::config::ScreenshotMode::Stable) {
        cfg.stable_secs
    } else {
        0.0
    };
    let mut times: Vec<(f64, f64)> = vec![];
    let mut last_emitted: Option<GrayImage> = None;
    let mut candidate: Option<GrayImage> = None;
    let mut candidate_first_t: Option<f64> = None;
    let mut candidate_last_t: Option<f64> = None;
    let mut last_emit_t: f64 = -f64::INFINITY;
    let mut i: u64 = 0;
    loop {
        crate::runtime::check_cancelled()?;
        match stdout.read_exact(&mut buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
        // Nominal time t = i * interval. Rounding in the fps filter and variable frame rates shift real frames
        // by less than one interval, which is fine at this precision,
        // so frame timestamps are not parsed.
        let t = i as f64 * interval;
        i += 1;
        pb.inc(1);
        // Reuse the frame buffer: move it into a GrayImage and take it back with into_raw;
        // only a new candidate is copied.
        let gray = GrayImage::from_raw(tw, th, std::mem::take(&mut buf))
            .ok_or_else(|| anyhow::anyhow!("Grayscale frame size mismatch {tw}x{th}"))?;
        {
            let cmp = crop_roi(&gray, cfg.roi);

            let differs_from_emitted = match &last_emitted {
                None => true,
                Some(prev) => {
                    prev.dimensions() != cmp.dimensions() || ssim(prev, &cmp) < cfg.similarity
                }
            };
            if !differs_from_emitted {
                // Back to the last screenshot's picture: the candidate was a transition, drop it.
                candidate = None;
                candidate_first_t = None;
            } else {
                // Different from the last screenshot: track the candidate, restarting if it keeps changing (an animation).
                let candidate_changed = match &candidate {
                    None => true,
                    Some(c) => c.dimensions() != cmp.dimensions() || ssim(c, &cmp) < cfg.similarity,
                };
                if candidate_changed {
                    // Copy the frame only when the candidate changes.
                    candidate = Some(cmp.into_owned());
                    candidate_first_t = Some(t);
                    candidate_last_t = Some(t);
                } else {
                    // Same picture: move the capture time to the latest stable frame.
                    candidate_last_t = Some(t);
                }
                let onset_t = candidate_first_t.unwrap_or(t);
                let capture_t = candidate_last_t.unwrap_or(t);
                let stable = t - onset_t >= stable_for;
                let gap_ok = t - last_emit_t >= cfg.cooldown;
                if stable && gap_ok {
                    // Two times: onset places the screenshot in the timeline, capture is the frame that is extracted
                    // (in stable mode, after the transition has settled).
                    times.push((onset_t, capture_t));
                    last_emitted = candidate.take();
                    candidate_first_t = None;
                    candidate_last_t = None;
                    last_emit_t = t;
                    pb.set_message(format!("Found {} candidate screenshots", times.len()));
                }
            }
        }
        buf = gray.into_raw();
    }
    // At the end, keep a stable last candidate even within the cooldown; otherwise the final slide would be lost.
    if let (Some(_), Some(onset), Some(capture)) = (&candidate, candidate_first_t, candidate_last_t)
    {
        let stable = capture - onset >= stable_for;
        if stable {
            times.push((onset, capture));
        }
    }
    let status = child.wait().await?;
    let stderr_bytes = stderr_task.await.unwrap_or_default();
    if !status.success() {
        let tail = crate::error::tail_lines(&String::from_utf8_lossy(&stderr_bytes), 5);
        anyhow::bail!("Frame sampling failed ({status}): {tail}");
    }
    pb.finish();
    tracing::info!(screenshots = times.len(), frames = i, mode = %cfg.screenshot_mode, "ssim scan done");
    Ok(times)
}

/// Runs screenshot detection and extraction.
pub async fn run(cfg: &PipelineConfig, media: &Path) -> Result<Output> {
    let Some(info) = media::probe_video(media).await? else {
        return Ok(Output::NoVideo);
    };
    let frames_dir = cfg.frames_dir();
    tokio::fs::create_dir_all(&frames_dir).await?;
    let t0 = std::time::Instant::now();
    crate::progress::stage("scenes/scan", "start");
    let times = sample_timestamps(cfg, media, &info).await?;
    crate::progress::stage("scenes/scan", "done");
    anyhow::ensure!(!times.is_empty(), "No frames sampled");

    crate::progress::stage("scenes/extract", "start");
    let pb = crate::progress::Bar::new("scenes/extract", times.len() as u64)
        .with_template("{spinner:.green} extract {pos}/{len} [{bar:32.cyan/blue}] {msg}");
    // Emit before launching ffmpeg: the first image may take a long time.
    pb.set_position(0);
    // Extract with at most EXTRACT_CONCURRENCY ffmpeg processes and sort the results by index,
    // so names and order match a sequential run.
    let mut set: tokio::task::JoinSet<(usize, Result<()>)> = tokio::task::JoinSet::new();
    let mut pending = times.iter().copied().enumerate();
    let mut results: Vec<(usize, Result<()>)> = Vec::with_capacity(times.len());
    loop {
        crate::runtime::check_cancelled()?;
        while set.len() < EXTRACT_CONCURRENCY {
            let Some((i, (_, capture_t))) = pending.next() else {
                break;
            };
            // Extract at the capture time; place in the timeline at the onset.
            let path = frames_dir.join(slide_name(i + 1));
            let media = media.to_path_buf();
            set.spawn(async move { (i, extract_frame(&media, capture_t, &path).await) });
        }
        match set.join_next().await {
            Some(Ok((i, Ok(())))) => {
                pb.inc(1);
                results.push((i, Ok(())));
            }
            Some(Ok((i, Err(error)))) => {
                set.abort_all();
                let (onset_t, _) = times[i];
                return Err(error).with_context(|| {
                    format!(
                        "Could not extract screenshot {n} (t={onset_t:.1}s)",
                        n = i + 1
                    )
                });
            }
            Some(Err(e)) => {
                return Err(e).context("Screenshot extraction stopped unexpectedly");
            }
            None => break,
        }
    }
    results.sort_by_key(|(i, _)| *i);
    let mut frames = Vec::with_capacity(results.len());
    for (i, r) in results {
        let (onset_t, _) = times[i];
        r.with_context(|| {
            format!(
                "Could not extract screenshot {n} (t={onset_t:.1}s)",
                n = i + 1
            )
        })?;
        let name = slide_name(i + 1);
        frames.push(FrameEvent {
            t: onset_t,
            image: format!("frames/{name}"),
        });
    }
    tracing::info!(
        frames = frames.len(),
        secs = format_args!("{:.1}", t0.elapsed().as_secs_f64()),
        "screenshots extracted"
    );
    pb.finish();
    crate::progress::stage("scenes/extract", "done");
    Ok(Output::Frames(frames))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_keeps_even_height() {
        let (w, h) = scaled_wh(1280, 410, 640);
        assert_eq!(w, 640);
        assert_eq!(h % 2, 0);
    }
}
