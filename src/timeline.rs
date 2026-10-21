//! Timeline events, screenshot alignment, and paragraph assembly.

use serde::{Deserialize, Serialize};
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FrameEvent {
    /// 帧对应的视频时间（秒）
    pub t: f64,
    /// 相对输出目录的图片路径，如 "frames/slide_0001.jpg"
    pub image: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscriptEvent {
    pub start: f64,
    pub end: f64,
    /// 展示文本（LLM 校对后的）；未校对时与 raw 相同
    pub text: String,
    /// ASR 原始文本（provenance；校对后保留）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub below: Vec<StageText>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StageText {
    pub stage: String,
    pub text: String,
}

/// 一张截图 + 展示期间的语音。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub t: f64,
    /// 本截图覆盖的结束时间（下一张截图起点；最后一段为媒体时长）
    #[serde(default)]
    pub end: f64,
    pub image: String,
    pub speech: Vec<TranscriptEvent>,
}

/// 段落组织：同一截图内相邻片段间隔超过此值则分段。
const PARAGRAPH_GAP_SECS: f64 = 3.5;
/// Paragraph limit: East Asian wide characters count double; other characters count once.
const MAX_PARAGRAPH_UNITS: usize = 840;

/// 宽容的时间戳解析（字幕 cue、旧导出标题、LLM 大纲时间共用，取三者超集语义）：
/// `HH:MM:SS` / `MM:SS` / 裸秒数，小数分隔符 `.` 或 `,`，容忍首尾空白、
/// `[...]` 包裹（旧导出标题）与秒后缀 `s`（LLM 常见的 "120s" 输出）。
/// 分/秒位 >= 60、负数或非有限数值一律拒绝（下游排序/二分不接受异常值）。
pub(crate) fn parse_timestamp(value: &str) -> Option<f64> {
    let value = value.trim().trim_matches(['[', ']']).trim();
    let value = value.strip_suffix('s').unwrap_or(value);
    let value = value.replace(',', ".");
    let parts: Vec<&str> = value.split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut result = 0.0;
    for (i, part) in parts.iter().enumerate() {
        let n: f64 = part.trim().parse().ok()?;
        if !n.is_finite() || n < 0.0 || (i > 0 && n >= 60.0) {
            return None;
        }
        result = result * 60.0 + n;
    }
    Some(result)
}

/// 合并算法：
/// 每条语音按中点归属「时间 ≤ 中点的最后一张截图」；首张之前的归首张。
/// 跨截图边界的语音只在边界附近找到句读/空格时才拆分（保持图文对应）；
/// 找不到自然断点则整段保留——宁可晚一页，也不按字符比例从词中间截断。
pub fn merge(
    frames: Vec<FrameEvent>,
    speech: Vec<TranscriptEvent>,
    media_end: f64,
) -> Vec<Section> {
    if frames.is_empty() {
        return vec![];
    }
    let mut sections: Vec<Section> = frames
        .into_iter()
        .map(|f| Section {
            t: f.t,
            end: 0.0,
            image: f.image,
            speech: vec![],
        })
        .collect();
    // 每段的结束时间 = 下一段起点；末段用媒体时长（未知时退化为自身起点）
    for i in 0..sections.len() {
        let next = sections.get(i + 1).map(|s| s.t).unwrap_or(f64::INFINITY);
        sections[i].end = next.min(media_end.max(sections[i].t));
    }
    let boundaries: Vec<f64> = sections.iter().map(|s| s.t).collect();
    for ev in speech {
        for piece in split_at_natural_boundaries(ev, &boundaries) {
            let mid = (piece.start + piece.end) / 2.0;
            let idx = match sections[..].binary_search_by(|s| s.t.total_cmp(&mid)) {
                Ok(i) => i,
                Err(0) => 0, // 首张截图之前
                Err(i) => i - 1,
            };
            sections[idx].speech.push(piece);
        }
    }
    sections
}

