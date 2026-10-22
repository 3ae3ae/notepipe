//! Screenshot detection on synthetic video.
//! ffmpeg generates videos of solid-color "slides" on the fly (no fixtures), and the tests check that:
//! - slide changes are detected at the time the slide first appears, not when the cooldown ends
//! - detection continues during the cooldown, so later slides keep their real start times
//!
//! Needs ffmpeg; skipped without it.

#![cfg(feature = "integration")]

use std::process::Command;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Writes a 20 s 1280x720 video of full-screen white, black and gray, which SSIM tells apart
/// (no drawtext, so ffmpeg builds without freetype work):
/// 0–6.5 s white, 6.5–7.5 s black (a transition), 7.5–14.5 s gray, 14.5–20 s white.
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

fn sections(
    video: &std::path::Path,
    dir: &std::path::Path,
    options: &[&str],
) -> Vec<serde_json::Value> {
    use std::io::Write;
    std::fs::write(
        video.with_extension("srt"),
        "1\n00:00:00,000 --> 00:00:20,000\nScene fixture.\n",
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_notepipe"))
        .arg(video)
        .args([
            "--config",
            "-",
            "--json",
            "--sensitivity",
            "0.9",
            "--frame-interval",
            "0.5",
        ])
        .args(options)
        .arg("--out")
        .arg(dir.join("notes"))
        .current_dir(dir)
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env_remove("RUST_LOG")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(br#"{"pipeline":["download","transcript","screenshots","draft","markdown"],"stages":{"download":{"type":"download","input":"source","output":"media","command":["unused"]},"transcript":{"type":"transcript","input":"media","output":"transcript.json","sources":["subtitles"]},"screenshots":{"type":"screenshots","input":"media","output":"images"},"draft":{"type":"document","input":["transcript.json","images"],"output":"draft.json","publish":"note.json"},"markdown":{"type":"markdown","input":"draft.json","output":"note.md","publish":"note.md"}}}"#).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let done: serde_json::Value = serde_json::from_str(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .last()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(done["type"], "done");
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(done["files"]["draft"].as_str().unwrap()).unwrap())
            .unwrap();
    document["sections"].as_array().unwrap().clone()
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

    let frames = sections(
        &video,
        &dir,
        &[
            "--min-gap",
            "10",
            "--screenshot-mode",
            "first",
            "--settle",
            "0",
        ],
    );

    // Expected with cooldown 10 s in first mode:
    //   "A" at 0 s is kept; "B" at 6.5 s becomes the candidate and is replaced by "C" at 7.5 s;
    //   when the cooldown ends at ~10 s, "C" is kept with its start time 7.5 s, not 10 s;
    //   "D" at 14.5 s is within 10 s of the last screenshot and is skipped.
    let ts: Vec<f64> = frames
        .iter()
        .map(|frame| frame["t"].as_f64().unwrap())
        .collect();
    assert!(ts.len() >= 2, "at least 2 screenshots, got {ts:?}");
    assert!(
        (ts[0] - 0.0).abs() < 1.0,
        "first screenshot ~0 s, got {}",
        ts[0]
    );
    assert!(
        (ts[1] - 7.5).abs() < 1.0,
        "second screenshot at C's start 7.5 s, not the end of min_gap at ~10 s; got {} (all: {ts:?})",
        ts[1]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Stable mode: intermediate pictures shorter than settle are skipped and only the settled one is kept,
/// placed at its onset and captured once it is stable.
#[test]
fn stable_mode_waits_for_settled_state() {
    if !have_ffmpeg() {
        eprintln!("skip: ffmpeg not found");
        return;
    }
    let dir = std::env::temp_dir().join(format!("notepipe-stable-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let video = dir.join("synthetic.mp4");
    // 0–2 s white (kept as the first screenshot); 2–3.5 s black (a transition, three 0.5 s samples);
    // 3.5–10 s gray (the settled picture)
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

    let frames = sections(
        &video,
        &dir,
        &[
            "--min-gap",
            "2",
            "--screenshot-mode",
            "stable",
            "--settle",
            "1.2",
        ],
    );
    let ts: Vec<f64> = frames
        .iter()
        .map(|frame| frame["t"].as_f64().unwrap())
        .collect();
    // Whether the black picture (1.5 s, longer than settle 1.2 s) is kept depends on sampling phase,
    // so only the first screenshot (~0 s) and the gray onset (~3.5 s) are asserted.
    assert!(
        ts.len() >= 2,
        "at least 2 screenshots in stable mode, got {ts:?}"
    );
    assert!((ts[0] - 0.0).abs() < 1.0, "first slide ~0s, got {}", ts[0]);
    let gray = ts.iter().find(|&&t| (t - 3.5).abs() < 1.0);
    assert!(gray.is_some(), "gray onset ~3.5 s, got {ts:?}");
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
            .args(["--json", "--config", "-"])
            .current_dir(root.path())
            .env("XDG_CACHE_HOME", root.path().join("cache"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(br#"{"pipeline":["download","transcript","screenshots","draft","markdown"],"stages":{"download":{"type":"download","input":"source","output":"media","command":["unused"]},"transcript":{"type":"transcript","input":"media","output":"transcript.json","sources":["subtitles"]},"screenshots":{"type":"screenshots","input":"media","output":"images"},"draft":{"type":"document","input":["transcript.json","images"],"output":"draft.json","publish":"note.json"},"markdown":{"type":"markdown","input":"draft.json","output":"note.md","publish":"note.md"}}}"#)?;
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
                .is_some_and(|stage| stage.starts_with("screenshots/"))
        }));
        let done = events.last().unwrap();
        assert_eq!(done["type"], "done");
        assert!(done["warnings"].as_array().unwrap().is_empty());
        let note = std::fs::read_to_string(done["files"]["markdown"].as_str().unwrap()).unwrap();
        assert!(note.contains("An audio-only note.") && !note.contains("!["));
        let document: serde_json::Value = serde_json::from_slice(
            &std::fs::read(done["files"]["draft"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(document["sections"][0]["image"], "");
    }
}
