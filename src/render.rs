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
                .map(|p| p.text.trim().to_owned())
                .filter(|s| !s.is_empty()),
        );
    }
    parts.join("\n\n")
}
pub fn markdown(inputs: &[String]) -> String {
    let text = inputs
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("{text}\n")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn minimal_layout_preserves_order_and_one_trailing_newline() {
        let sections = serde_json::from_str::<Vec<Section>>(r#"[{"t":0,"end":1,"image":"images/0001.jpg","speech":[{"start":0,"end":1,"text":" First "}]},{"t":1,"end":2,"image":"","speech":[{"start":1,"end":2,"text":"Second"}]}]"#).unwrap();
        assert_eq!(
            markdown(&[
                "\n## Summary\n".into(),
                document(&sections),
                "  Tail\n\n".into()
            ]),
            "## Summary\n\n![](<images/0001.jpg>)\n\nFirst\n\nSecond\n\nTail\n"
        );
        assert_eq!(markdown(&[document(&sections[1..])]), "Second\n");
    }
}
