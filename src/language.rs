//! Translation targets and a light guess of a transcript's language.
//!
//! The guess only decides whether a bilingual note needs translating at all,
//! so it compares language families. Chinese script/region targets always need
//! translation, as does a transcript too short or mixed to judge.

/// English names shared by translation instructions and speech language hints.
const LANGUAGE_NAMES: &[(&str, &str)] = &[
    ("af", "Afrikaans"),
    ("am", "Amharic"),
    ("ar", "Arabic"),
    ("az", "Azerbaijani"),
    ("bg", "Bulgarian"),
    ("bn", "Bengali"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("en", "English"),
    ("es", "Spanish"),
    ("fa", "Persian"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("gu", "Gujarati"),
    ("ha", "Hausa"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hu", "Hungarian"),
    ("id", "Indonesian"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("jv", "Javanese"),
    ("kn", "Kannada"),
    ("ko", "Korean"),
    ("ml", "Malayalam"),
    ("mr", "Marathi"),
    ("ms", "Malay"),
    ("my", "Burmese"),
    ("ne", "Nepali"),
    ("nl", "Dutch"),
    ("no", "Norwegian"),
    ("pa", "Punjabi"),
    ("pl", "Polish"),
    ("ps", "Pashto"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("si", "Sinhala"),
    ("so", "Somali"),
    ("sr", "Serbian"),
    ("su", "Sundanese"),
    ("sv", "Swedish"),
    ("sw", "Swahili"),
    ("ta", "Tamil"),
    ("te", "Telugu"),
    ("th", "Thai"),
    ("tl", "Tagalog"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("ur", "Urdu"),
    ("uz", "Uzbek"),
    ("vi", "Vietnamese"),
    ("yo", "Yoruba"),
    ("zh", "Chinese"),
    ("zh-Hans", "Simplified Chinese"),
    ("zh-Hant", "Traditional Chinese"),
];

/// Validate a language code or name, preserving the trimmed user input.
pub fn target(value: &str) -> Result<&str, &'static str> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 40 || value.chars().any(char::is_control) {
        return Err(
            "Translation target must be a non-empty language code or name of at most 40 characters, with no control characters",
        );
    }
    Ok(trimmed)
}

/// English language name, accepting region/script subtags; unknown codes pass through.
pub fn english_name(code: &str) -> &str {
    let normalized = code.trim().replace('_', "-");
    let mut wanted = normalized.as_str();
    loop {
        if let Some((_, name)) = LANGUAGE_NAMES
            .iter()
            .find(|(target, _)| target.eq_ignore_ascii_case(wanted))
        {
            return name;
        }
        let Some((parent, _)) = wanted.rsplit_once('-') else {
            return code;
        };
        wanted = parent;
    }
}

/// Language family of a target code: `zh-Hans` and `zh-Hant` are both `zh`.
fn family(code: &str) -> &str {
    code.split(['-', '_']).next().unwrap_or(code)
}

/// Whether text that `detect` judged to be `detected` is already in `target`.
pub fn already_in(detected: Option<&str>, target: &str) -> bool {
    let target = target.trim();
    let language = family(target);
    if language.eq_ignore_ascii_case("zh") && target.contains(['-', '_']) {
        return false;
    }
    detected.is_some_and(|detected| family(detected).eq_ignore_ascii_case(language))
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
    fn chinese_script_and_region_targets_always_translate() {
        for target in ["zh-Hans", "zh-Hant", "zh-TW", "ZH_hant", "zh-HK"] {
            for detected in [Some("zh"), Some("zh-Hant"), None] {
                assert!(!already_in(detected, target), "{detected:?} → {target}");
            }
        }
        assert!(already_in(Some("zh"), "zh"));
        assert!(already_in(Some("en"), "en"));
        assert!(already_in(Some("en"), "EN_us"));
        assert!(!already_in(Some("en"), "zh-Hans"));
        assert!(!already_in(None, "en"));
    }

    #[test]
    fn targets_accept_trimmed_codes_and_names() {
        for value in ["zh_hans", "EN", "pt-BR", "Arabic", "Canadian French", "xx"] {
            assert_eq!(target(&format!("  {value}  ")), Ok(value));
        }
        let forty_chars = "語".repeat(40);
        assert_eq!(target(&forty_chars), Ok(forty_chars.as_str()));
        for value in [
            "",
            "   ",
            "en\n",
            "\tja",
            "A\0B",
            "ar\u{85}",
            &"語".repeat(41),
        ] {
            assert!(target(value).is_err(), "{value:?}");
        }
        assert_eq!(english_name("ja"), "Japanese");
    }

    #[test]
    fn language_names_accept_subtags_and_preserve_unknown_codes() {
        for (code, name) in [
            ("zh", "Chinese"),
            ("pt", "Portuguese"),
            ("pt-BR", "Portuguese"),
            (" PT_br ", "Portuguese"),
            ("zh-Hans", "Simplified Chinese"),
            ("zh-Hant", "Traditional Chinese"),
            ("zh_hant_TW", "Traditional Chinese"),
            ("xx-custom", "xx-custom"),
        ] {
            assert_eq!(english_name(code), name);
        }
        assert_eq!(target("pt-BR"), Ok("pt-BR"));
    }
}
