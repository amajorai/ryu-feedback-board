//! The language the public visitor page renders in.
//!
//! Visitors are not Ryu users and have no Ryu host to translate through, so
//! the page follows the browser's `Accept-Language`, then the node owner's
//! locale (`LC_ALL`, `LC_MESSAGES`, `LANG`), then English: the same order as
//! the Forms and Bot Flows respondent pages
//! (`apps-store/bot-flows/sidecar/src/respondent-locale.ts`). Translations come
//! from `public-catalog.generated.json`, the official catalog entries for the
//! page's `tl()` copy, written by `scripts/sync-feedback-board-catalog.mjs`
//! (the backend is a standalone satellite, so it carries its own slice). Only
//! a language with a slice is chosen; a sentence the slice lacks stays English.
//!
//! The shell gets `<html lang dir>`, a translated `<title>`, and the chosen
//! slice as JSON in `<script id="feedback-board-i18n">`, which the page's
//! `ui/src/public/public-locale.ts` hands to its i18n runtime.

use std::collections::BTreeMap;
use std::sync::OnceLock;

type Messages = BTreeMap<String, String>;

const CATALOG_JSON: &str = include_str!("public-catalog.generated.json");
const RTL_LANGUAGES: [&str; 4] = ["ar", "fa", "he", "ur"];
const MAX_LANGUAGE_RANGES: usize = 20;
/// The element id `ui/src/public/public-locale.ts` reads.
pub const I18N_ELEMENT_ID: &str = "feedback-board-i18n";
const SHELL_TITLE: &str = "<title>Feedback</title>";

fn catalogs() -> &'static BTreeMap<String, Messages> {
    static CATALOGS: OnceLock<BTreeMap<String, Messages>> = OnceLock::new();
    CATALOGS.get_or_init(|| serde_json::from_str(CATALOG_JSON).unwrap_or_default())
}

/// The negotiated page language and its catalog slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicLocale {
    /// BCP 47 tag for `<html lang>` (`en` when no catalog matches).
    pub locale: String,
    /// Text direction for `<html dir>`.
    pub dir: &'static str,
    messages: Option<&'static Messages>,
}

impl PublicLocale {
    fn english() -> Self {
        Self {
            locale: "en".to_owned(),
            dir: "ltr",
            messages: None,
        }
    }

    fn for_catalog(locale: &str, messages: &'static Messages) -> Self {
        let language = language_of(locale);
        Self {
            locale: locale.to_owned(),
            dir: if RTL_LANGUAGES.contains(&language.as_str()) {
                "rtl"
            } else {
                "ltr"
            },
            messages: Some(messages),
        }
    }

    /// `source` in this language, or the English source when untranslated.
    pub fn tl<'a>(&'a self, source: &'a str) -> &'a str {
        self.messages
            .and_then(|messages| messages.get(source))
            .map_or(source, String::as_str)
    }
}

fn language_of(tag: &str) -> String {
    tag.split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// The script a tag writes in, as far as the shipped catalogs distinguish:
/// an explicit script subtag, else Traditional Chinese for TW/HK/MO, else the
/// language's default (empty).
fn script_of(tag: &str) -> String {
    let mut subtags = tag.split(['-', '_']).skip(1);
    let language = language_of(tag);
    let mut region = String::new();
    for subtag in subtags.by_ref() {
        if subtag.len() == 4 && subtag.chars().all(|c| c.is_ascii_alphabetic()) {
            let script = subtag.to_ascii_lowercase();
            return if language == "zh" && script == "hans" {
                String::new()
            } else {
                script
            };
        }
        if region.is_empty() && (subtag.len() == 2 || subtag.len() == 3) {
            region = subtag.to_ascii_uppercase();
        }
    }
    if language == "zh" && matches!(region.as_str(), "TW" | "HK" | "MO") {
        "hant".to_owned()
    } else {
        String::new()
    }
}

/// The catalog key whose language and script match `tag`, `en`, or `None`.
fn catalog_locale_for(tag: &str) -> Option<PublicLocale> {
    let catalogs = catalogs();
    if let Some((key, messages)) = catalogs
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(tag))
    {
        return Some(PublicLocale::for_catalog(key, messages));
    }
    let language = language_of(tag);
    if language.is_empty() {
        return None;
    }
    if language == "en" {
        return Some(PublicLocale::english());
    }
    let script = script_of(tag);
    catalogs
        .iter()
        .find(|(key, _)| language_of(key) == language && script_of(key) == script)
        .map(|(key, messages)| PublicLocale::for_catalog(key, messages))
}

