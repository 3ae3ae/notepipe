//! Header-driven interface languages and the English-source message catalog.
//!
//! Empty translations fall back to English; Traditional Chinese first tries
//! Simplified Chinese. Missing messages keep their English source text.
//! Call sites use [`tr!`] and its existing `{}` placeholder syntax.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Lang(usize);

impl Lang {
    pub fn source() -> Self {
        Self(0)
    }

    /// Stable language code from the catalog header.
    pub fn code(self) -> &'static str {
        catalog().languages[self.0]
    }

    pub fn from_code(value: &str) -> Option<Self> {
        catalog().language(value)
    }

    /// `NOTEPIPE_LANG`, then POSIX locale variables, then the platform locale.
    pub fn detect() -> Self {
        for name in ["NOTEPIPE_LANG", "LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(value) = std::env::var(name)
                && let Some(lang) = Self::from_code(&value)
            {
                return lang;
            }
        }
        sys_locale::get_locale()
            .as_deref()
            .and_then(Self::from_code)
            .unwrap_or(Self::source())
    }
}

pub fn languages() -> impl Iterator<Item = Lang> {
    (0..catalog().languages.len()).map(Lang)
}

// Library callers keep the source language until an application selects one.
static CURRENT: AtomicUsize = AtomicUsize::new(0);

pub fn set_lang(lang: Lang) {
    CURRENT.store(lang.0, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    Lang(CURRENT.load(Ordering::Relaxed))
}

pub fn init_from_env() -> Lang {
    let lang = Lang::detect();
    set_lang(lang);
    lang
}

const CATALOG: &str = include_str!("../i18n/catalog.tsv");

struct Catalog {
    languages: Vec<&'static str>,
    entries: HashMap<&'static str, Vec<&'static str>>,
}

impl Catalog {
    fn language(&self, value: &str) -> Option<Lang> {
        let normalized = value.trim().replace('_', "-").to_ascii_lowercase();
        let value = normalized.split(['.', '@']).next().unwrap_or_default();
        let mut candidate = value;
        loop {
            if let Some(index) = self
                .languages
                .iter()
                .position(|code| code.eq_ignore_ascii_case(candidate))
            {
                return Some(Lang(index));
            }
            let Some((parent, _)) = candidate.rsplit_once('-') else {
                break;
            };
            candidate = parent;
        }
        if matches!(candidate, "zh" | "cmn" | "yue") {
            let traditional = value
                .split('-')
                .any(|part| matches!(part, "hant" | "tw" | "hk" | "mo"));
            let code = if traditional { "zh-Hant" } else { "zh-Hans" };
            return self
                .languages
                .iter()
                .position(|header| *header == code)
                .map(Lang);
        }
        None
    }

    fn text<'a>(&self, source: &'a str, lang: Lang) -> &'a str {
        let Some(entry) = self.entries.get(source) else {
            return source;
        };
        let translated = |index: usize| entry.get(index).copied().filter(|text| !text.is_empty());
        translated(lang.0)
            .or_else(|| {
                if self.languages[lang.0] != "zh-Hant" {
                    return None;
                }
                self.languages
                    .iter()
                    .position(|code| *code == "zh-Hans")
                    .and_then(translated)
            })
            .unwrap_or(source)
    }
}

fn catalog() -> &'static Catalog {
    static CATALOG_INDEX: OnceLock<Catalog> = OnceLock::new();
    CATALOG_INDEX.get_or_init(|| parse_catalog(CATALOG))
}

fn leak(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

fn parse_catalog(source: &'static str) -> Catalog {
    let mut lines = source.lines();
    let languages = split_fields(lines.next().expect("Catalog needs a header"));
    assert_eq!(
        languages.first(),
        Some(&"en"),
        "Catalog source language must be English"
    );
    assert!(
        languages
            .iter()
            .enumerate()
            .all(|(index, code)| !code.is_empty() && !languages[..index].contains(code)),
        "Duplicate or empty language code"
    );
    let mut entries = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let fields = split_fields(line);
        assert!(
            fields.len() <= languages.len(),
            "Too many catalog cells: {line}"
        );
        let fields: Vec<_> = fields
            .into_iter()
            .map(|field| leak(unescape(field)))
            .collect();
        let key = fields[0];
        assert!(!key.is_empty(), "Empty catalog key");
        assert!(
            entries.insert(key, fields).is_none(),
            "Duplicate catalog key: {key}"
        );
    }
    Catalog { languages, entries }
}

