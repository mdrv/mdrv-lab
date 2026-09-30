//! mdrv-lab — GPUI-on-Android lab app.
//!
//! Three screens exercising the pieces a real app needs on Android:
//! - **Notes** — mdrv-db (Fjall envelope + Turso) CRUD: journal/fsync under
//!   app-private storage.
//! - **Gallery** — a bundled image with drag-pan / wheel-zoom / rotate:
//!   input events + the img pipeline.
//! - **Orbit** — Duck.glb rendered with our own wgpu pipeline on the
//!   platform renderer's shared device, composited via `paint_surface`:
//!   GPU throughput + fork surface path.
//!
//! Entries:
//! - Desktop (dev): `main.rs` behind the `desktop` feature → gpui_platform
//!   (Wayland).
//! - Android: `android_main` in this lib → mdrv-gpui-mobile
//!   (`Application::with_platform`); the run closure fires once the system
//!   delivers a native surface (MainEvent::InitWindow).

pub mod app;
pub mod gallery;
pub mod notes;
pub mod orbit;

/// Entry-point shared by both platforms: open the root window and run.
pub fn start(cx: &mut gpui::App) {
    app::init(cx);
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
    // Capture the app-private data dir before the platform starts; notes.rs
    // reads it once at first DB open.
    let data_dir = app.internal_data_path().map(std::path::PathBuf::from);
    notes::set_android_data_dir(data_dir);

    use mdrv_gpui_mobile::android::jni as platform_jni;
    let shared = platform_jni::shared_platform()
        .unwrap_or_else(|| panic!("mdrv-lab: platform not initialised by ANativeActivity_onCreate"));

    gpui::Application::with_platform(shared.into_rc()).run(|cx| start(cx));
}
