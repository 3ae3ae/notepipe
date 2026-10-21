//! Markdown notes with safely quoted front matter.
use crate::{source::MediaMeta, timeline::Section};
use anyhow::Result;
use std::{fmt::Write as _, path::Path};

/// mm:ss 或 h:mm:ss
pub fn fmt_ts(sec: f64) -> String {
    let s = sec.max(0.0).floor() as u64;
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

fn source_url(meta: &MediaMeta) -> String {
    let source = meta.source.trim();
    if Path::new(source).is_absolute() {
        let path = Path::new(source)
            .canonicalize()
            .unwrap_or_else(|_| source.into());
        if let Ok(url) = url::Url::from_file_path(path) {
            return url.into();
        }
    }
    source.to_string()
}

/// Existing timestamps and fragments must not mask the requested seek time.
/// `source` is resolved by the caller (canonicalize runs once per render,
/// not once per section).
fn ts_url_from(source: &str, sec: f64) -> String {
    let Ok(mut url) = url::Url::parse(source) else {
        return source.to_string();
    };
    let seconds = (sec.max(0.0).floor() as u64).to_string();
    if url.scheme() == "file" {
        url.set_fragment(Some(&format!("t={seconds}")));
    } else {
        let pairs: Vec<_> = url
            .query_pairs()
            .filter(|(key, _)| key != "t")
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        url.set_query(None);
        url.query_pairs_mut()
            .extend_pairs(pairs)
            .append_pair("t", &seconds);
        url.set_fragment(None);
    }
    url.into()
}

/// md 侧的最小转义策略：标题/作者等内联元信息只 strip 换行（换行会截断 ATX
/// 标题与列表行）与行首 `#`（防伪造标题）。其余 Markdown 字符保持原样。
fn md_inline(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
        .trim_start_matches('#')
        .to_string()
}

pub fn render_markdown(
    meta: &MediaMeta,
    sections: &[Section],
    summary: Option<&str>,
) -> Result<String> {
    let mut md = String::from("---\n");
    for (key, value) in [("title", &meta.title), ("source", &meta.source)] {
        writeln!(md, "{key}: {}", serde_json::to_string(value)?).unwrap();
    }
    if !meta.author.trim().is_empty() {
        writeln!(md, "author: {}", serde_json::to_string(&meta.author)?).unwrap();
    }
    if meta.duration > 0. {
        let seconds = meta.duration.floor() as u64;
        writeln!(
            md,
            "duration: \"{}:{:02}:{:02}\"",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
        .unwrap();
    }
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    writeln!(md, "created: \"{}\"", utc_timestamp(seconds)).unwrap();
    writeln!(
        md,
        "generator: \"notepipe {}\"\n---\n",
        env!("CARGO_PKG_VERSION")
    )
    .unwrap();
    writeln!(md, "# {}\n", md_inline(&meta.title)).unwrap();
    if let Some(summary) = summary {
        md.push_str(summary);
        md.push_str("\n\n");
    }
    let source = source_url(meta);
    for section in sections {
        if source.is_empty() {
            writeln!(md, "## {}\n", fmt_ts(section.t)).unwrap();
        } else {
            writeln!(
                md,
                "## [{}]({})\n",
                fmt_ts(section.t),
                ts_url_from(&source, section.t)
            )
            .unwrap();
        }
        if !section.image.is_empty() {
            writeln!(md, "![{}]({})\n", fmt_ts(section.t), section.image).unwrap();
        }
        for event in &section.speech {
            writeln!(md, "{}\n", event.text).unwrap();
            if let Some(translation) = &event.translation {
                writeln!(md, "> {}\n", md_inline(translation)).unwrap();
            }
        }
    }
    Ok(md)
}

fn utc_timestamp(seconds: u64) -> String {
    let mut days = seconds / 86_400;
    let mut year = 1970;
    let leap = |year| year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    loop {
        let length = if leap(year) { 366 } else { 365 };
        if days < length {
            break;
        }
        days -= length;
        year += 1;
    }
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1;
    for length in months {
        if days < length {
            break;
        }
        days -= length;
        month += 1;
    }
    format!(
        "{year:04}-{month:02}-{:02}T{:02}:{:02}:{:02}Z",
        days + 1,
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamps_use_utc_and_links_replace_existing_seek_times() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc_timestamp(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(
            ts_url_from("https://example.test/watch?t=3&x=1#old", 61.9),
            "https://example.test/watch?x=1&t=61"
        );
        assert_eq!(
            ts_url_from("file:///tmp/a%20lesson.mp4#t=2", 4.),
            "file:///tmp/a%20lesson.mp4#t=4"
        );
    }
    #[test]
    fn metadata_is_quoted_and_summary_is_preserved_verbatim() {
        let meta = MediaMeta {
            title: "Title\nsource: false".into(),
            author: "A \"quote\"".into(),
            duration: 3725.,
            source: "https://example.test/a".into(),
        };
        let summary = "## Custom heading\n\nExact **Markdown**.";
        let md = render_markdown(&meta, &[], Some(summary)).unwrap();
        assert!(md.starts_with(
            "---\ntitle: \"Title\\nsource: false\"\nsource: \"https://example.test/a\""
        ));
        assert!(md.contains("duration: \"1:02:05\""));
        assert!(md.contains(summary));
        assert!(!md.contains("Generated by") && !md.contains("Author:"));
    }
}
