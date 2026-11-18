use crate::ui_locale::{NativeText, UiLocale};

pub const BAR_LABELS: &[(&str, NativeText)] = &[
    ("listen.listen", NativeText::BarListen),
    ("listen.stop", NativeText::BarStop),
    ("setup.connectAi", NativeText::BarConnectAi),
    ("shortcut.onOffScreen", NativeText::BarOnOffScreen),
];

pub fn labels_json(locale: UiLocale) -> String {
    let labels: serde_json::Map<String, serde_json::Value> = BAR_LABELS
        .iter()
        .map(|(key, text)| (key.to_string(), text.in_locale(locale).into()))
        .collect();
    serde_json::Value::Object(labels).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWIFT_SOURCE: &str = include_str!("../../swift/GlassControlBar.swift");

    fn keys_used_by_swift() -> Vec<String> {
        SWIFT_SOURCE
            .split("barText(\"")
            .skip(1)
            .filter_map(|rest| rest.split_once('"').map(|(key, _)| key.to_string()))
            .collect()
    }

    #[test]
    fn every_label_the_swift_bar_asks_for_is_pushed_from_rust() {
        for key in keys_used_by_swift() {
            assert!(
                BAR_LABELS.iter().any(|(known, _)| *known == key),
                "the Swift bar reads {key:?}, which Rust never sends"
            );
        }
    }

    #[test]
    fn every_pushed_label_is_used_by_the_swift_bar() {
        let keys = keys_used_by_swift();
        for (key, _) in BAR_LABELS {
            assert!(keys.iter().any(|used| used == key), "{key:?} is unused");
        }
    }

    #[test]
    fn the_label_payload_is_a_flat_map_in_the_requested_locale() {
        let payload: std::collections::HashMap<String, String> =
            serde_json::from_str(&labels_json(UiLocale::Polish)).unwrap();
        assert_eq!(payload.len(), BAR_LABELS.len());
        assert_eq!(payload["listen.stop"], "Zakończ");
        assert_eq!(payload["setup.connectAi"], "Połącz AI");
    }
}
