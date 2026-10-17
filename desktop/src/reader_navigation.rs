//! Reader identities, source links, search, proportional anchors and display timestamps.
use std::{
    collections::BTreeMap,
    ops::Range,
    path::{Path, PathBuf},
};

/// Content coordinates keyed by document block (or frame), regardless of layout nesting.
///
/// The note list records window coordinates: only differences within one item
/// and item heights are read (`search_within`, `item_height`), so the scroll
/// base cancels out, and the list state cannot be queried while its items
/// prepaint. The screenshot grid records offsets inside its scrolled content.
#[derive(Default)]
pub struct ReadingLayout {
    blocks: BTreeMap<usize, (f32, f32)>,
    search_lines: BTreeMap<(usize, usize), (f32, f32)>,
}

impl ReadingLayout {
    pub fn record(&mut self, index: usize, top: f32, height: f32) {
        self.blocks.insert(index, (top, height));
    }

    pub fn record_search_line(&mut self, block: usize, byte: usize, top: f32, height: f32) {
        self.search_lines.insert((block, byte), (top, height));
    }

    /// Forget measurements after the layout or the measured content changes.
    pub fn clear(&mut self) {
        self.blocks.clear();
        self.search_lines.clear();
    }

    pub fn top_item(&self, offset: f32) -> Option<(usize, f32, f32)> {
        let top = -offset;
        let covering = self
            .blocks
            .iter()
            .filter(|(_, (y, height))| *y <= top && *y + *height > top)
            .max_by(|a, b| a.1.0.total_cmp(&b.1.0).then_with(|| b.0.cmp(a.0)));
        let next = || {
            self.blocks
                .iter()
                .filter(|(_, (y, _))| *y > top)
                .min_by(|a, b| a.1.0.total_cmp(&b.1.0).then_with(|| a.0.cmp(b.0)))
        };
        let last = || self.blocks.iter().max_by(|a, b| a.1.0.total_cmp(&b.1.0));
        let (&index, &(y, height)) = covering.or_else(next).or_else(last)?;
        Some((index, (y - top).min(0.), height))
    }

    pub fn restore(&self, index: usize, fraction: Option<f32>, within: f32) -> Option<f32> {
        let &(top, height) = self.blocks.get(&index)?;
        Some(-top + restore_within(fraction, within, height))
    }

    /// Height of a rendered item, without interpreting its position.
    pub fn item_height(&self, index: usize) -> Option<f32> {
        self.blocks.get(&index).map(|&(_, height)| height)
    }

    /// Offset of a found line inside its own item, leaving one line of context
    /// above it. Both ends were recorded against the same viewport base, so the
    /// base cancels out of the difference.
    pub fn search_within(&self, block: usize, byte: usize) -> Option<f32> {
        let &(line_top, line_height) = self.search_lines.get(&(block, byte))?;
        let &(block_top, _) = self.blocks.get(&block)?;
        Some((line_top - block_top - line_height).max(0.))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceTarget {
    Web(String),
    Local(PathBuf),
}

pub fn source_target(raw: &str, local: bool) -> Option<SourceTarget> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(url) = url::Url::parse(raw) {
        if url.scheme() == "file" {
            return url.to_file_path().ok().map(SourceTarget::Local);
        }
        if !local && matches!(url.scheme(), "https" | "http") && url.host_str().is_some() {
            return Some(SourceTarget::Web(url.into()));
        }
    }
    let path = PathBuf::from(raw);
    path.is_absolute().then_some(SourceTarget::Local(path))
}

/// Only these source platforms promise a seek URL; arbitrary sites and local players do not.
pub fn seek_url(target: &SourceTarget, seconds: f64) -> Option<String> {
    if !seconds.is_finite() || seconds < 0. {
        return None;
    }
    let SourceTarget::Web(raw) = target else {
        return None;
    };
    let mut url = url::Url::parse(raw).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let supported = ["youtube.com", "youtu.be", "bilibili.com"]
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")));
    if !supported {
        return None;
    }
    let pairs: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| key != "t" && key != "start")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    url.query_pairs_mut()
        .extend_pairs(pairs)
        .append_pair("t", &(seconds.floor() as u64).to_string());
    url.set_fragment(None);
    Some(url.into())
}