/// 仅在边界附近找到句读或空格时才拆分文字。
///
/// 时间与字符位置没有可靠的一一映射：找不到自然断点时，宁可让整段留在
/// 一张截图下，也不按比例从词中间截断；多边界事件只要有任一边界无法安全
/// 切分，就完整保留，避免产生半自然的混合结果。
fn split_at_natural_boundaries(event: TranscriptEvent, boundaries: &[f64]) -> Vec<TranscriptEvent> {
    let inner: Vec<f64> = boundaries
        .iter()
        .copied()
        .filter(|&b| b > event.start + 0.3 && b < event.end - 0.3)
        .collect();
    if inner.is_empty() {
        return vec![event];
    }

    let points = std::iter::once(event.start)
        .chain(inner)
        .chain(std::iter::once(event.end))
        .collect::<Vec<_>>();
    let chars: Vec<char> = event.text.chars().collect();
    let total = event.end - event.start;
    let mut char_pos = 0usize;
    let mut cut_positions = Vec::with_capacity(points.len() - 1);

    for (index, window) in points.windows(2).enumerate() {
        let end_char = if index + 2 == points.len() {
            chars.len()
        } else {
            let ideal = (chars.len() as f64 * ((window[1] - event.start) / total)).round() as usize;
            match snap_to_natural_break(&chars, ideal, char_pos + 1, chars.len()) {
                Some(p) => p,
                // 任一边界找不到自然断点：整段保留
                None => return vec![event],
            }
        };
        cut_positions.push(end_char);
        char_pos = end_char;
    }

    let mut start_char = 0usize;
    points
        .windows(2)
        .zip(cut_positions)
        .map(|(window, end_char)| {
            let text: String = chars[start_char..end_char].iter().collect();
            start_char = end_char;
            TranscriptEvent {
                start: window[0],
                end: window[1],
                text,
                raw: event.raw.clone(),
                below: Vec::new(),
            }
        })
        .collect()
}

/// 在理想切点附近（±WINDOW 字符）找最近的句读或空格，切点落在标点之后。
fn snap_to_natural_break(chars: &[char], ideal: usize, lo: usize, hi: usize) -> Option<usize> {
    const WINDOW: i32 = 6;
    const BREAKS: &str = "。！？；：，、,.!?;: ";
    for offset in 0..=WINDOW {
        for candidate in [ideal as i32 - offset, ideal as i32 + offset] {
            if candidate < lo as i32 || candidate > hi as i32 {
                continue;
            }
            let index = candidate as usize;
            if index >= 1 && index <= chars.len() && BREAKS.contains(chars[index - 1]) {
                return Some(index);
            }
        }
    }
    None
}

/// 将同一截图下连续的 ASR 片段组织为可阅读的段落。
///
/// 只作用于供渲染与可选 LLM 校对使用的 Section；调用方应在此之前把细粒度
/// ASR 事件写入 timeline.jsonl，保留原始时间线与可追溯性。
pub fn coalesce_sections(sections: &mut [Section]) {
    for section in sections {
        let mut paragraphs: Vec<TranscriptEvent> = Vec::new();
        let mut current: Option<TranscriptEvent> = None;

        for event in std::mem::take(&mut section.speech) {
            let text = event.text.trim();
            if text.is_empty() {
                continue;
            }

            let should_break = current.as_ref().is_some_and(|paragraph| {
                !continues_grapheme(&paragraph.text, text)
                    && (event.start - paragraph.end > PARAGRAPH_GAP_SECS
                        || paragraph_units(&paragraph.text)
                            + paragraph_units(text)
                            + usize::from(needs_space(&paragraph.text, text))
                            > MAX_PARAGRAPH_UNITS)
            });
            if should_break {
                paragraphs.push(current.take().expect("paragraph exists when breaking"));
            }

            match current.as_mut() {
                Some(paragraph) => {
                    append_text(&mut paragraph.text, text);
                    paragraph.end = event.end;
                }
                None => {
                    current = Some(TranscriptEvent {
                        start: event.start,
                        end: event.end,
                        text: text.to_string(),
                        raw: None,
                        below: Vec::new(),
                    });
                }
            }
        }
        if let Some(paragraph) = current {
            paragraphs.push(paragraph);
        }
        section.speech = paragraphs;
    }
}

