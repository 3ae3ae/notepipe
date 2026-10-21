//! 场景检测的合成视频回归测试。
//! 用 ffmpeg 现场生成“色块换页”视频（无需提交 fixture），验证状态机的关键行为：
//! - 换页被检出、时间戳为候选首次出现时间（而非 cooldown 到期时间）
//! - cooldown 期间检测不休眠：跳过的中间页、后续页真实起点仍正确
//!
//! 运行需要 ffmpeg；不存在时跳过。

#![cfg(feature = "integration")]

use std::process::Command;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 生成 20s 1280x720 测试视频（黑/灰/白整屏填充，亮度差异大、SSIM 可区分；
/// 不用 drawtext 以兼容无 freetype 的 ffmpeg 构建）：
/// 0-6.5s 白、6.5-7.5s 黑（模拟过渡页）、7.5-14.5s 灰、14.5-20s 白。
fn make_test_video(path: &std::path::Path) {
    let fill = |color: &str, from: &str, to: &str| {
        format!("drawbox=color={color}:t=fill:enable='between(t,{from},{to})'")
    };
    let filter = format!(
        "{},{},{}",
        fill("black", "6.5", "7.5"),
        fill("gray", "7.5", "14.5"),
        fill("white", "14.5", "20")
    );
    let st = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .args(["-i", "color=c=white:s=1280x720:d=20:r=10"])
        .args(["-vf", &filter])
        .arg(path)
        .status()
        .expect("spawn ffmpeg");
    assert!(st.success());
}

/// 两个场景测试共享的 PipelineConfig 基座；差异字段在调用处用结构体更新语法覆盖。
fn test_cfg(video: &std::path::Path, dir: &std::path::Path) -> notepipe::config::PipelineConfig {
    notepipe::config::PipelineConfig {
        url: video.display().to_string(),
        config_path: None,
        on_complete: Vec::new(),
        out_dir: dir.to_path_buf(),
        out_root: dir.to_path_buf(),
        similarity: 0.9,
        sample_interval: 0.5,
        cooldown: 10.0,
        screenshot_mode: notepipe::config::ScreenshotMode::First,
        stable_secs: 0.0,
        roi: None,
        no_resume: false,
        subtitle: None,
        llm: Default::default(),
        source: Default::default(),
        asr: Default::default(),
        transcript_source: notepipe::config::TranscriptSource::Asr,
    }
}

