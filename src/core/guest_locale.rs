//! Host-testable first-install language defaults for Plasma.
//!
//! The guest ships every KDE translation but generates only the `en_GB` and `en_US` glibc
//! locales. Messages follow gettext's `LANGUAGE`, which needs no generated locale, so Android's
//! language can translate the desktop; formats stay `en_GB` unless the language is US English.

/// Plasma `plasma-localerc` defaults for an Android language tag (BCP 47, e.g. `pt-BR`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlasmaLocale {
    /// `[Translations] LANGUAGE`, a gettext language such as `pt_BR` or `sr@latin`.
    pub language: String,
    /// `[Formats] LANG`, only when a generated locale other than the default fits.
    pub formats: Option<&'static str>,
}

/// Map Android's primary language to Plasma defaults, or `None` when they match the guest's
/// own (British English) or the tag cannot be read.
pub fn plasma_locale_for(language_tag: &str) -> Option<PlasmaLocale> {
    let mut parts = language_tag.split(['-', '_']);
    let language = parts.next()?.to_ascii_lowercase();
    // "und" is Java's tag for an undetermined locale.
    if language == "und"
        || !(2..=3).contains(&language.len())
        || !language.chars().all(|c| c.is_ascii_alphabetic())
    {
        return None;
    }
    let mut script = None;
    let mut region = None;
    for part in parts {
        if part.len() == 4 && part.chars().all(|c| c.is_ascii_alphabetic()) && script.is_none() {
            script = Some(part.to_ascii_lowercase());
        } else if part.len() == 2 && part.chars().all(|c| c.is_ascii_alphabetic()) && region.is_none() {
            region = Some(part.to_ascii_uppercase());
        }
    }

    let gettext = match (language.as_str(), script.as_deref(), region.as_deref()) {
        // KDE's Chinese catalogs are zh_CN (Simplified) and zh_TW (Traditional).
        ("zh", Some("hant"), _) | ("zh", None, Some("TW" | "HK" | "MO")) => "zh_TW".to_string(),
        ("zh", _, _) => "zh_CN".to_string(),
        ("sr", Some("latn"), _) => "sr@latin".to_string(),
        (_, _, Some(region)) => format!("{language}_{region}"),
        (_, _, None) => language.clone(),
    };
    let formats = (gettext == "en_US").then_some("en_US.UTF-8");
    if gettext == "en_GB" || (gettext == "en" && formats.is_none()) {
        return None;
    }
    Some(PlasmaLocale {
        language: gettext,
        formats,
    })
}

/// `plasma-localerc` contents for [`PlasmaLocale`].
pub fn plasma_localerc(locale: &PlasmaLocale) -> String {
    let mut contents = String::new();
    if let Some(formats) = locale.formats {
        contents.push_str(&format!("[Formats]\nLANG={formats}\n\n"));
    }
    contents.push_str(&format!("[Translations]\nLANGUAGE={}\n", locale.language));
    contents
}

#[cfg(test)]
mod tests {
    use super::*;

    fn language(tag: &str) -> Option<String> {
        plasma_locale_for(tag).map(|locale| locale.language)
    }

    #[test]
    fn regional_tags_become_gettext_languages() {
        assert_eq!(language("de-DE").as_deref(), Some("de_DE"));
        assert_eq!(language("pt-BR").as_deref(), Some("pt_BR"));
        assert_eq!(language("fr").as_deref(), Some("fr"));
        assert_eq!(language("it-IT").as_deref(), Some("it_IT"));
    }

    #[test]
    fn scripts_pick_the_catalogs_kde_ships() {
        assert_eq!(language("zh-Hans-CN").as_deref(), Some("zh_CN"));
        assert_eq!(language("zh-Hant-TW").as_deref(), Some("zh_TW"));
        assert_eq!(language("zh-HK").as_deref(), Some("zh_TW"));
        assert_eq!(language("zh").as_deref(), Some("zh_CN"));
        assert_eq!(language("sr-Latn-RS").as_deref(), Some("sr@latin"));
        assert_eq!(language("sr-RS").as_deref(), Some("sr_RS"));
    }

    #[test]
    fn british_english_keeps_the_guest_defaults() {
        assert_eq!(plasma_locale_for("en-GB"), None);
        assert_eq!(plasma_locale_for("en"), None);
    }

    #[test]
    fn us_english_also_gets_us_formats() {
        let locale = plasma_locale_for("en-US").unwrap();
        assert_eq!(locale.formats, Some("en_US.UTF-8"));
        assert_eq!(
            plasma_localerc(&locale),
            "[Formats]\nLANG=en_US.UTF-8\n\n[Translations]\nLANGUAGE=en_US\n"
        );
        assert_eq!(plasma_locale_for("de-DE").unwrap().formats, None);
    }

    #[test]
    fn unreadable_tags_are_ignored() {
        assert_eq!(plasma_locale_for(""), None);
        assert_eq!(plasma_locale_for("und"), None);
        assert_eq!(plasma_locale_for("1234"), None);
    }
}