/// Whether a transcript has any text worth publishing or reusing.
pub fn has_readable_events(events: &[TranscriptEvent]) -> bool {
    events.iter().any(|event| !event.text.trim().is_empty())
}

fn is_unspaced(c: char) -> bool {
    (c.is_alphabetic()
        && matches!(c as u32,
            0x3005..=0x3007 | 0x3040..=0x30ff | 0x31f0..=0x31ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff
                | 0xf900..=0xfaff | 0xff66..=0xff9f | 0x1aff0..=0x1afff
                | 0x1b000..=0x1b16f | 0x20000..=0x2fa1f | 0x30000..=0x3347f
        ))
        // Letters, marks and numerals in the remaining unspaced scripts, excluding punctuation.
        || matches!(c as u32,
            0x3099..=0x309a
                | 0x0e01..=0x0e3a | 0x0e40..=0x0e4e | 0x0e50..=0x0e59
                | 0x0e81..=0x0ec6 | 0x0ec8..=0x0ece | 0x0ed0..=0x0edf
                | 0x1780..=0x17d3 | 0x17d7 | 0x17dc..=0x17dd | 0x17e0..=0x17e9 | 0x17f0..=0x17f9
                | 0x1000..=0x1049 | 0x1050..=0x109d | 0xa9e0..=0xa9fe | 0xaa60..=0xaa76 | 0xaa7a..=0xaa7f
        )
}

fn paragraph_units(text: &str) -> usize {
    let mut utf8 = [0; 4];
    text.chars()
        .map(|c| dialoguer::console::measure_text_width(c.encode_utf8(&mut utf8)).max(1))
        .sum()
}

fn continues_grapheme(paragraph: &str, next: &str) -> bool {
    let (Some(left), Some(right)) = (
        paragraph.graphemes(true).next_back(),
        next.graphemes(true).next(),
    ) else {
        return false;
    };
    ![left, right]
        .concat()
        .grapheme_indices(true)
        .any(|(offset, _)| offset == left.len())
}

fn needs_space(paragraph: &str, next: &str) -> bool {
    if continues_grapheme(paragraph, next) {
        return false;
    }
    let boundary_character = |c: &char| {
        !matches!(
            get_general_category(*c),
            GeneralCategory::NonspacingMark
                | GeneralCategory::EnclosingMark
                | GeneralCategory::Format
        )
    };
    match (
        paragraph.chars().rfind(boundary_character),
        next.chars().find(boundary_character),
    ) {
        (Some(left), Some(right)) => {
            !left.is_whitespace()
                && !right.is_whitespace()
                && !(is_unspaced(left) && is_unspaced(right))
        }
        _ => false,
    }
}

