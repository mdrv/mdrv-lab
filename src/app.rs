//! Root view — screen tabs, global zoom slider (rem scaling, persisted),
//! and the FPS HUD.

use gpui::prelude::*;
use gpui::{
    canvas, div, px, rems, AnyElement, Bounds, Context, Entity, IntoElement, ParentElement,
    Pixels, Render, Styled, Window,
};

use crate::{
    gallery::GalleryScreen, notes::NotesScreen, orbit::OrbitScreen, pics::PicsScreen, settings,
    sys::SysScreen, touch::TouchScreen,
};

/// Global UI zoom range (multiplier on the 16 px rem).
pub const MIN_ZOOM: f32 = 0.6;
pub const MAX_ZOOM: f32 = 2.4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Notes,
    Gallery,
    Pics,
    Orbit,
    Sys,
    Touch,
}

pub struct LabApp {
    screen: Screen,
    frames: u32,
    fps: u32,
    /// Paint diagnostics line (fork draw total + renderer perf snapshot).
    draw_ms: String,
    zoom: f32,
    zoom_drag: bool,
    /// Debounce flag for persisting `zoom` while a touch drag moves it.
    zoom_save_pending: bool,
    /// Track bounds captured at layout time (shared into listeners).
    zoom_bounds: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    notes: Option<Entity<NotesScreen>>,
    gallery: Option<Entity<GalleryScreen>>,
    pics: Option<Entity<PicsScreen>>,
    orbit: Option<Entity<OrbitScreen>>,
    sys: Option<Entity<SysScreen>>,
    touch: Option<Entity<TouchScreen>>,
}

#[cfg(target_os = "android")]
fn last_draw_ms() -> String {
    let total = gpui_mobile::android::window::last_draw_stats().0;
    use gpui_wgpu::perf;
    use std::sync::atomic::Ordering;
    let (q, pv, sp, ot, a, r, p, f) = perf::snapshot();
    static PREV_FRAMES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dps = f.saturating_sub(PREV_FRAMES.swap(f, Ordering::Relaxed));
    let mut nm = String::new();
    for w in [
        perf::NAME0.load(Ordering::Relaxed),
        perf::NAME1.load(Ordering::Relaxed),
        perf::NAME2.load(Ordering::Relaxed),
        perf::NAME3.load(Ordering::Relaxed),
    ] {
        nm.extend(w.to_le_bytes().into_iter().filter(|&b| b != 0).map(|b| b as char));
    }
    format!(
        "{total}ms T{} g{} m{} d{dps} | a{a} r{r} p{p} q{q} sp{sp} P{} b{}:{nm}",
        perf::FRAME_PERIOD_MS.load(Ordering::Relaxed),
        perf::GPU_WAIT_MS.load(Ordering::Relaxed),
        perf::PRESENT_MODE.load(Ordering::Relaxed),
        perf::PASS_COUNT.load(Ordering::Relaxed),
        perf::BACKEND.load(Ordering::Relaxed),
    )
}

#[cfg(not(target_os = "android"))]
fn last_draw_ms() -> String {
    String::new()
}

impl LabApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // 1-second FPS counter.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1000))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.fps = std::mem::replace(&mut this.frames, 0);
                        this.draw_ms = last_draw_ms();
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();

        Self {
            screen: Screen::Notes,
            frames: 0,
            fps: 0,
            draw_ms: String::new(),
            zoom: settings::load_zoom(),
            zoom_drag: false,
            zoom_save_pending: false,
            zoom_bounds: Default::default(),
            notes: None,
            gallery: None,
            pics: None,
            orbit: None,
            sys: None,
            touch: None,
        }
    }

    fn tab(
        &self,
        label: &'static str,
        screen: Screen,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.screen == screen;
        div()
            .id(label)
            .px(rems(0.19))
            .py(rems(0.09))
            .rounded(px(6.))
            .cursor_pointer()
            .text_size(rems(0.875))
            .when(active, |d| d.bg(gpui::rgba(0x3b82f64d)))
            .text_color(if active {
                gpui::rgb(0xe9e9ee)
            } else {
                gpui::rgb(0x8b8b93)
            })
            .child(label)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                    this.screen = screen;
                    cx.notify();
                }),
            )
    }

    /// Zoom slider: track + knob; drag anywhere on the track. The move/up
    /// handlers live on the root div so the drag continues outside the track.
    fn zoom_slider(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let track_w: f32 = 110.;
        let knob: f32 = 12.;
        let frac = ((self.zoom - MIN_ZOOM) / (MAX_ZOOM - MIN_ZOOM)).clamp(0., 1.);

        div()
            .flex()
            .flex_row()
            .items_center()
            .child(
                div()
                    .mr(px(4.))
                    .text_size(rems(0.75))
                    .text_color(gpui::rgb(0x8b8b93))
                    .child(format!("{:.0}%", self.zoom * 100.)),
            )
            .child(
                div()
                    .id("zoom-track")
                    .relative()
                    .mr(px(4.))
                    .w(px(track_w))
                    // 32 px tall hit area: finger drift must not leave the
                    // track mid-drag (the visual track is drawn inside it).
                    .h(px(32.))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener({
                            let cell = self.zoom_bounds.clone();
                            move |this, e: &gpui::MouseDownEvent, _, cx| {
                                this.zoom_drag = true;
                                apply_zoom(this, e.position, cell.get(), cx);
                            }
                        }),
                    )
                    // Touch drags never produce MouseDown (the platform touch
                    // layer emits pressed MouseMoves only), so pressed-move
                    // inside the track drives the zoom directly.
                    .on_mouse_move(cx.listener({
                        let cell = self.zoom_bounds.clone();
                        move |this, e: &gpui::MouseMoveEvent, _, cx| {
                            if e.pressed_button == Some(gpui::MouseButton::Left) {
                                apply_zoom(this, e.position, cell.get(), cx);
                                zoom_save_debounced(this, cx);
                            }
                        }
                    }))
                    // Visual track, vertically centered in the hit area.
                    .child(
                        div()
                            .absolute()
                            .top(px(8.))
                            .w(px(track_w))
                            .h(px(16.))
                            .rounded(px(4.))
                            .bg(gpui::rgba(0x2c2c34a0)),
                    )
                    // Knob indicator (pure visual).
                    .child(
                        div()
                            .absolute()
                            .top(px(10.))
                            .left(px(frac * (track_w - knob)))
                            .w(px(knob))
                            .h(px(12.))
                            .rounded(px(3.))
                            .bg(gpui::rgb(0x8b5cf6)),
                    )
                    // Capture the track's layout bounds for pointer→zoom math.
                    .child(
                        canvas(
                            {
                                let cell = self.zoom_bounds.clone();
                                move |b: Bounds<Pixels>, _, _| cell.set(Some(b))
                            },
                            {
                                let cell = self.zoom_bounds.clone();
                                move |b: Bounds<Pixels>, _, _, _| cell.set(Some(b))
                            },
                        )
                        .absolute()
                        .inset_0(),
                    ),
            )
    }
}

