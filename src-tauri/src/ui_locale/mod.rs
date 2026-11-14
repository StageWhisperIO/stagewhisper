mod catalog;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicU8, Ordering};

use tauri::{AppHandle, Manager};

pub use catalog::NativeText;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiLocale {
    English,
    Polish,
}

impl UiLocale {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "en" => Some(Self::English),
            "pl" => Some(Self::Polish),
            _ => None,
        }
    }

    const fn bits(self) -> u8 {
        match self {
            Self::English => 0,
            Self::Polish => 1,
        }
    }

    const fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::Polish,
            _ => Self::English,
        }
    }
}

pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    values
        .iter()
        .fold(template.to_string(), |text, (name, value)| {
            text.replace(&format!("{{{name}}}"), value)
        })
}

pub struct LocaleCell(AtomicU8);

impl LocaleCell {
    pub const fn new() -> Self {
        Self(AtomicU8::new(UiLocale::English.bits()))
    }

    pub fn get(&self) -> UiLocale {
        UiLocale::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub fn replace(&self, locale: UiLocale) -> bool {
        self.0.swap(locale.bits(), Ordering::Relaxed) != locale.bits()
    }
}

static CURRENT: LocaleCell = LocaleCell::new();

pub fn current() -> UiLocale {
    CURRENT.get()
}

pub fn text(key: NativeText) -> &'static str {
    key.in_locale(current())
}

#[tauri::command]
pub fn set_ui_locale(app: AppHandle, locale: String) {
    let Some(locale) = UiLocale::parse(&locale) else {
        log::warn!("[ui_locale] ignoring unsupported locale {locale:?}");
        return;
    };
    if CURRENT.replace(locale) {
        refresh_native_surfaces(&app);
    }
}

fn refresh_native_surfaces(app: &AppHandle) {
    crate::glass_buttons::sync_labels();
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        #[cfg(target_os = "windows")]
        crate::tray::rebuild_menu(&handle);
        if let Some(session) = handle.get_webview_window(crate::panels::SESSION_LABEL) {
            let _ = session.set_title(text(NativeText::SessionWindowTitle));
        }
    });
}
