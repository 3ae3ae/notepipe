//! Minimal Markdown assembly, keeping input order.
use crate::timeline::Section;

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
pub fn document(sections: &[Section]) -> String {
    let mut parts = Vec::new();
    for section in sections {
        if !section.image.is_empty() {
            parts.push(format!("![](<{}>)", section.image));
        }
        parts.extend(
            section
                .speech
                .iter()
                .map(|p| trim_blank_lines(&p.text).to_owned())
                .filter(|s| !s.is_empty()),
        );
    }
    parts.join("\n\n")
}
pub fn markdown(inputs: &[String]) -> String {
    let text = inputs
        .iter()
        .map(|s| trim_blank_lines(s))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("{text}\n")
}
fn trim_blank_lines(mut text: &str) -> &str {
    while let Some((line, rest)) = text.split_once('\n') {
        if !line.trim().is_empty() {
            break;
        }
        text = rest;
    }
    while let Some((rest, line)) = text.rsplit_once('\n') {
        if !line.trim().is_empty() {
            break;
        }
        text = rest;
    }
    if text.trim().is_empty() { "" } else { text }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rendering_drops_only_outer_blank_lines_and_preserves_indentation() {
        let input = " \n\t\n    code  \n\n  next\t\n \t\n";
        let expected = "    code  \n\n  next\t";
        let sections: Vec<Section> = serde_json::from_value(serde_json::json!([{
            "t": 0, "end": 1, "image": "", "speech": [{"start": 0, "end": 1, "text": input}]
        }]))
        .unwrap();
        assert_eq!(document(&sections), expected);
        assert_eq!(
            markdown(&[input.into(), " \n\t".into(), input.into()]),
            format!("{expected}\n\n{expected}\n")
        );
        assert_eq!(markdown(&[" \n\t".into()]), "\n");
    }

    #[test]
    fn minimal_layout_preserves_order_and_one_trailing_newline() {
        let sections = serde_json::from_str::<Vec<Section>>(r#"[{"t":0,"end":1,"image":"images/0001.jpg","speech":[{"start":0,"end":1,"text":" First "}]},{"t":1,"end":2,"image":"","speech":[{"start":1,"end":2,"text":"Second"}]}]"#).unwrap();
        assert_eq!(
            markdown(&[
                "\n## Summary\n".into(),
                document(&sections),
                "  Tail\n\n".into()
            ]),
            "## Summary\n\n![](<images/0001.jpg>)\n\n First \n\nSecond\n\n  Tail\n"
        );
        assert_eq!(markdown(&[document(&sections[1..])]), "Second\n");
    }
}
