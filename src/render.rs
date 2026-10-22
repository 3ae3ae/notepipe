//! Markdown notes with safely quoted front matter.
use crate::{source::MediaMeta, timeline::Section};
use anyhow::Result;
use std::{fmt::Write as _, path::Path};

/// mm:ss or h:mm:ss
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
    let source = meta.source.as_str();
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

/// File links use Media Fragments; HTTP seeking is defined by the source adapter.
fn ts_url_from(source: &str, template: Option<&str>, sec: f64) -> String {
    let Ok(mut url) = url::Url::parse(source) else {
        return source.to_string();
    };
    let seconds = (sec.max(0.0).floor() as u64).to_string();
    if url.scheme() == "file" {
        url.set_fragment(Some(&format!("t={seconds}")));
        return url.into();
    }
    template.map_or_else(
        || source.to_owned(),
        |template| template.replace("{t}", &seconds),
    )
}

/// Minimal escaping for inline metadata such as the title: remove line breaks, which would end
/// headings and list items, and a leading `#`, which would create a heading. Other Markdown is kept.
fn md_inline(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
        .trim_start_matches('#')
        .to_string()
}

pub fn render_markdown(
    meta: &MediaMeta,
    sections: &[Section],
    top: &[crate::timeline::StageText],
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
    for output in top {
        md.push_str(&output.text);
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
                ts_url_from(&source, meta.seek_url.as_deref(), section.t)
            )
            .unwrap();
        }
        if !section.image.is_empty() {
            writeln!(md, "![{}]({})\n", fmt_ts(section.t), section.image).unwrap();
        }
        for event in &section.speech {
            writeln!(md, "{}\n", event.text).unwrap();
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
    fn timestamps_use_utc_and_http_seeking_requires_an_adapter_template() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc_timestamp(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(
            ts_url_from("https://example.test/watch?t=3&x=1#old", None, 61.9),
            "https://example.test/watch?t=3&x=1#old"
        );
        assert_eq!(
            ts_url_from("https://EXAMPLE.test:443/%7epath?x=a%20b#old", None, 61.9),
            "https://EXAMPLE.test:443/%7epath?x=a%20b#old"
        );
        assert_eq!(
            ts_url_from(
                "https://example.test/watch",
                Some("https://example.test/play?seconds={t}&end={t}#keep"),
                61.9
            ),
            "https://example.test/play?seconds=61&end=61#keep"
        );
        assert_eq!(
            ts_url_from("file:///tmp/a%20lesson.mp4#t=2", None, 4.),
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
            seek_url: None,
        };
        let summary = "## Custom heading\n\nExact **Markdown**.";
        let md = render_markdown(
            &meta,
            &[],
            &[crate::timeline::StageText {
                stage: "summary".into(),
                text: summary.into(),
            }],
        )
        .unwrap();
        assert!(md.starts_with(
            "---\ntitle: \"Title\\nsource: false\"\nsource: \"https://example.test/a\""
        ));
        assert!(md.contains("duration: \"1:02:05\""));
        assert!(md.contains(summary));
        assert!(!md.contains("Generated by") && !md.contains("Author:"));
    }
}
