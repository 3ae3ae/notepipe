//! Translation targets and a light guess of a transcript's language.
//!
//! The guess only decides whether a bilingual note needs translating at all,
//! so it compares language families: Simplified and Traditional Chinese count
//! as the same language, and a transcript too short or mixed to judge is
//! translated rather than skipped.

/// Target languages offered for bilingual notes: stable code and the name
/// the AI service is asked to write in.
pub const TARGETS: [(&str, &str); 8] = [
    ("zh-Hans", "Simplified Chinese"),
    ("zh-Hant", "Traditional Chinese"),
    ("en", "English"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("fr", "French"),
    ("de", "German"),
    ("es", "Spanish"),
];

/// The supported target matching `code` (case and `_`/`-` insensitive).
pub fn target(code: &str) -> Option<&'static str> {
    let wanted = code.trim().replace('_', "-").to_ascii_lowercase();
    TARGETS
        .iter()
        .map(|(code, _)| *code)
        .find(|code| code.to_ascii_lowercase() == wanted)
}

/// The English name used in the translation instruction.
pub fn english_name(code: &str) -> &'static str {
    TARGETS
        .iter()
        .find(|(target, _)| *target == code)
        .map(|(_, name)| *name)
        .unwrap_or("English")
}

/// Language family of a target code: `zh-Hans` and `zh-Hant` are both `zh`.
fn family(code: &str) -> &str {
    code.split('-').next().unwrap_or(code)
}

/// Whether text that `detect` judged to be `detected` is already in `target`.
pub fn already_in(detected: Option<&str>, target: &str) -> bool {
    detected.is_some_and(|detected| family(detected) == family(target))
}

/// Minimum letters before a guess is trusted.
const MIN_LETTERS: usize = 40;

/// Language family of `text` (`zh`, `ja`, `ko`, `en`, `fr`, `de`, `es`), or
/// `None` when the sample is too small or no language clearly dominates.
pub fn detect(text: &str) -> Option<&'static str> {
    let (mut han, mut kana, mut hangul, mut latin) = (0usize, 0usize, 0usize, 0usize);
    for c in text.chars() {
        match c as u32 {
            0x3040..=0x30FF | 0x31F0..=0x31FF => kana += 1,
            0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => hangul += 1,
            0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF => han += 1,
            _ if c.is_alphabetic() => latin += 1,
            _ => {}
        }
    }
    let letters = han + kana + hangul + latin;
    if letters < MIN_LETTERS {
        return None;
    }
    // Japanese mixes kanji with kana; a modest share of kana decides it.
    if kana * 10 >= letters {
        return Some("ja");
    }
    if hangul * 2 >= letters {
        return Some("ko");
    }
    if han * 2 >= letters {
        return Some("zh");
    }
    if latin * 10 >= letters * 8 {
        return latin_language(text);
    }
    None
}

/// Frequent short words that tell the Latin-script targets apart.
const STOPWORDS: [(&str, &[&str]); 4] = [
    (
        "en",
        &[
            "the", "and", "is", "of", "to", "in", "that", "it", "you", "this", "we", "for", "are",
            "with", "what", "so",
        ],
    ),
    (
        "fr",
        &[
            "le", "la", "les", "et", "est", "des", "une", "un", "que", "qui", "dans", "pour",
            "pas", "nous", "vous", "ce",
        ],
    ),
    (
        "de",
        &[
            "der", "die", "das", "und", "ist", "nicht", "ein", "eine", "ich", "wir", "zu", "mit",
            "den", "auf", "sie", "es",
        ],
    ),
    (
        "es",
        &[
            "el", "la", "los", "las", "y", "es", "que", "de", "en", "un", "una", "por", "para",
            "con", "no", "se",
        ],
    ),
];

fn latin_language(text: &str) -> Option<&'static str> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphabetic() && c != '\'')
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut scores: Vec<(&'static str, usize)> = STOPWORDS
        .iter()
        .map(|(code, list)| {
            (
                *code,
                words
                    .iter()
                    .filter(|word| list.contains(&word.as_str()))
                    .count(),
            )
        })
        .collect();
    scores.sort_by_key(|score| std::cmp::Reverse(score.1));
    let (best, top) = scores[0];
    let second = scores[1].1;
    // A clear winner among enough function words; otherwise do not guess.
    (top >= 5 && top * 2 > second * 3).then_some(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_and_function_words_identify_the_language() {
        let chinese = "今天我们来讲一下线性代数里面最重要的概念，也就是矩阵的特征值和特征向量。";
        assert_eq!(detect(&chinese.repeat(2)), Some("zh"));
        let japanese = "今日は線形代数の中で最も重要な概念、つまり行列の固有値と固有ベクトルについて説明します。";
        assert_eq!(detect(&japanese.repeat(2)), Some("ja"));
        let korean = "오늘은 선형대수에서 가장 중요한 개념인 행렬의 고유값과 고유벡터에 대해 설명하겠습니다.";
        assert_eq!(detect(&korean.repeat(2)), Some("ko"));
        let english = "So today we are going to talk about the most important idea in linear algebra, \
                       and that is what we call the eigenvalues of a matrix. It is the key to this course.";
        assert_eq!(detect(english), Some("en"));
        let french = "Aujourd'hui nous allons parler de la notion la plus importante dans les cours \
                      d'algèbre, et c'est une idée que vous devez comprendre pour la suite des cours.";
        assert_eq!(detect(french), Some("fr"));
        let german = "Heute sprechen wir über das wichtigste Konzept der linearen Algebra, und das ist \
                      die Idee, die wir in der Vorlesung mit den Eigenwerten einer Matrix beschreiben.";
        assert_eq!(detect(german), Some("de"));
        assert_eq!(detect("short"), None);
    }

    #[test]
    fn chinese_variants_are_one_language() {
        assert!(already_in(Some("zh"), "zh-Hant"));
        assert!(already_in(Some("en"), "en"));
        assert!(!already_in(Some("en"), "zh-Hans"));
        assert!(!already_in(None, "en"));
    }

    #[test]
    fn targets_parse_loosely() {
        assert_eq!(target("zh_hans"), Some("zh-Hans"));
        assert_eq!(target(" EN "), Some("en"));
        assert_eq!(target("xx"), None);
        assert_eq!(english_name("ja"), "Japanese");
    }
}