/// Prefer the registered new location even while the old recovery copy still exists.
pub fn relocated_source(path: &Path, locations: &[(PathBuf, Vec<PathBuf>)]) -> PathBuf {
    let mut candidates = Vec::new();
    for (current, previous) in locations {
        for old in previous {
            if let Ok(relative) = path.strip_prefix(old) {
                candidates.push((old.components().count(), current.join(relative)));
            } else if let (Ok(path), Ok(old)) = (path.canonicalize(), old.canonicalize())
                && let Ok(relative) = path.strip_prefix(&old)
            {
                candidates.push((old.components().count(), current.join(relative)));
            }
        }
    }
    candidates
        .into_iter()
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, path)| path)
        .unwrap_or_else(|| path.to_path_buf())
}

pub fn within_fraction(within: f32, item_height: f32) -> f32 {
    if !within.is_finite() || !item_height.is_finite() || item_height <= 0. {
        return 0.;
    }
    (-within / item_height).clamp(0., 0.999)
}

pub fn restore_within(fraction: Option<f32>, within: f32, item_height: f32) -> f32 {
    if let Some(fraction) = fraction.filter(|value| value.is_finite()) {
        -(fraction.clamp(0., 0.999) * item_height.max(0.))
    } else if within.is_finite() {
        within.min(0.).max(-item_height.max(0.))
    } else {
        0.
    }
}

pub fn nearest_time(
    times: impl IntoIterator<Item = (usize, Option<f64>)>,
    target: f64,
) -> Option<(usize, bool)> {
    if !target.is_finite() {
        return None;
    }
    times
        .into_iter()
        .filter_map(|(index, seconds)| {
            seconds
                .filter(|s| s.is_finite())
                .map(|seconds| (index, (seconds - target).abs()))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
        .map(|(index, distance)| (index, distance < 0.05))
}

/// Map case-folded matches back to valid UTF-8 ranges, including expanding lowercase chars.
pub fn text_matches(text: &str, query: &str) -> Vec<Range<usize>> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let mut lower = String::new();
    let mut mapping = Vec::new();
    for (start, character) in text.char_indices() {
        for lower_char in character.to_lowercase() {
            let count = lower_char.len_utf8();
            lower.push(lower_char);
            mapping.extend(std::iter::repeat_n(
                (start, start + character.len_utf8()),
                count,
            ));
        }
    }
    lower
        .match_indices(&query)
        .filter_map(|(start, value)| {
            Some(mapping.get(start)?.0..mapping.get(start + value.len() - 1)?.1)
        })
        .collect()
}

/// A contents list describes topics. Frame boundaries remain in the article,
/// but a timestamp alone does not become a chapter in the reading rail.
pub fn content_outline(
    headings: &[(usize, String, Option<f64>)],
    topics: &[(f64, String)],
) -> Vec<(usize, String, Option<f64>)> {
    let mut named = BTreeMap::new();
    for (index, label, seconds) in headings {
        if !label.trim().is_empty()
            && !seconds.is_some_and(|seconds| course2md::render::fmt_ts(seconds) == *label)
        {
            named.insert(*index, (label.clone(), *seconds));
        }
    }
    for (seconds, title) in topics.iter().filter(|(_, title)| !title.trim().is_empty()) {
        if let Some((index, _)) = nearest_time(
            headings
                .iter()
                .map(|(index, _, seconds)| (*index, *seconds)),
            *seconds,
        ) {
            // The entry opens this section, whose displayed time may differ
            // slightly from the summary's proposed topic boundary.
            let displayed_time = headings
                .iter()
                .find(|(heading, _, _)| *heading == index)
                .and_then(|(_, _, seconds)| *seconds);
            named.insert(index, (title.clone(), displayed_time));
        }
    }
    if named.len() < 2 {
        return Vec::new();
    }
    named
        .into_iter()
        .map(|(index, (label, seconds))| (index, label, seconds))
        .collect()
}

/// Display an immutable UTC instant using the system timezone's rules for that
/// date, rather than applying today's offset to every historical timestamp.
/// Calendar date in local time, without slicing a formatted timestamp.
pub fn date_local(milliseconds: u64) -> String {
    use chrono::Datelike;
    let instant = i64::try_from(milliseconds)
        .ok()
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis);
    let Some(instant) = instant.filter(|instant| (1..=9999).contains(&instant.year())) else {
        return "-".into();
    };
    instant
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d")
        .to_string()
}