/// Persist zoom changes at most once per 800 ms while dragging (touch drags
/// never produce a MouseUp on Android — the layer ends drags as wheel flings
/// — so save-on-release alone would lose them).
fn zoom_save_debounced(this: &mut LabApp, cx: &mut Context<LabApp>) {
    if this.zoom_save_pending {
        return;
    }
    this.zoom_save_pending = true;
    cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(800))
            .await;
        let _ = this.update(cx, |this, cx| {
            this.zoom_save_pending = false;
            settings::save_zoom(this.zoom);
            cx.notify();
        });
    })
    .detach();
}

fn apply_zoom(
    this: &mut LabApp,
    pos: gpui::Point<Pixels>,
    b: Option<Bounds<Pixels>>,
    cx: &mut Context<LabApp>,
) {
    let Some(b) = b else { return };
    let knob = 12.;
    let usable = f32::from(b.size.width) - knob;
    if usable <= 0. {
        return;
    }
    let f = ((f32::from(pos.x) - f32::from(b.origin.x) - knob / 2.) / usable).clamp(0., 1.);
    let z = MIN_ZOOM + f * (MAX_ZOOM - MIN_ZOOM);
    if (z - this.zoom).abs() > 0.004 {
        this.zoom = z;
        cx.notify();
    }
}

impl Render for LabApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Frame counter for the HUD's 1 s fps ticker (line 57).
        self.frames += 1;

        // Global UI zoom: the rem base is 16 px × slider factor; everything
        // sized in rems scales with it.
        window.set_rem_size(px(16. * self.zoom));

        let bounds_cell = self.zoom_bounds.clone();

        let content: AnyElement = match self.screen {
            Screen::Notes => self
                .notes
                .get_or_insert_with(|| cx.new(NotesScreen::new))
                .clone()
                .into_any_element(),
            Screen::Gallery => self
                .gallery
                .get_or_insert_with(|| cx.new(GalleryScreen::new))
                .clone()
                .into_any_element(),
            Screen::Pics => self
                .pics
                .get_or_insert_with(|| cx.new(PicsScreen::new))
                .clone()
                .into_any_element(),
            Screen::Orbit => self
                .orbit
                .get_or_insert_with(|| cx.new(OrbitScreen::new))
                .clone()
                .into_any_element(),
            Screen::Sys => self
                .sys
                .get_or_insert_with(|| cx.new(SysScreen::new))
                .clone()
                .into_any_element(),
            Screen::Touch => self
                .touch
                .get_or_insert_with(|| cx.new(TouchScreen::new))
                .clone()
                .into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::rgb(0x101014))
            .text_color(gpui::rgb(0xe9e9ee))
            .when(cfg!(target_os = "android"), |d| d.pt(px(90.)))
            // Zoom-drag continuation outside the slider track.
            .on_mouse_move(cx.listener({
                let cell = bounds_cell.clone();
                move |this, e: &gpui::MouseMoveEvent, _, cx| {
                    if this.zoom_drag {
                        apply_zoom(this, e.position, cell.get(), cx);
                    }
                }
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                    if this.zoom_drag {
                        this.zoom_drag = false;
                        settings::save_zoom(this.zoom);
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .px(px(3.))
                    .py(px(2.))
                    .border_b_1()
                    .border_color(gpui::rgb(0x2c2c34))
                    .child(self.tab("Notes", Screen::Notes, cx))
                    .child(self.tab("Gallery", Screen::Gallery, cx))
                    .child(self.tab("Pics", Screen::Pics, cx))
                    .child(self.tab("Orbit", Screen::Orbit, cx))
                    .child(self.tab("sys", Screen::Sys, cx))
                    .child(self.tab("Touch", Screen::Touch, cx))
                    .child(self.zoom_slider(cx))
                    .child(
                        div()
                            .flex_1()
                            .text_right()
                            .text_size(rems(0.75))
                            .text_color(gpui::rgb(0x6b6b74))
                            .child(format!("{} fps · {} · {:.0}%", self.fps, self.draw_ms, self.zoom * 100.)),
                    ),
            )
            .child(div().flex_1().min_h_0().child(content))
    }
}
