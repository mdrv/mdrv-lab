//! Tiny persisted UI settings — a plain `key=value` text file in the data
//! root (no serde dependency). Written on change, read at startup.

use crate::notes::data_root;

fn path() -> std::path::PathBuf {
    data_root().join("settings.txt")
}

fn get(key: &str) -> Option<String> {
    std::fs::read_to_string(path()).ok()?.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().to_string())
    })
}

fn set(key: &str, value: &str) {
    let p = path();
    let _ = std::fs::create_dir_all(p.parent().unwrap_or(&p));
    let prefix = format!("{key}=");
    let mut lines: Vec<String> = std::fs::read_to_string(&p)
        .map(|s| {
            s.lines()
                .filter(|l| !l.starts_with(&prefix))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    lines.push(format!("{key}={value}"));
    let _ = std::fs::write(&p, lines.join("\n") + "\n");
}

/// Global UI zoom (rem multiplier), clamped to the app's slider range.
pub fn load_zoom() -> f32 {
    get("zoom")
        .and_then(|v| v.parse().ok())
        .map(|z: f32| z.clamp(0.6, 2.4))
        .unwrap_or(1.0)
}

pub fn save_zoom(z: f32) {
    set("zoom", &format!("{z:.3}"));
}
