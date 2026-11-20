use super::UiLocale;

macro_rules! native_texts {
    ($($key:ident => ($english:expr, $polish:expr),)+) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[cfg_attr(not(all(target_os = "macos", has_swift_glass)), allow(dead_code))]
        pub enum NativeText {
            $($key,)+
        }

        impl NativeText {
            #[cfg(test)]
            pub const ALL: &'static [NativeText] = &[$(NativeText::$key,)+];

            pub fn in_locale(self, locale: UiLocale) -> &'static str {
                match (self, locale) {
                    $(
                        (NativeText::$key, UiLocale::English) => $english,
                        (NativeText::$key, UiLocale::Polish) => $polish,
                    )+
                }
            }
        }
    };
}

native_texts! {
    TrayOpenControlPanel => ("Open Control Panel", "Otwórz pasek sterowania"),
    TrayOpenSettings => ("Open Settings", "Otwórz ustawienia"),
    TrayQuit => ("Quit StageWhisper", "Zamknij StageWhisper"),

    SessionStartFailedTitle => ("Couldn't start the session", "Nie udało się rozpocząć sesji"),
    EngineNotSet => (
        "Connect your AI assistant or set up a local model in Settings before starting a session.",
        "Połącz asystenta AI albo skonfiguruj model lokalny w Ustawieniach, zanim zaczniesz sesję."
    ),
    PairingPending => (
        "Your assistant hasn't approved this device yet. Finish pairing in Connection settings before starting a session.",
        "Twój asystent nie zatwierdził jeszcze tego urządzenia. Dokończ parowanie w ustawieniach połączenia, zanim zaczniesz sesję."
    ),
    TranscriptionModelsMissing => (
        "Transcription models are required. Download them in Settings → Model before starting.",
        "Potrzebne są modele transkrypcji. Pobierz je w Ustawieniach → Model, zanim zaczniesz."
    ),
    SystemAudioDenied => (
        "macOS is blocking StageWhisper from hearing your call. Allow it in System Settings > Privacy & Security > Screen & System Audio Recording, then try again.",
        "macOS nie pozwala StageWhisper słyszeć rozmowy. Zezwól na to w Ustawieniach systemowych > Prywatność i ochrona > Nagrywanie ekranu i dźwięku systemowego, a potem spróbuj ponownie."
    ),

    UpdateAvailableTitle => ("Update available", "Dostępna aktualizacja"),
    UpdateAvailableMessage => (
        "StageWhisper {version} is available. Install it and restart now?",
        "Dostępna jest wersja StageWhisper {version}. Zainstalować ją i uruchomić ponownie?"
    ),
    UpdateInstall => ("Install", "Zainstaluj"),
    UpdateLater => ("Later", "Później"),

    SessionWindowTitle => ("Session", "Sesja"),

    BarListen => ("Listen", "Słuchaj"),
    BarStop => ("Stop", "Zakończ"),
    BarConnectAi => ("Connect AI", "Połącz AI"),
    BarOnOffScreen => ("On/Off-screen", "Widoczny/ukryty"),
}