fn split_fields(line: &str) -> Vec<&str> {
    // Escaped tabs are `\t` in the file, so a literal tab is always a separator.
    line.split('\t').collect()
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Ok(code) = u32::from_str_radix(&hex, 16)
                    && let Some(c) = char::from_u32(code)
                {
                    out.push(c);
                }
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn t_in(source: &str, lang: Lang) -> &str {
    catalog().text(source, lang)
}

/// Translates a source message into the current interface language.
///
/// Missing entries keep the source text, so an incomplete catalog never
/// makes a message unreadable.
pub fn t(source: &str) -> &str {
    t_in(source, lang())
}

/// Translates a message and fills its `{}` placeholders.
///
/// ```ignore
/// tr!("Speech transcription failed for segment {}.", count)
/// ```
#[macro_export]
macro_rules! tr {
    ($text:expr $(,)?) => {
        $crate::i18n::t($text)
    };
    ($text:expr, $($arg:expr),+ $(,)?) => {{
        // Build the slice inside the call so temporaries live until formatting ends.
        $crate::i18n::template($text, &[$(&$arg as &dyn ::std::fmt::Display),+])
    }};
}

/// Localized `println!`; placeholders use the same subset as [`tr!`].
#[macro_export]
macro_rules! println_tr {
    ($($arg:tt)*) => {
        println!("{}", $crate::tr!($($arg)*))
    };
}

/// Localized `eprintln!`.
#[macro_export]
macro_rules! eprintln_tr {
    ($($arg:tt)*) => {
        eprintln!("{}", $crate::tr!($($arg)*))
    };
}

/// Localized `print!`.
#[macro_export]
macro_rules! print_tr {
    ($($arg:tt)*) => {
        print!("{}", $crate::tr!($($arg)*))
    };
}

/// Localized `eprint!`.
#[macro_export]
macro_rules! eprint_tr {
    ($($arg:tt)*) => {
        eprint!("{}", $crate::tr!($($arg)*))
    };
}

/// Fills `{}` placeholders in an already translated template.
///
/// `{{` and `}}` are literal braces. A missing translation keeps the source
/// template; placeholders are consumed left to right.
pub fn template(source: &str, args: &[&dyn Display]) -> String {
    let template = t(source);
    let mut out = String::with_capacity(template.len());
    let mut args = args.iter();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                // Only the plain `{}` form is used by the catalog.
                if chars.peek() == Some(&'}') {
                    chars.next();
                }
                if let Some(arg) = args.next() {
                    out.push_str(&arg.to_string());
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// Looks up a source string without consulting the global language.
/// Used by tests and by callers that resolve a language per document.
pub fn t_for(source: &str, lang: Lang) -> &str {
    t_in(source, lang)
}

/// Catalog coverage for a source string (diagnostics and tests).
pub fn has_translation(source: &str) -> bool {
    catalog().entries.contains_key(source)
}

/// Rendered translation table used by tooling that audits coverage.
pub fn catalog_len() -> usize {
    catalog().entries.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_codes_round_trip() {
        for lang in languages() {
            assert_eq!(Lang::from_code(lang.code()), Some(lang));
        }
        for (locale, code) in [
            ("zh_CN.UTF-8", "zh-Hans"),
            ("zh_TW.UTF-8", "zh-Hant"),
            ("zh-HK", "zh-Hant"),
            ("ja_JP", "ja"),
            ("en_US.UTF-8", "en"),
        ] {
            assert_eq!(Lang::from_code(locale), Lang::from_code(code));
        }
        assert_eq!(Lang::from_code("C"), None);
        assert_eq!(Lang::from_code("auto"), None);
    }

    #[test]
    fn headers_define_languages_and_fallbacks() {
        let catalog = parse_catalog(
            "en\tja\tzh-Hant\tko\tzh-Hans\nMessage\t\t\tMessage in Korean\tMessage in Simplified Chinese\nSource only\n",
        );
        let korean = catalog.language("ko_KR.UTF-8").unwrap();
        assert_eq!(catalog.languages[korean.0], "ko");
        assert_eq!(catalog.text("Message", korean), "Message in Korean");
        assert_eq!(
            catalog.text("Message", catalog.language("zh-Hant").unwrap()),
            "Message in Simplified Chinese"
        );
        assert_eq!(
            catalog.text("Message", catalog.language("ja").unwrap()),
            "Message"
        );
        assert_eq!(catalog.text("Source only", korean), "Source only");
        assert_eq!(catalog.text("Unknown", korean), "Unknown");
        let without_simplified = parse_catalog("en\tzh-Hant\nMessage\n");
        assert_eq!(without_simplified.text("Message", Lang(1)), "Message");
    }

    #[test]
    #[should_panic(expected = "Too many catalog cells")]
    fn extra_catalog_cells_are_rejected() {
        parse_catalog("en\tja\nMessage\tJapanese\textra");
    }

    #[test]
    fn template_fills_placeholders() {
        assert_eq!(
            template("Speech transcription failed for segment {}.", &[&3]),
            "Speech transcription failed for segment 3."
        );
        // Synthetic templates are intentionally untranslated.
        assert_eq!(template("Braces {{}} stay", &[]), "Braces {} stay");
    }

    // Test fixtures intentionally use untranslated text; audit production lookups.
    fn lookup_literals() -> Vec<(std::path::PathBuf, String)> {
        let mut directories =
            vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut literals = Vec::new();
        while let Some(directory) = directories.pop() {
            for file in std::fs::read_dir(directory).unwrap() {
                let path = file.unwrap().path();
                if path.is_dir() {
                    directories.push(path);
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).unwrap();
                let source = source.split("#[cfg(test)]\nmod ").next().unwrap();
                for call in ["tr!(", "t(", "t_for(", "template("] {
                    for (offset, _) in source.match_indices(call) {
                        if call != "tr!("
                            && source[..offset]
                                .chars()
                                .next_back()
                                .is_some_and(|c| c.is_alphanumeric() || c == '_')
                        {
                            continue;
                        }
                        let tail = source[offset + call.len()..].trim_start();
                        let key = if let Some(body) = tail.strip_prefix('"') {
                            let mut escaped = false;
                            let end = body
                                .char_indices()
                                .find_map(|(index, c)| {
                                    if escaped {
                                        escaped = false;
                                        return None;
                                    }
                                    if c == '\\' {
                                        escaped = true;
                                        return None;
                                    }
                                    (c == '"').then_some(index)
                                })
                                .unwrap();
                            unescape(&body[..end].replace("\\\"", "\""))
                        } else if let Some(raw) = tail.strip_prefix('r') {
                            let Some((marks, body)) = raw.split_once('"') else {
                                continue;
                            };
                            if !marks.chars().all(|c| c == '#') {
                                continue;
                            }
                            body.split_once(&format!("\"{marks}")).unwrap().0.to_owned()
                        } else {
                            continue;
                        };
                        literals.push((path.clone(), key));
                    }
                }
            }
        }
        literals
    }

    #[test]
    fn every_lookup_literal_has_a_catalog_row() {
        for (path, key) in lookup_literals() {
            assert!(
                has_translation(&key),
                "{}: missing catalog key {key:?}",
                path.display()
            );
        }
        assert!(has_translation(crate::cli::QUICK_START));
        assert!(has_translation(crate::pipeline::NO_SCREENSHOTS));
    }

    #[test]
    fn clap_help_source_text_has_catalog_rows() {
        use clap::CommandFactory;
        let mut commands = vec![crate::cli::Cli::command()];
        while let Some(command) = commands.pop() {
            for text in [
                command.get_about(),
                command.get_long_about(),
                command.get_before_help(),
                command.get_after_help(),
            ]
            .into_iter()
            .flatten()
            {
                assert!(
                    has_translation(&text.to_string()),
                    "missing clap text: {text}"
                );
            }
            for argument in command.get_arguments() {
                if argument.get_id() == "lang" {
                    continue;
                } // Its help is formatted from a catalog template.
                for text in [argument.get_help(), argument.get_long_help()]
                    .into_iter()
                    .flatten()
                {
                    assert!(
                        has_translation(&text.to_string()),
                        "missing clap text: {text}"
                    );
                }
                if let Some(heading) = argument.get_help_heading() {
                    assert!(has_translation(heading), "missing clap heading: {heading}");
                }
            }
            commands.extend(command.get_subcommands().cloned());
        }
    }

    #[test]
    fn translated_templates_keep_their_placeholders() {
        let catalog = catalog();
        // Raw Rust diagnostics retain their own formatting; check tr!/t/template lookups.
        for (_, key) in lookup_literals() {
            if let Some(fields) = catalog.entries.get(key.as_str()) {
                for text in fields.iter().filter(|text| !text.is_empty()) {
                    assert_eq!(
                        text.matches("{}").count(),
                        key.matches("{}").count(),
                        "placeholder count differs: {key}"
                    );
                }
            }
        }
    }

    #[test]
    fn known_cli_messages_have_translations() {
        for source in [
            "Error: {}",
            "Missing video source. Example: notepipe ./lecture.mp4",
            "Speech transcription failed for segment {}.",
        ] {
            assert!(has_translation(source), "missing catalog entry: {source}");
        }
    }

    #[test]
    fn escaping_round_trips() {
        let value = "First line\nSecond line\tTab \\ \"quotes\"";
        assert_eq!(unescape(&escape(value)), value);
    }
}
