use std::collections::BTreeSet;

use super::*;

fn placeholders(text: &str) -> BTreeSet<String> {
    text.split('{')
        .skip(1)
        .filter_map(|rest| rest.split_once('}').map(|(name, _)| name.to_string()))
        .collect()
}

#[test]
fn only_english_and_polish_codes_are_accepted() {
    assert_eq!(UiLocale::parse("en"), Some(UiLocale::English));
    assert_eq!(UiLocale::parse("pl"), Some(UiLocale::Polish));
    assert_eq!(UiLocale::parse(" pl "), Some(UiLocale::Polish));
    for rejected in ["", "de", "PL", "pl-PL", "en-US", "polish"] {
        assert_eq!(UiLocale::parse(rejected), None, "{rejected:?}");
    }
}

#[test]
fn a_fresh_locale_cell_is_english_and_reports_only_real_changes() {
    let cell = LocaleCell::new();
    assert_eq!(cell.get(), UiLocale::English);
    assert!(!cell.replace(UiLocale::English));
    assert!(cell.replace(UiLocale::Polish));
    assert_eq!(cell.get(), UiLocale::Polish);
    assert!(!cell.replace(UiLocale::Polish));
    assert!(cell.replace(UiLocale::English));
}

#[test]
fn every_text_is_present_in_both_locales_with_the_same_placeholders() {
    for key in NativeText::ALL {
        let english = key.in_locale(UiLocale::English);
        let polish = key.in_locale(UiLocale::Polish);
        assert!(!english.trim().is_empty(), "{key:?} has no English text");
        assert!(!polish.trim().is_empty(), "{key:?} has no Polish text");
        assert_eq!(placeholders(english), placeholders(polish), "{key:?}");
    }
}

#[test]
fn lookup_returns_the_text_for_the_requested_locale() {
    assert_eq!(
        NativeText::TrayQuit.in_locale(UiLocale::English),
        "Quit StageWhisper"
    );
    assert_eq!(
        NativeText::TrayQuit.in_locale(UiLocale::Polish),
        "Zamknij StageWhisper"
    );
}

#[test]
fn fill_replaces_every_named_placeholder() {
    assert_eq!(
        fill("{title}. Open {title}", &[("title", "Demo")]),
        "Demo. Open Demo"
    );
    assert_eq!(
        fill("in {minutes} min", &[("other", "1")]),
        "in {minutes} min"
    );
}
