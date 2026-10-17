//! Desktop binding for the shared catalog and the persisted interface language.
//!
//! The core crate owns language resolution and the catalog. This module applies
//! the preference to both the catalog and gpui-component's built-in strings.

use crate::preferences::InterfaceLanguage;
use course2md::i18n::{self, Lang};

/// Applies a stored preference. Call before the first frame and after a change.
pub(crate) fn apply(language: InterfaceLanguage) {
    let lang = language.resolve();
    i18n::set_lang(lang);
    gpui_component::set_locale(component_locale(lang));
}

pub(crate) fn resolve(language: InterfaceLanguage) -> Lang {
    language.resolve()
}

/// gpui-component ships English and Simplified Chinese; other locales fall back
/// to English for its built-in controls, while our catalog covers the app.
fn component_locale(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "en",
        Lang::ZhHans => "zh-CN",
        Lang::ZhHant => "zh-TW",
        Lang::Ja => "ja",
    }
}