pub fn timestamp_local(milliseconds: u64) -> String {
    timestamp_in_timezone(milliseconds, &chrono::Local)
}

fn timestamp_in_timezone<Tz: chrono::TimeZone>(milliseconds: u64, timezone: &Tz) -> String {
    use chrono::Datelike;

    let display = || {
        let instant = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
            i64::try_from(milliseconds).ok()?,
        )?;
        // Keep ordinary display years bounded before adding a timezone offset;
        // malformed persisted values must not overflow or invent a nearby date.
        if !(1..=9999).contains(&instant.year()) {
            return None;
        }
        let local = instant.with_timezone(timezone).naive_local();
        if !(1..=9999).contains(&local.year()) {
            return None;
        }
        Some(local.format("%Y-%m-%d %H:%M").to_string())
    };
    display().unwrap_or_else(|| course2md::tr!("时间未知").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contents_use_real_topics_instead_of_every_frame_timestamp() {
        let headings = vec![
            (0, "摘要".into(), None),
            (2, "00:06".into(), Some(6.)),
            (5, "00:16".into(), Some(16.)),
            (8, "00:27".into(), Some(27.)),
            (11, "00:40".into(), Some(40.)),
        ];
        assert!(content_outline(&headings, &[]).is_empty());
        assert_eq!(
            content_outline(
                &headings,
                &[(6., "问题由来".into()), (39., "讨论与回应".into())]
            ),
            vec![
                (0, "摘要".into(), None),
                (2, "问题由来".into(), Some(6.)),
                (11, "讨论与回应".into(), Some(40.)),
            ]
        );
        let authored = vec![(0, "第一章".into(), None), (5, "第二章".into(), None)];
        assert_eq!(content_outline(&authored, &[]), authored);
    }

    #[test]
    fn nested_summary_blocks_and_wrapped_frames_use_their_own_coordinates() {
        let mut body = ReadingLayout::default();
        body.record(1, 20., 40.); // Summary card paragraphs are nested, not scroll children.
        body.record(2, 70., 50.);
        body.record(4, 150., 30.);
        body.record(5, 200., 400.);
        assert_eq!(body.top_item(-300.), Some((5, -100., 400.)));
        assert_eq!(body.restore(2, Some(0.5), 0.), Some(-95.));
        assert_eq!(body.restore(5, Some(0.25), 0.), Some(-300.));
        let mut grid = ReadingLayout::default();
        grid.record(0, 0., 200.);
        grid.record(1, 0., 200.);
        grid.record(2, 216., 200.);
        grid.record(3, 216., 200.);
        assert_eq!(grid.top_item(-240.), Some((2, -24., 200.)));
        assert_eq!(grid.restore(3, None, 0.), Some(-216.));
        grid.record(2, 432., 200.); // The same frame moves after a window resize.
        assert_eq!(grid.restore(2, Some(0.12), 0.), Some(-456.));
    }

    #[test]
    fn search_within_is_relative_to_the_owning_item() {
        let mut layout = ReadingLayout::default();
        layout.record(1, 500., 320.);
        layout.record_search_line(1, 130, 584., 32.);
        assert_eq!(layout.search_within(1, 130), Some(52.));
        layout.record_search_line(1, 0, 508., 32.);
        assert_eq!(layout.search_within(1, 0), Some(0.));
        assert_eq!(layout.search_within(1, 42), None);
        assert_eq!(layout.search_within(2, 130), None);
        assert_eq!(layout.item_height(1), Some(320.));
        assert_eq!(layout.item_height(2), None);
    }
    #[test]
    fn source_links_do_not_invent_local_or_unknown_seek_support() {
        assert!(source_target("", false).is_none());
        assert!(source_target("javascript:alert(1)", false).is_none());
        let directory = tempfile::tempdir().unwrap();
        let video = directory.path().join("video.mp4");
        let local = source_target(video.to_str().unwrap(), true).unwrap();
        assert!(seek_url(&local, 12.).is_none());
        let unknown = source_target("https://video.example/watch", false).unwrap();
        assert!(seek_url(&unknown, 12.).is_none());
        let web = source_target("https://www.bilibili.com/video/BV123?p=2&t=4#old", false).unwrap();
        let url = seek_url(&web, 90.).unwrap();
        assert!(url.contains("p=2") && url.contains("t=90") && !url.contains("old"));
    }
    #[test]
    fn move_uses_new_root_even_when_old_copy_is_kept() {
        let locations = vec![(PathBuf::from("/new"), vec![PathBuf::from("/old")])];
        assert_eq!(
            relocated_source(Path::new("/old/video.mp4"), &locations),
            PathBuf::from("/new/video.mp4")
        );
        assert_eq!(
            relocated_source(Path::new("/outside/video.mp4"), &locations),
            PathBuf::from("/outside/video.mp4")
        );
    }
    #[test]
    fn anchor_restores_inside_paragraph_after_font_reflow() {
        let fraction = within_fraction(-75., 300.);
        assert_eq!(fraction, 0.25);
        assert_eq!(restore_within(Some(fraction), -75., 600.), -150.);
        assert_eq!(restore_within(Some(f32::NAN), -75., 600.), -75.);
        assert_eq!(within_fraction(f32::NAN, 10.), 0.);
    }
    #[test]
    fn nearest_time_uses_known_data_and_reports_approximate_matches() {
        assert_eq!(
            nearest_time([(0, None), (1, Some(10.)), (2, Some(20.))], 19.),
            Some((2, false))
        );
        assert_eq!(
            nearest_time([(0, None), (1, Some(0.))], 0.),
            Some((1, true))
        );
        assert_eq!(nearest_time([(0, None)], 0.), None);
    }
    #[test]
    fn search_preserves_utf8_and_finds_multiple_real_matches() {
        let value = "中文 AI 与 ai，İstanbul";
        let ranges = text_matches(value, "ai");
        assert_eq!(
            ranges.iter().map(|r| &value[r.clone()]).collect::<Vec<_>>(),
            ["AI", "ai"]
        );
        let range = text_matches(value, "i̇").remove(0);
        assert_eq!(&value[range], "İ");
        assert!(text_matches(value, "").is_empty());
    }
    #[test]
    fn local_display_rolls_dates_across_year_and_leap_day_boundaries() {
        let singapore = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
        let pacific = chrono::FixedOffset::west_opt(8 * 3600).unwrap();
        let milliseconds = |value: &str| {
            chrono::DateTime::parse_from_rfc3339(value)
                .unwrap()
                .timestamp_millis() as u64
        };
        assert_eq!(
            timestamp_in_timezone(milliseconds("2023-12-31T20:30:00Z"), &singapore),
            "2024-01-01 04:30"
        );
        assert_eq!(
            timestamp_in_timezone(milliseconds("2024-02-28T20:30:00Z"), &singapore),
            "2024-02-29 04:30"
        );
        assert_eq!(timestamp_in_timezone(0, &pacific), "1969-12-31 16:00");
        assert_eq!(
            timestamp_in_timezone(milliseconds("2024-03-01T00:15:00Z"), &pacific),
            "2024-02-29 16:15"
        );
    }
    #[test]
    fn local_display_preserves_non_hour_offsets_and_rejects_invalid_dates() {
        let nepal = chrono::FixedOffset::east_opt(5 * 3600 + 45 * 60).unwrap();
        assert_eq!(timestamp_in_timezone(0, &nepal), "1970-01-01 05:45");
        assert_eq!(timestamp_in_timezone(u64::MAX, &nepal), "时间未知");
        assert_eq!(timestamp_in_timezone(253402300799999, &nepal), "时间未知");
    }
}