#[test]
fn scene_detects_slides_with_true_timestamps() {
    if !have_ffmpeg() {
        eprintln!("skip: ffmpeg not found");
        return;
    }
    let dir = std::env::temp_dir().join(format!("notepipe-scene-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let video = dir.join("synthetic.mp4");
    make_test_video(&video);

    let cfg = test_cfg(&video, &dir);

    let notepipe::scene::Output::Frames(frames) = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(notepipe::scene::run(&cfg, &video))
        .expect("scene run")
    else {
        panic!("Expected a video stream");
    };

    // 期望（cooldown=10s、first 模式）：
    //   0s "A" 被发射；6.5s "B" 成为候选、7.5s 被 "C" 替换（候选持续更新，检测无盲区）；
    //   cooldown 在 ~10s 结束时发射的是 "C" 的首次出现时间 7.5s（而非 10s）；
    //   14.5s "D" 距上次发射 <10s，被跳过。
    let ts: Vec<f64> = frames.iter().map(|f| f.t).collect();
    assert!(ts.len() >= 2, "至少应检出 2 帧，got {ts:?}");
    assert!(
        (ts[0] - 0.0).abs() < 1.0,
        "第一帧应为 0s 附近，got {}",
        ts[0]
    );
    assert!(
        (ts[1] - 7.5).abs() < 1.0,
        "第二帧应为 C 页首次出现时间 7.5s（而非 cooldown 到期的 ~10s），got {}（全部：{ts:?}）",
        ts[1]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// stable 模式：动画中间态（A→A'→A''，每步 < stable_secs）应只发射最终稳定态，
/// 且时间线用 onset（候选首现），截帧用 capture（确认稳定时）。
#[test]
fn stable_mode_waits_for_settled_state() {
    if !have_ffmpeg() {
        eprintln!("skip: ffmpeg not found");
        return;
    }
    let dir = std::env::temp_dir().join(format!("notepipe-stable-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let video = dir.join("synthetic.mp4");
    // 0-2s 白（短页，会被 min 发射间隔保留为第一帧）；2-3.5s 黑（transition，0.5s 间隔 ×3）；
    // 3.5-10s 灰（最终稳定态）
    let filter = "drawbox=color=black:t=fill:enable='between(t,2,3.5)',\
drawbox=color=gray:t=fill:enable='gte(t,3.5)'";
    let st = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .args(["-i", "color=c=white:s=1280x720:d=10:r=10"])
        .args(["-vf", filter])
        .arg(&video)
        .status()
        .expect("spawn ffmpeg");
    assert!(st.success());

    let cfg = notepipe::config::PipelineConfig {
        cooldown: 2.0,
        screenshot_mode: notepipe::config::ScreenshotMode::Stable,
        stable_secs: 1.2,
        ..test_cfg(&video, &dir)
    };

    let notepipe::scene::Output::Frames(frames) = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(notepipe::scene::run(&cfg, &video))
        .expect("scene run")
    else {
        panic!("Expected a video stream");
    };
    let ts: Vec<f64> = frames.iter().map(|f| f.t).collect();
    // 黑页（2~3.5s，持续 1.5s > stable_secs=1.2s）是否被单独发射取决于采样相位，
    // 非确定行为；这里只断言确定性行为：首页 ~0s 与稳定灰页 onset ~3.5s 都存在。
    assert!(ts.len() >= 2, "stable 模式至少检出 2 页，got {ts:?}");
    assert!((ts[0] - 0.0).abs() < 1.0, "first slide ~0s, got {}", ts[0]);
    let gray = ts.iter().find(|&&t| (t - 3.5).abs() < 1.0);
    assert!(gray.is_some(), "灰页 onset 应为 3.5s 附近，got {ts:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn audio_only_and_attached_cover_inputs_publish_without_screenshot_warnings() {
    if !have_ffmpeg() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let cover = root.path().join("cover.jpg");
    image::RgbImage::new(32, 32).save(&cover).unwrap();
    for attached in [false, true] {
        let media = root
            .path()
            .join(if attached { "covered.m4a" } else { "audio.wav" });
        let mut generate = Command::new("ffmpeg");
        generate.args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2",
        ]);
        if attached {
            generate.arg("-i").arg(&cover).args([
                "-map",
                "0:a",
                "-map",
                "1:v",
                "-c:a",
                "aac",
                "-c:v",
                "mjpeg",
                "-disposition:v:0",
                "attached_pic",
            ]);
        }
        let output = generate.arg(&media).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if attached {
            let probe = Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream_disposition=attached_pic",
                    "-of",
                    "json",
                ])
                .arg(&media)
                .output()
                .unwrap();
            assert!(probe.status.success());
            let stream: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
            assert_eq!(stream["streams"][0]["disposition"]["attached_pic"], 1);
        }
        let subtitle = media.with_extension("srt");
        std::fs::write(
            &subtitle,
            "1\n00:00:00,000 --> 00:00:02,000\nAn audio-only note.\n",
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_notepipe"))
            .arg(&media)
            .args(["--transcript-source", "subtitle", "--json", "--config", "-"])
            .current_dir(root.path())
            .env("XDG_CACHE_HOME", root.path().join("cache"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(b"{}")?;
                child.wait_with_output()
            })
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let events: Vec<serde_json::Value> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(!events.iter().any(|event| {
            event["stage"]
                .as_str()
                .is_some_and(|stage| stage.starts_with("scenes/"))
        }));
        let done = events.last().unwrap();
        assert_eq!(done["type"], "done");
        assert!(done["warnings"].as_array().unwrap().is_empty());
        let note = std::fs::read_to_string(done["markdown"].as_str().unwrap()).unwrap();
        assert!(note.contains("An audio-only note.") && !note.contains("!["));
        let document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(done["json"].as_str().unwrap()).unwrap())
                .unwrap();
        assert!(document["warnings"].as_array().unwrap().is_empty());
        assert_eq!(document["sections"][0]["image"], "");
    }
}
