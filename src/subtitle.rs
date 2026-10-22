//! Subtitles as the transcript: SRT/VTT parsing, sidecar discovery and language preference.
//!
//! Like speech recognition, they produce `TranscriptEvent`s for the rest of the pipeline.

use crate::timeline::TranscriptEvent;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A track is a discovery result, not proof that its text can be read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubtitleTrack {
    pub language: Option<String>,
    pub kind: SubtitleKind,
    pub path: PathBuf,
    #[serde(default)]
    pub source_order: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleKind {
    Manual,
    Automatic,
    Unknown,
}

fn normalize_language(language: &str) -> String {
    language.trim().replace('_', "-").to_ascii_lowercase()
}

/// ASR language → known original language → discovery
/// order. A missed preference always falls through, never removes other languages.
/// A generic preference such as `zh` covers its language family equally; a
/// script/region preference such as `zh-Hans` prefers that exact variant.
/// Within an identical language, known human captions precede known automatic ones.
pub fn sort_tracks(
    tracks: &mut [SubtitleTrack],
    preferred_language: &str,
    original_language: Option<&str>,
) {
    let mut languages = Vec::new();
    for language in std::iter::once(preferred_language).chain(original_language) {
        let value = normalize_language(language);
        if !value.is_empty() && !languages.contains(&value) {
            languages.push(value);
        }
    }
    // Precompute one rank for each exact language group. This preserves the
    // extractor's order between remaining languages, including non-Latin tracks.
    let mut groups: Vec<Option<String>> = Vec::new();
    let mut discovery: Vec<_> = tracks.iter().collect();
    discovery.sort_by_key(|track| track.source_order);
    for track in discovery {
        let language = track.language.as_deref().map(normalize_language);
        if !groups.contains(&language) {
            groups.push(language);
        }
    }
    let rank = |track: &SubtitleTrack| {
        let language = track.language.as_deref().map(normalize_language);
        let preferred = language
            .as_ref()
            .and_then(|language| {
                languages
                    .iter()
                    .enumerate()
                    .find_map(|(index, preference)| {
                        let same = language == preference;
                        // shortcut: tags are compared as written (primary subtag, case-insensitive), so "eng" does not match "en"; add an ISO 639-2 to 639-1 table if local sidecars commonly use three-letter codes
                        let family = language.split('-').next() == preference.split('-').next();
                        (same || family).then_some((
                            index,
                            if same || !preference.contains('-') {
                                0
                            } else {
                                1
                            },
                        ))
                    })
            })
            .unwrap_or((languages.len(), 0));
        let group = groups
            .iter()
            .position(|group| group == &language)
            .unwrap_or(usize::MAX);
        (preferred, group)
    };
    tracks.sort_by_key(rank);
    // Unknown labels retain their positions. Only reorder slots whose type the
    // source explicitly distinguishes, avoiding invented claims about a track.
    let mut start = 0;
    while start < tracks.len() {
        let language = tracks[start].language.as_deref().map(normalize_language);
        let end = tracks[start..]
            .iter()
            .position(|track| track.language.as_deref().map(normalize_language) != language)
            .map(|offset| start + offset)
            .unwrap_or(tracks.len());
        let positions: Vec<_> = (start..end)
            .filter(|index| tracks[*index].kind != SubtitleKind::Unknown)
            .collect();
        let mut known: Vec<_> = positions
            .iter()
            .map(|index| tracks[*index].clone())
            .collect();
        known.sort_by_key(|track| track.kind == SubtitleKind::Automatic);
        for (position, track) in positions.into_iter().zip(known) {
            tracks[position] = track;
        }
        start = end;
    }
}

/// Decode supported sidecars without lossy replacement of the original words.
pub fn read_subtitle_text(path: &Path) -> Result<String> {
    use std::io::Read;
    const MAX_BYTES: usize = 32 * 1024 * 1024;
    let file = std::fs::File::open(path)
        .with_context(|| format!("Cannot read subtitle file {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_BYTES,
        "Subtitle file exceeds 32 MB; choose a smaller SRT or VTT file"
    );
    decode_text_with_bom(&bytes)
}

/// Decodes UTF-8 or UTF-16 (byte order from the BOM) and strips the BOM;
/// without a BOM the text is UTF-8. Invalid or incomplete input is rejected, never replaced.
pub(crate) fn decode_text_with_bom(bytes: &[u8]) -> Result<String> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        ensure!(
            (bytes.len() - 2).is_multiple_of(2),
            "The file's UTF-16 encoding is incomplete; re-export as UTF-8"
        );
        let little = bytes[0] == 0xff;
        let words: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                if little {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            })
            .collect();
        return String::from_utf16(&words)
            .context("The file's character encoding is unreadable; re-export as UTF-8");
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    String::from_utf8(bytes.to_vec())
        .context("The file's character encoding is unreadable; re-export as UTF-8")
}