/// Default to a space unless both boundary characters use unspaced scripts.
fn append_text(paragraph: &mut String, next: &str) {
    if needs_space(paragraph, next) {
        paragraph.push(' ');
    }
    paragraph.push_str(next);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(t: f64) -> FrameEvent {
        FrameEvent {
            t,
            image: format!("f{t}.jpg"),
        }
    }

    fn sp(start: f64, end: f64) -> TranscriptEvent {
        TranscriptEvent {
            start,
            end,
            text: format!("{start}-{end}"),
            raw: None,
            below: Vec::new(),
        }
    }

    #[test]
    fn merge_uses_natural_breaks_or_keeps_complete_speech() {
        let frames = vec![frame(0.0), frame(60.0), frame(120.0)];
        let speech = vec![sp(10.0, 20.0), sp(50.0, 70.0), sp(5.0, 8.0)];
        let s = merge(frames, speech, 120.0);
        // sp(50,70) 文本无句读：整段保留，按中点归属 slide1（不从词中间截断）
        assert_eq!(s[0].speech.len(), 2);
        assert_eq!(s[1].speech.len(), 1);
        assert_eq!(s[1].speech[0].text, "50-70");
        assert_eq!(s[1].speech[0].start, 50.0);
        assert_eq!(s[1].speech[0].end, 70.0);
        assert!(merge(vec![], vec![sp(1.0, 2.0)], 10.0).is_empty());
    }

    #[test]
    fn merge_splits_at_punctuation_near_boundary() {
        let frames = vec![frame(0.0), frame(5.0)];
        let speech = vec![TranscriptEvent {
            start: 0.0,
            end: 10.0,
            text: "前半句，后半句。".into(),
            raw: None,
            below: Vec::new(),
        }];
        let s = merge(frames, speech, 10.0);
        // 边界 5s 附近有句读「，」：安全拆分，图文对应
        assert_eq!(s[0].speech.len(), 1);
        assert_eq!(s[1].speech.len(), 1);
        assert_eq!(s[0].speech[0].text, "前半句，");
        assert_eq!(s[1].speech[0].text, "后半句。");
    }

    #[test]
    fn no_punctuation_means_keep_whole() {
        // 无句读文本：无论多少边界，整段保留（不按比例词中截断）
        let ev = TranscriptEvent {
            start: 0.0,
            end: 10.0,
            text: "一二三四五六七八九十".into(),
            raw: None,
            below: Vec::new(),
        };
        let parts = split_at_natural_boundaries(ev.clone(), &[5.0]);
        assert_eq!(parts.len(), 1, "无句读不切分");
        assert_eq!(parts[0].text, ev.text);

        // 边界太靠近端点（<0.3s）也不拆
        let ev2 = TranscriptEvent {
            start: 0.0,
            end: 1.0,
            text: "abc".into(),
            raw: None,
            below: Vec::new(),
        };
        assert_eq!(split_at_natural_boundaries(ev2, &[0.1]).len(), 1);
    }

    #[test]
    fn split_snaps_to_nearest_punctuation() {
        // 比例切点 idx=5（「很」），最近的句读是 idx=3 的「，」→ 切在其后
        let ev = TranscriptEvent {
            start: 0.0,
            end: 10.0,
            text: "你好，世界很好。再见".into(),
            raw: None,
            below: Vec::new(),
        };
        let parts = split_at_natural_boundaries(ev.clone(), &[5.0]);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].text, "你好，");
        assert_eq!(parts[1].text, "世界很好。再见");

        // 英文：吸附到最近的空格，不切在单词中间
        let ev2 = TranscriptEvent {
            start: 0.0,
            end: 10.0,
            text: "compiler optimization lecture".into(),
            raw: None,
            below: Vec::new(),
        };
        let parts2 = split_at_natural_boundaries(ev2, &[5.0]);
        assert_eq!(parts2[0].text, "compiler ");
        assert_eq!(parts2[1].text, "optimization lecture");
    }

    #[test]
    fn multi_boundary_all_or_nothing() {
        // 两个边界：一个有句读一个没有 → 整段保留
        let ev = TranscriptEvent {
            start: 0.0,
            end: 30.0,
            text: "零一，二三四五六七八九十".into(),
            raw: None,
            below: Vec::new(),
        };
        let parts = split_at_natural_boundaries(ev.clone(), &[10.0, 20.0]);
        assert_eq!(parts.len(), 1, "任一边界无自然断点则整段保留：{parts:?}");
        assert_eq!(parts[0].text, ev.text);
    }

    #[test]
    fn coalesce_merges_fragments_into_paragraphs() {
        let sections = vec![Section {
            t: 0.0,
            end: 30.0,
            image: "f.jpg".into(),
            speech: vec![
                sp(0.0, 2.0),
                sp(2.5, 4.0),   // 与上段间隔 0.5s：合并
                sp(10.0, 12.0), // 间隔 6s > 3.5s：分段
                sp(12.1, 14.0), // 合并
            ],
        }];
        let mut sections = sections;
        coalesce_sections(&mut sections);
        assert_eq!(sections[0].speech.len(), 2, "两段");
        assert_eq!(sections[0].speech[0].text, "0-2 2.5-4");
        assert_eq!(sections[0].speech[1].text, "10-12 12.1-14");
        assert!((sections[0].speech[0].start - 0.0).abs() < 1e-9);
        assert!(
            (sections[0].speech[0].end - 4.0).abs() < 1e-9,
            "段落时间覆盖成员区间"
        );
    }

    #[test]
    fn coalesce_breaks_on_length_limit() {
        for (long_text, expected) in [
            ("word ".repeat(84), 1),
            ("word ".repeat(85), 2),
            ("x".repeat(419), 1),
            ("x".repeat(420), 2),
            ("字".repeat(210), 1),
            ("字".repeat(211), 2),
            ("字".repeat(300), 2),
        ] {
            let speech = vec![
                TranscriptEvent {
                    start: 0.0,
                    end: 2.0,
                    text: long_text.clone(),
                    raw: None,
                    below: Vec::new(),
                },
                TranscriptEvent {
                    start: 2.1,
                    end: 4.0,
                    text: long_text.clone(),
                    raw: None,
                    below: Vec::new(),
                },
            ];
            let mut sections = vec![Section {
                t: 0.0,
                end: 10.0,
                image: "f.jpg".into(),
                speech,
            }];
            coalesce_sections(&mut sections);
            assert_eq!(sections[0].speech.len(), expected, "{long_text}");
        }
        assert_eq!(paragraph_units("a字あカ한ᄒㄱｶ𠀀"), 16);
        assert_eq!(paragraph_units("、・。"), 6);
        assert_eq!(paragraph_units("々〇"), 4);
        assert_eq!(paragraph_units("ｶ\u{ffa1}"), 2);
    }

    #[test]
    fn coalesce_keeps_grapheme_continuations_despite_length_or_time_gaps() {
        for prefix in [String::new(), "x".repeat(MAX_PARAGRAPH_UNITS)] {
            for start in [2.1, 10.0] {
                let mut sections = vec![Section {
                    t: 0.,
                    end: start + 1.,
                    image: String::new(),
                    speech: vec![
                        TranscriptEvent {
                            text: format!("{prefix}e"),
                            ..sp(0., 2.)
                        },
                        TranscriptEvent {
                            text: "\u{301}clair".into(),
                            ..sp(start, start + 1.)
                        },
                    ],
                }];
                coalesce_sections(&mut sections);
                assert_eq!(sections[0].speech.len(), 1);
                assert_eq!(sections[0].speech[0].text, format!("{prefix}e\u{301}clair"));
                assert_eq!(sections[0].speech[0].start, 0.);
                assert_eq!(sections[0].speech[0].end, start + 1.);
            }
        }
    }

    #[test]
    fn coalesce_preserves_all_nonempty_speech() {
        for text in ["um", "HMM!", "음", "嗯，", "えっと", "Ordinary words."] {
            let mut sections = vec![Section {
                t: 0.,
                end: 1.,
                image: String::new(),
                speech: vec![TranscriptEvent {
                    text: text.into(),
                    ..sp(0., 1.)
                }],
            }];
            coalesce_sections(&mut sections);
            assert_eq!(sections[0].speech[0].text, text);
        }
    }

    #[test]
    fn coalesce_joins_english_with_space() {
        let speech = vec![
            TranscriptEvent {
                start: 0.0,
                end: 2.0,
                text: "hello world".into(),
                raw: None,
                below: Vec::new(),
            },
            TranscriptEvent {
                start: 2.2,
                end: 4.0,
                text: "next sentence".into(),
                raw: None,
                below: Vec::new(),
            },
        ];
        let mut sections = vec![Section {
            t: 0.0,
            end: 10.0,
            image: "f.jpg".into(),
            speech,
        }];
        coalesce_sections(&mut sections);
        assert_eq!(sections[0].speech.len(), 1);
        assert_eq!(sections[0].speech[0].text, "hello world next sentence");
    }

    #[test]
    fn fragment_boundaries_default_to_spaces_except_between_unspaced_scripts() {
        for (left, right, expected) in [
            (
                "Le risque est limité.",
                "La suite.",
                "Le risque est limité. La suite.",
            ),
            ("café", "économique", "café économique"),
            ("오늘은", "수업입니다", "오늘은 수업입니다"),
            ("한", "글", "한 글"),
            ("ㄱ", "ㄴ", "ㄱ ㄴ"),
            ("\u{ffa1}", "\u{ffa2}", "\u{ffa1} \u{ffa2}"),
            ("A risk?", "Yes.", "A risk? Yes."),
            ("a word", ", next", "a word , next"),
            ("l’", "investissement", "l’ investissement"),
            ("long-", "term", "long- term"),
            ("السؤال؟", "الجواب", "السؤال؟ الجواب"),
            ("风险。", "收益。", "风险。 收益。"),
            ("字幕は", "保持する。", "字幕は保持する。"),
            ("ภาษา", "ไทย", "ภาษาไทย"),
            ("ພາສາ", "ລາວ", "ພາສາລາວ"),
            ("ភាសា", "ខ្មែរ", "ភាសាខ្មែរ"),
            ("မြန်မာ", "စာ", "မြန်မာစာ"),
            ("ပ", "့", "ပ့"),
            ("あ", "\u{3099}", "あ\u{3099}"),
            ("字", "word", "字 word"),
            ("word ", "next", "word next"),
            ("word", "\tnext", "word\tnext"),
            ("字\n", "字", "字\n字"),
            ("", "right", "right"),
            ("left", "", "left"),
        ] {
            let mut paragraph = left.to_owned();
            append_text(&mut paragraph, right);
            assert_eq!(paragraph, expected);
        }
    }

    #[test]
    fn fragment_joining_preserves_marks_and_never_splits_a_grapheme() {
        for (left, right, expected) in [
            ("葛\u{e0100}", "飾", "葛\u{e0100}飾"),
            ("葛", "\u{e0100}飾", "葛\u{e0100}飾"),
            ("e", "\u{301}clair", "e\u{301}clair"),
            ("cafe\u{301}", "noir", "cafe\u{301} noir"),
            ("A", "\u{20dd}", "A\u{20dd}"),
            ("字\u{200e}", "字", "字\u{200e}字"),
            ("a", "\u{200e}b", "a \u{200e}b"),
            ("👩\u{200d}", "💻", "👩\u{200d}💻"),
            ("👍", "🏽", "👍🏽"),
            ("🇰", "🇷", "🇰🇷"),
            ("क्", "ष", "क्ष"),
        ] {
            let mut paragraph = left.to_owned();
            append_text(&mut paragraph, right);
            assert_eq!(paragraph, expected);
        }
    }

    #[test]
    fn section_end_is_next_frame_start_or_media_end() {
        let frames = vec![frame(0.0), frame(60.0)];
        let s = merge(frames, vec![], 95.0);
        assert_eq!(s[0].end, 60.0);
        assert_eq!(s[1].end, 95.0, "末段 end = 媒体时长");
        // 时长未知（0）时退化为自身起点，不 panic 不产生负区间
        let s2 = merge(vec![frame(5.0)], vec![], 0.0);
        assert_eq!(s2[0].end, 5.0);
    }
}