/// `Accept-Language` ranges, most preferred first (`*` and `q=0` dropped).
pub fn accepted_languages(header: Option<&str>) -> Vec<String> {
    let mut ranges: Vec<(f64, usize, String)> = Vec::new();
    for (order, part) in header
        .unwrap_or_default()
        .split(',')
        .take(MAX_LANGUAGE_RANGES)
        .enumerate()
    {
        let mut pieces = part.split(';');
        let tag = pieces.next().unwrap_or_default().trim();
        let q = pieces
            .find_map(|param| param.trim().strip_prefix("q="))
            .map_or(Some(1.0), |value| value.trim().parse::<f64>().ok());
        if let Some(q) = q {
            if !tag.is_empty() && tag != "*" && q.is_finite() && q > 0.0 {
                ranges.push((q, order, tag.to_owned()));
            }
        }
    }
    ranges.sort_by(|left, right| right.0.total_cmp(&left.0).then(left.1.cmp(&right.1)));
    ranges.into_iter().map(|(_, _, tag)| tag).collect()
}

/// A POSIX locale (`de_DE.UTF-8`) as a BCP 47 tag, or `None` for `C`/`POSIX`.
fn posix_tag(value: Option<String>) -> Option<String> {
    let value = value?;
    let tag = value.split(['.', '@']).next()?.replace('_', "-");
    let tag = tag.trim();
    (!tag.is_empty() && tag != "C" && tag != "POSIX").then(|| tag.to_owned())
}

/// The node owner's locale from the process environment.
pub fn owner_locale_from_env() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| posix_tag(std::env::var(name).ok()))
}