/// All exact-name and language-qualified sidecars, retaining both SRT and VTT.
pub fn sidecar_tracks(video: &Path) -> Vec<SubtitleTrack> {
    let Some(parent) = video.parent() else {
        return Vec::new();
    };
    let Some(stem) = video.file_stem().and_then(|value| value.to_str()) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            if !path.is_file() || !is_subtitle_file(path) {
                return false;
            }
            path.file_stem()
                .and_then(|value| value.to_str())
                .is_some_and(|candidate| {
                    candidate == stem || candidate.starts_with(&format!("{stem}."))
                })
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .enumerate()
        .map(|(index, path)| {
            let language = path
                .file_stem()
                .and_then(|value| value.to_str())
                .and_then(|candidate| candidate.strip_prefix(&format!("{stem}.")))
                .filter(|candidate| {
                    candidate
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
                })
                .map(str::to_owned);
            let mut track = file_track(path, language);
            track.source_order = index;
            track
        })
        .collect()
}

pub fn is_subtitle_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "srt" | "vtt"))
}

pub fn file_track(path: PathBuf, language: Option<String>) -> SubtitleTrack {
    SubtitleTrack {
        language,
        kind: SubtitleKind::Unknown,
        path,
        source_order: usize::MAX,
    }
}

/// Parses SRT or VTT subtitles into transcript events.
/// - Timestamps: `HH:MM:SS,mmm` (SRT), `HH:MM:SS.mmm` (VTT) and `MM:SS.mmm`
/// - Inline VTT tags (`<c>...</c>` and others) and common HTML entities are removed
/// - Lines of one cue are joined; rolling captions (common in automatic captions) are de-duplicated
pub fn parse_subtitle(content: &str) -> Vec<TranscriptEvent> {
    let mut out: Vec<TranscriptEvent> = vec![];
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some((start, end)) = parse_cue_header(line) {
            i += 1;
            // Untrusted input: drop cues with non-finite times,
            // which would break sorting and binary search later.
            if !start.is_finite() || !end.is_finite() || end <= start {
                tracing::warn!(line, "Skipped subtitle with invalid timestamps");
                continue;
            }
            let mut text_parts: Vec<String> = vec![];
            while i < lines.len() && !lines[i].trim().is_empty() {
                let cleaned = clean_cue_text(lines[i]);
                if !cleaned.is_empty() {
                    text_parts.push(cleaned);
                }
                i += 1;
            }
            let text = text_parts.join(" ");
            // Rolling captions: keep the first of adjacent identical cues and extend it.
            if !text.is_empty() {
                match out.last_mut() {
                    Some(prev) if prev.text == text && start <= prev.end => {
                        prev.end = end.max(prev.end)
                    }
                    _ => out.push(TranscriptEvent {
                        start,
                        end,
                        text,
                        raw: None,
                    }),
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

fn parse_cue_header(line: &str) -> Option<(f64, f64)> {
    let (a, b) = line.split_once("-->")?;
    // A VTT cue header may carry settings such as "line:0".
    let end = b.split_whitespace().next()?;
    Some((
        crate::timeline::parse_timestamp(a)?,
        crate::timeline::parse_timestamp(end)?,
    ))
}

/// Removes HTML/VTT tags and decodes common entities.
fn clean_cue_text(line: &str) -> String {
    let mut s = String::with_capacity(line.len());
    let mut chars = line.trim().chars();
    while let Some(c) = chars.next() {
        if c == '<'
            && let Some(end) = chars.as_str().find(['<', '>'])
            && chars.as_str().as_bytes()[end] == b'>'
            && is_cue_tag(&chars.as_str()[..end])
        {
            chars = chars.as_str()[end + 1..].chars();
        } else {
            s.push(c);
        }
    }
    // Replace &amp; last; otherwise "&amp;lt;" would become "&lt;" and then "<".
    for (from, to) in [
        ("&nbsp;", " "),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&amp;", "&"),
    ] {
        s = s.replace(from, to);
    }
    s.trim().to_string()
}

fn is_cue_tag(tag: &str) -> bool {
    if tag.contains(':')
        && tag.contains('.')
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b':' || byte == b'.')
        && crate::timeline::parse_timestamp(tag).is_some()
    {
        return true;
    }
    let tag = tag.strip_prefix('/').unwrap_or(tag);
    let end = tag
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .unwrap_or(tag.len());
    let (name, suffix) = tag.split_at(end);
    name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && (suffix.is_empty()
            || suffix == "/"
            || suffix.starts_with(|c: char| c.is_ascii_whitespace())
            || (name == "c" && suffix.starts_with('.')))
}

/// Same-name sidecars, preferring the requested ASR language when set.
pub fn sidecar_subtitle(video: &Path, preferred_language: &str) -> Option<PathBuf> {
    let mut tracks = sidecar_tracks(video);
    sort_tracks(&mut tracks, preferred_language, None);
    tracks.into_iter().next().map(|track| track.path)
}

pub fn validate_events(events: &[TranscriptEvent]) -> Result<()> {
    anyhow::ensure!(
        events.iter().all(|e| e.start.is_finite()
            && e.end.is_finite()
            && e.start >= 0.0
            && e.end >= e.start),
        "Invalid subtitle timestamps"
    );
    anyhow::ensure!(
        events.iter().any(|e| !e.text.trim().is_empty()),
        "Selected subtitles contain no readable text"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(language: &str, kind: SubtitleKind) -> SubtitleTrack {
        SubtitleTrack {
            language: Some(language.into()),
            kind,
            source_order: 0,
            path: format!("{language}.srt").into(),
        }
    }

    #[test]
    fn missed_preferences_fall_through_without_hiding_languages() {
        let mut tracks = vec![
            track("fr", SubtitleKind::Automatic),
            track("ja", SubtitleKind::Manual),
            track("fr", SubtitleKind::Manual),
            track("de", SubtitleKind::Manual),
        ];
        sort_tracks(&mut tracks, "zh", Some("ja"));
        assert_eq!(
            tracks
                .iter()
                .map(|track| track.language.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["ja", "fr", "fr", "de"]
        );
        assert_eq!(tracks[1].kind, SubtitleKind::Manual);
        assert_eq!(tracks[2].kind, SubtitleKind::Automatic);
        sort_tracks(&mut tracks, "xx", None);
        assert_eq!(tracks.len(), 4);
    }

    #[test]
    fn explicit_language_then_original_then_discovery_order() {
        let mut tracks = vec![
            track("ru", SubtitleKind::Manual),
            track("fr", SubtitleKind::Manual),
            track("de", SubtitleKind::Manual),
            track("en", SubtitleKind::Manual),
            track("ja", SubtitleKind::Manual),
        ];
        sort_tracks(&mut tracks, "ja", Some("de"));
        assert_eq!(
            tracks
                .iter()
                .map(|track| track.language.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["ja", "de", "ru", "fr", "en"]
        );
    }

    #[test]
    fn language_matching_ignores_case_and_subtags_without_aliasing_tags() {
        for (preferred, expected) in [("EN-us", "en"), ("eNg", "eng"), ("FOO-latn", "foo-Latn")] {
            let mut tracks = vec![
                track("eng", SubtitleKind::Manual),
                track("foo-Cyrl", SubtitleKind::Manual),
                track("en", SubtitleKind::Manual),
                track("foo-Latn", SubtitleKind::Manual),
            ];
            sort_tracks(&mut tracks, preferred, None);
            assert_eq!(tracks[0].language.as_deref(), Some(expected), "{preferred}");
        }
    }

    #[test]
    fn script_preference_preserves_unknown_caption_kind() {
        let mut tracks = vec![
            track("zh", SubtitleKind::Automatic),
            track("zh-Hans", SubtitleKind::Unknown),
            track("ja", SubtitleKind::Manual),
        ];
        for (index, track) in tracks.iter_mut().enumerate() {
            track.source_order = index;
        }
        sort_tracks(&mut tracks, "zh-Hans", Some("ja"));
        assert_eq!(tracks[0].language.as_deref(), Some("zh-Hans"));
        assert_eq!(tracks[0].kind, SubtitleKind::Unknown);
        assert_eq!(tracks[1].kind, SubtitleKind::Automatic);
    }

    #[test]
    fn generic_language_preference_does_not_promote_unspecified_script() {
        let mut tracks = vec![
            track("zh-Hans", SubtitleKind::Unknown),
            track("zh", SubtitleKind::Automatic),
            track("ja", SubtitleKind::Manual),
        ];
        for (index, track) in tracks.iter_mut().enumerate() {
            track.source_order = index;
        }
        sort_tracks(&mut tracks, "zh", None);
        assert_eq!(tracks[0].language.as_deref(), Some("zh-Hans"));
        assert_eq!(tracks[1].language.as_deref(), Some("zh"));
    }

    #[test]
    fn sidecars_include_every_language_and_both_formats_without_unrelated_files() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("Lecture.mp4");
        for name in [
            "Lecture.fr.srt",
            "Lecture.JA.VTT",
            "Lecture.srt",
            "Other.srt",
        ] {
            std::fs::write(
                dir.path().join(name),
                "1\n00:00:00,000 --> 00:00:01,000\nBonjour\n",
            )
            .unwrap();
        }
        let tracks = sidecar_tracks(&video);
        assert_eq!(tracks.len(), 3);
        assert!(
            tracks
                .iter()
                .any(|track| track.language.as_deref() == Some("fr"))
        );
        assert!(
            tracks
                .iter()
                .any(|track| track.language.as_deref() == Some("JA"))
        );
    }

    #[test]
    fn numeric_region_sidecars_are_recognized_and_preferred() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("Lecture.mp4");
        let preferred = dir.path().join("Lecture.es-419.srt");
        std::fs::write(&preferred, "Latin American Spanish").unwrap();
        std::fs::write(dir.path().join("Lecture.es-ES.srt"), "Spanish").unwrap();

        assert_eq!(sidecar_subtitle(&video, "es-419"), Some(preferred));
        assert!(
            sidecar_tracks(&video)
                .iter()
                .any(|track| track.language.as_deref() == Some("es-419"))
        );
    }

    #[test]
    fn utf16_sidecars_preserve_words_and_timing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("arbitrary-name.srt");
        let text = "1\n00:00:01,125 --> 00:00:02,375\nBonjour 世界\n";
        let bytes: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        std::fs::write(&file, bytes).unwrap();
        let events = parse_subtitle(&read_subtitle_text(&file).unwrap());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].start, 1.125);
        assert_eq!(events[0].end, 2.375);
        assert_eq!(events[0].text, "Bonjour 世界");
        std::fs::write(&file, [0xff, 0xff, 0xfe]).unwrap();
        assert!(read_subtitle_text(&file).is_err());
    }

    #[test]
    fn repeated_words_after_a_pause_remain_separate() {
        let events = parse_subtitle(
            "1\n00:00:01,000 --> 00:00:02,000\nhello\n\n2\n00:00:10,000 --> 00:00:11,000\nhello\n",
        );
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].end, 2.0);
        assert!(parse_subtitle("00:00:02,000 --> 00:00:01,000\nbackwards\n").is_empty());
        assert!(crate::timeline::parse_timestamp("00:00:01.bad").is_none());
        assert!(crate::timeline::parse_timestamp("00:99:01.000").is_none());
    }

    #[test]
    fn parses_srt_with_multiline_cues() {
        let srt = "\
1
00:00:01,000 --> 00:00:04,000
大家好，今天我们讲
编译原理

2
00:00:04,500 --> 00:00:07,250
这是第二段
";
        let ev = parse_subtitle(srt);
        assert_eq!(ev.len(), 2);
        assert!((ev[0].start - 1.0).abs() < 1e-3);
        assert!((ev[0].end - 4.0).abs() < 1e-3);
        assert_eq!(ev[0].text, "大家好，今天我们讲 编译原理");
        assert!((ev[1].start - 4.5).abs() < 1e-3);
    }

    #[test]
    fn parses_vtt_and_strips_tags() {
        let vtt = "\
WEBVTT

NOTE 这是注释

00:01:01.000 --> 00:01:03.000 align:start
<c>hello</c> world&nbsp;!

00:03.500 --> 00:05.000
second cue
";
        let ev = parse_subtitle(vtt);
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].text, "hello world !");
        assert!((ev[0].start - 61.0).abs() < 1e-3);
        assert_eq!(ev[1].text, "second cue");
    }

    #[test]
    fn cue_text_preserves_literal_angle_brackets() {
        for (input, expected) in [
            ("if x < 3, return true.", "if x < 3, return true."),
            ("if x < 3 and y > 2.", "if x < 3 and y > 2."),
            ("a > b", "a > b"),
            ("an unfinished <b", "an unfinished <b"),
            ("2 < 3 <b>bold</b> > 1", "2 < 3 bold > 1"),
        ] {
            assert_eq!(clean_cue_text(input), expected, "{input}");
        }
    }

    #[test]
    fn cue_text_strips_html_and_vtt_markup_and_keeps_entity_decoding() {
        for (input, expected) in [
            ("<c>one</c><c.yellow>two</c>", "onetwo"),
            ("<i>one</i><b>two</b><u>three</u>", "onetwothree"),
            ("<v Speaker>one</v><lang en>two</lang>", "onetwo"),
            ("<ruby>one<rt>two</rt></ruby>", "onetwo"),
            ("<00:00:01.000>one<00:02.000>two", "onetwo"),
            ("<font color=\"red\">one</font><br/>two", "onetwo"),
            ("a <b> c", "a  c"),
            ("&lt;b&gt; &amp;lt; &nbsp;&quot;", "<b> &lt;  \""),
        ] {
            assert_eq!(clean_cue_text(input), expected, "{input}");
        }
    }

    #[test]
    fn dedupes_rolling_captions() {
        // Rolling duplicates as in automatic captions: adjacent cues with the same text
        let srt = "\
1
00:00:01,000 --> 00:00:02,000
机器学习是

2
00:00:02,000 --> 00:00:03,000
机器学习是

3
00:00:03,000 --> 00:00:04,000
一门人工智能分支
";
        let ev = parse_subtitle(srt);
        assert_eq!(ev.len(), 2, "adjacent duplicates merge");
        assert!(
            (ev[0].end - 3.0).abs() < 1e-3,
            "a duplicate extends the cue"
        );
        assert_eq!(ev[1].text, "一门人工智能分支");
    }

    #[test]
    fn sidecar_lookup() {
        let d = std::env::temp_dir().join(format!("notepipe-side-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let video = d.join("lecture.mp4");
        std::fs::write(&video, b"v").unwrap();
        assert!(sidecar_subtitle(&video, "").is_none());
        std::fs::write(
            d.join("lecture.srt"),
            b"1\n00:00:01,000 --> 00:00:02,000\nx\n",
        )
        .unwrap();
        assert_eq!(sidecar_subtitle(&video, "").unwrap(), d.join("lecture.srt"));
        std::fs::write(
            d.join("lecture.zh.VTT"),
            b"WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nx\n",
        )
        .unwrap();
        assert_eq!(sidecar_subtitle(&video, "").unwrap(), d.join("lecture.srt"));
        assert_eq!(
            sidecar_subtitle(&video, "zh").unwrap(),
            d.join("lecture.zh.VTT")
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
