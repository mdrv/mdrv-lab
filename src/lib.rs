//! mdrv-lab — GPUI-on-Android lab app.
//!
//! Six screens exercising the pieces a real app needs on Android:
//! - **Notes** — mdrv-db (Fjall envelope + Turso) CRUD: journal/fsync under
//!   app-private storage.
//! - **Gallery** — a bundled image with drag-pan / wheel-zoom / rotate:
//!   input events + the img pipeline (plus two-finger pinch/rotate on
//!   Android via the fork's gesture tracker).
//! - **Pics** — upload images into mdrv-db blobs (photo picker on Android),
//!   animated Ken Burns view, big-picture notifications.
//! - **Sys** — sound (rodio), TTS read-aloud, haptics, keep-screen-on,
//!   immersive fullscreen.
//! - **Orbit** — Duck.glb rendered with our own wgpu pipeline on the
//!   platform renderer's shared device, composited via `paint_surface`:
//!   GPU throughput + fork surface path.
//! - **Touch** — multi-touch visualiser: every finger draws a circle +
//!   crosshair via the fork's raw pointer feed (`gesture::pointers()`).
//!
//! Entries:
//! - Desktop (dev): `main.rs` behind the `desktop` feature → gpui_platform
//!   (Wayland).
//! - Android: `android_main` in this lib → mdrv-gpui-mobile
//!   (`Application::with_platform`); the run closure fires once the system
//!   delivers a native surface (MainEvent::InitWindow).

pub mod app;
pub mod audio;
pub mod gallery;
pub mod notes;
pub mod orbit;
pub mod pics;
pub mod platform;
pub mod settings;
pub mod sys;
pub mod touch;

use gpui::AppContext;

/// Entry-point shared by both platforms: open the root window and run.
pub fn start(cx: &mut gpui::App) {
    let bounds = gpui::Bounds {
        origin: gpui::Point::default(),
        size: gpui::size(gpui::px(1080.), gpui::px(1920.)),
    };
    cx.open_window(
        gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: true,
            show: true,
            kind: gpui::WindowKind::Normal,
            ..Default::default()
        },
        |_, cx| cx.new(app::LabApp::new),
    )
    .expect("open root window");
}

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: android_activity::AndroidApp) {
    // File logging: logcat is unavailable on the test device, so everything
    // goes to <internal-data>/mdrv-lab.log (readable via `run-as id.mdrv.lab`).
    let log_path = app
        .internal_data_path()
        .map(|p| std::path::PathBuf::from(p).join("mdrv-lab.log"));
    fn log_line(path: &Option<std::path::PathBuf>, msg: &str) {
        use std::fmt::Write as _;
        let Some(path) = path else { return };
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let mut line = String::new();
        let _ = write!(line, "[{stamp}] {msg}\n");
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
    }
    if let Some(path) = &log_path {
        let _ = std::fs::write(path, b"");
    }
    log_line(&log_path, "android_main entered");
    let panic_log = log_path.clone();
    std::panic::set_hook(Box::new(move |info| {
        log_line(&panic_log, &format!("PANIC: {info}"));
    }));

    // Capture the app-private data dir before the platform starts; notes.rs
    // reads it once at first DB open.
    let data_dir = app.internal_data_path().map(std::path::PathBuf::from);
    notes::set_android_data_dir(data_dir);
    log_line(&log_path, "data dir captured");

    // Route the `log` crate (fork frame-time instrumentation, renderer
    // diagnostics) into the same file log — logcat is unavailable on this
    // ROM, so the file is the only sink.
    if let Some(path) = &log_path {
        gpui_mobile::android::set_log_file(path.clone());
    }
    gpui_mobile::android::init_logger();

    use gpui_mobile::android::jni as platform_jni;
    // Initialise the global AndroidApp + AndroidPlatform (the example does
    // this first; without it shared_platform() returns None).
    platform_jni::init_platform(&app);
    log_line(&log_path, "init_platform done");
    let shared = platform_jni::shared_platform();
    log_line(
        &log_path,
        &format!("shared_platform() -> {}", shared.is_some()),
    );
    let shared = shared
        .unwrap_or_else(|| panic!("mdrv-lab: platform not initialised by ANativeActivity_onCreate"));

    log_line(&log_path, "starting Application::with_platform");
    let run_log = log_path.clone();
    gpui::Application::with_platform(shared.into_rc()).run(move |cx| {
        log_line(&run_log, "run closure: opening window");
        start(cx);
        log_line(&run_log, "run closure: window opened");
    });
    log_line(&log_path, "Application.run returned");
}