/// The first `Accept-Language` range with a catalog (an English range wins
/// outright), else the owner's locale, else English.
pub fn negotiate(accept_language: Option<&str>, owner_locale: Option<&str>) -> PublicLocale {
    accepted_languages(accept_language)
        .iter()
        .find_map(|tag| catalog_locale_for(tag))
        .or_else(|| owner_locale.and_then(catalog_locale_for))
        .unwrap_or_else(PublicLocale::english)
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// JSON safe inside a `<script>` element.
fn script_json(value: &serde_json::Value) -> String {
    value
        .to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

/// The shell in `locale`: `<html lang dir>`, a translated title, and the
/// catalog slice. Only the document prologue before the first `<script` is
/// rewritten, so bundled code that happens to contain these strings is left
/// alone.
pub fn localize_shell(html: &str, locale: &PublicLocale) -> String {
    let split = html.find("<script").unwrap_or(html.len());
    let (prologue, rest) = html.split_at(split);
    let payload = serde_json::json!({
        "locale": locale.locale,
        "messages": locale.messages.cloned().unwrap_or_default(),
    });
    let data = format!(
        "<script type=\"application/json\" id=\"{I18N_ELEMENT_ID}\">{}</script>",
        script_json(&payload)
    );
    let prologue = prologue
        .replacen(
            "<html lang=\"en\">",
            &format!(
                "<html lang=\"{}\" dir=\"{}\">",
                escape_html(&locale.locale),
                locale.dir
            ),
            1,
        )
        .replacen(
            SHELL_TITLE,
            &format!("<title>{}</title>", escape_html(locale.tl("Feedback"))),
            1,
        );
    // The slice goes after the prologue's `<meta charset>` (which must stay in
    // the first 1024 bytes) and before the page script that reads it.
    if rest.is_empty() {
        return prologue.replacen("</head>", &format!("{data}</head>"), 1);
    }
    format!("{prologue}{data}{rest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHELL: &str = "<!doctype html>\n<html lang=\"en\">\n<head>\n<title>Feedback</title>\n<script type=\"module\">const x = \"<html lang=\\\"en\\\">\";</script>\n</head><body></body></html>";

    #[test]
    fn accept_language_wins_then_the_owner_locale_then_english() {
        assert_eq!(
            negotiate(Some("ja-JP,ja;q=0.9,en;q=0.8"), None).locale,
            "ja"
        );
        assert_eq!(
            negotiate(Some("en-US,de;q=0.9"), Some("de_DE.UTF-8")).locale,
            "en"
        );
        assert_eq!(negotiate(Some("xx-YY"), Some("de-DE")).locale, "de");
        assert_eq!(negotiate(None, Some("C")).locale, "en");
        assert_eq!(negotiate(Some("de;q=0, fr;q=0.5, ko"), None).locale, "ko");
    }

    #[test]
    fn regional_and_script_variants_match_the_shipped_catalog() {
        assert_eq!(negotiate(Some("pt-PT"), None).locale, "pt-BR");
        assert_eq!(negotiate(Some("zh"), None).locale, "zh-CN");
        assert_eq!(negotiate(Some("zh-Hans-SG"), None).locale, "zh-CN");
        // No Traditional Chinese catalog ships, so the owner's locale decides.
        assert_eq!(negotiate(Some("zh-TW"), None).locale, "en");
        assert_eq!(negotiate(Some("zh-Hant"), Some("fr_FR")).locale, "fr");
    }

    #[test]
    fn owner_locale_parses_posix_values() {
        assert_eq!(
            posix_tag(Some("de_DE.UTF-8".into())).as_deref(),
            Some("de-DE")
        );
        assert_eq!(
            posix_tag(Some("sr_RS@latin".into())).as_deref(),
            Some("sr-RS")
        );
        assert_eq!(posix_tag(Some("POSIX".into())), None);
    }

    #[test]
    fn arabic_visitors_get_an_rtl_translated_shell() {
        let locale = negotiate(Some("ar"), None);
        let html = localize_shell(SHELL, &locale);
        assert!(html.contains("<html lang=\"ar\" dir=\"rtl\">"));
        assert!(!html.contains("<title>Feedback</title>"));
        assert!(html.contains(&format!("<title>{}</title>", locale.tl("Feedback"))));
        assert_ne!(locale.tl("Roadmap"), "Roadmap");
        assert!(html.contains(&format!("id=\"{I18N_ELEMENT_ID}\">{{\"locale\":\"ar\"")));
        // The slice sits after the charset prologue, before the page script.
        let slice_at = html.find(I18N_ELEMENT_ID).unwrap();
        assert!(html.find("<title>").unwrap() < slice_at);
        assert!(slice_at < html.find("<script type=\"module\"").unwrap());
        // The bundled script body is never rewritten.
        assert!(html.contains("const x = \"<html lang=\\\"en\\\">\";"));
    }

    #[test]
    fn english_visitors_keep_the_english_shell_with_an_empty_slice() {
        let html = localize_shell(SHELL, &negotiate(Some("en-GB"), Some("ja_JP")));
        assert!(html.contains("<html lang=\"en\" dir=\"ltr\">"));
        assert!(html.contains("<title>Feedback</title>"));
        assert!(html.contains("{\"locale\":\"en\",\"messages\":{}}"));
    }

    #[test]
    fn a_shell_without_scripts_still_gets_the_slice_in_its_head() {
        let html = localize_shell(
            "<html lang=\"en\"><head><title>Feedback</title></head></html>",
            &negotiate(Some("de"), None),
        );
        assert!(html.starts_with("<html lang=\"de\" dir=\"ltr\"><head><title>"));
        assert!(html.ends_with("</script></head></html>"));
    }

    #[test]
    fn the_embedded_slice_cannot_close_its_script_element() {
        let value = serde_json::json!({ "x": "</script><b>&" });
        let json = script_json(&value);
        assert!(!json.contains('<'));
        assert!(!json.contains('&'));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json).unwrap(),
            value
        );
    }
}
