//! Desktop (dev) entry — Wayland via mdrv-gpui-platform. Android enters via
//! `android_main` in lib.rs; this binary is never called there.

fn main() {
    gpui_platform::application().run(mdrv_lab::start);
}
