//! Language names for the chat-ASR language hint.

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

#[cfg(test)]
mod tests {
    #[test]
    fn language_names_accept_subtags_and_preserve_unknown_codes() {
        for (code, name) in [
            ("ko", "Korean"),
            ("pt-BR", "Portuguese"),
            ("zh-Hant", "Traditional Chinese"),
            ("sr-Latn", "Serbian"),
            ("xx-custom", "xx-custom"),
        ] {
            assert_eq!(super::english_name(code), name);
        }
    }
}
