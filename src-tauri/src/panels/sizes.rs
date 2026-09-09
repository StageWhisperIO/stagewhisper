use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tauri::{AppHandle, LogicalSize, LogicalUnit, Manager, PixelUnit, WindowSizeConstraints};

const SIZES_FILE: &str = "panel_sizes.json";
const WRITE_DEBOUNCE: Duration = Duration::from_millis(600);

type StoredSizes = HashMap<String, (f64, f64)>;

static SIZES: OnceLock<Mutex<StoredSizes>> = OnceLock::new();
static WRITE_PENDING: OnceLock<Mutex<bool>> = OnceLock::new();

pub struct PanelConstraints {
    pub min_width: f64,
    pub min_height: f64,
    pub max_height: Option<f64>,
}

fn sizes_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join(SIZES_FILE))
}

fn sizes(app: &AppHandle) -> &'static Mutex<StoredSizes> {
    SIZES.get_or_init(|| {
        let stored = sizes_path(app)
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|raw| serde_json::from_str::<StoredSizes>(&raw).ok())
            .unwrap_or_default();
        Mutex::new(stored)
    })
}

fn logical(value: f64) -> PixelUnit {
    PixelUnit::Logical(LogicalUnit(value))
}

pub fn apply_saved_size(app: &AppHandle, label: &str, constraints: &PanelConstraints) {
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    let applied = window.set_size_constraints(WindowSizeConstraints {
        min_width: Some(logical(constraints.min_width)),
        min_height: Some(logical(constraints.min_height)),
        max_width: None,
        max_height: constraints.max_height.map(logical),
    });
    if let Err(err) = applied {
        eprintln!("[panels] could not constrain the size of '{label}': {err}");
    }
    let Some((width, height)) = sizes(app).lock().unwrap().get(label).copied() else {
        return;
    };
    let restored = LogicalSize::new(
        width.max(constraints.min_width),
        constraints
            .max_height
            .unwrap_or(f64::INFINITY)
            .min(height.max(constraints.min_height)),
    );
    if let Err(err) = window.set_size(restored) {
        eprintln!("[panels] could not restore the saved size for '{label}': {err}");
    }
}

pub fn remember_size_on_resize(app: &AppHandle, label: &str) {
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    let handle = app.clone();
    let tracked = label.to_string();
    let measured = window.clone();
    window.on_window_event(move |event| {
        let tauri::WindowEvent::Resized(size) = event else {
            return;
        };
        if size.width == 0 || size.height == 0 {
            return;
        }
        let logical = size.to_logical::<f64>(measured.scale_factor().unwrap_or(1.0));
        let changed = {
            let mut stored = sizes(&handle).lock().unwrap();
            stored.insert(tracked.clone(), (logical.width, logical.height))
                != Some((logical.width, logical.height))
        };
        if changed {
            schedule_write(&handle);
        }
    });
}

fn schedule_write(app: &AppHandle) {
    let pending = WRITE_PENDING.get_or_init(|| Mutex::new(false));
    {
        let mut in_flight = pending.lock().unwrap();
        if *in_flight {
            return;
        }
        *in_flight = true;
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(WRITE_DEBOUNCE);
        *WRITE_PENDING.get().unwrap().lock().unwrap() = false;
        write_sizes(&handle);
    });
}

fn write_sizes(app: &AppHandle) {
    let Some(path) = sizes_path(app) else {
        return;
    };
    let snapshot = sizes(app).lock().unwrap().clone();
    let Ok(serialized) = serde_json::to_string_pretty(&snapshot) else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            eprintln!("[panels] could not create {}: {err}", parent.display());
            return;
        }
    }
    if let Err(err) = fs::write(&path, serialized) {
        eprintln!(
            "[panels] could not save panel sizes to {}: {err}",
            path.display()
        );
    }
}
