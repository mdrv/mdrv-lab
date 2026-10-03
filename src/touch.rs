//! Touch — multi-touch visualiser ("multitouch visible test" style).
//!
//! Every active finger draws a coloured circle plus full-width/height
//! crosshair lines through its position, with a live "Touches detected: N"
//! counter and per-pointer coordinates — mirroring the classic Android
//! multi-touch test app. Powered by the fork's raw pointer feed
//! (`gpui_mobile::android::gesture::pointers()`, one entry per finger
//! currently down), which also drives pinch/rotate — so this doubles as the
//! proof that apps can build multi-player tap zones on top of the fork.
//!
//! Desktop: the mouse acts as a single pointer (press and drag).

use gpui::prelude::*;
use gpui::{
    div, px, Bounds, Context, Div, IntoElement, ParentElement, Pixels, Render, Styled, Window,
};
use std::cell::Cell;
use std::rc::Rc;

/// Vivid pointer colours (cycled by pointer index).
const PALETTE: [u32; 10] = [
    0x2196f3, 0xe91e63, 0x4caf50, 0xff5722, 0x00bcd4, 0xffeb3b, 0x9c27b0, 0xff9800, 0x8bc34a,
    0xf44336,
];

pub struct TouchScreen {
    /// Active pointers in **physical** pixels: `(id, x, y)` — converted to
    /// logical px in `render` using the current scale factor.
    pointers: Vec<(i32, f32, f32)>,
    /// Desktop only: whether the (single) pointer is currently pressed.
    #[allow(dead_code)]
    desktop_down: bool,
    /// Canvas bounds from the previous frame's prepaint. The pointer feed is
    /// in window space (display origin), but the canvas sits below the
    /// status-bar pad + header, so every drawn position subtracts the
    /// canvas origin — otherwise circles land ~100-200px below the finger.
    canvas_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl TouchScreen {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Desktop never reads cx (the poll task is android-only).
        #[cfg(not(target_os = "android"))]
        let _ = cx;
        // 8 ms poll of the fork's pointer feed (Android only): every finger
        // move updates the snapshot, so polling keeps circles glued to
        // fingers regardless of which pointer gpui's mouse synthesis tracks.
        #[cfg(target_os = "android")]
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(8))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        let p = gpui_mobile::android::gesture::pointers();
                        if p != this.pointers {
                            this.pointers = p;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();

        Self {
            pointers: Vec::new(),
            desktop_down: false,
            canvas_bounds: Rc::new(Cell::new(None)),
        }
    }
}

/// Multiply each channel of an 0xRRGGBB colour by `f`.
fn darken(c: u32, f: f32) -> u32 {
    let r = (((c >> 16) & 0xff) as f32 * f) as u32;
    let g = (((c >> 8) & 0xff) as f32 * f) as u32;
    let b = ((c & 0xff) as f32 * f) as u32;
    (r << 16) | (g << 8) | b
}

/// Desktop only: the mouse is pointer #0 (press and drag). On Android these
/// handlers must NOT be registered — synthesised mouse events from touches
/// would clobber the real pointer feed.
#[cfg(not(target_os = "android"))]
fn with_mouse_handlers(canvas: Div, cx: &mut Context<TouchScreen>) -> Div {
    canvas
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| {
                this.desktop_down = true;
                this.pointers = vec![(0, e.position.x.into(), e.position.y.into())];
                cx.notify();
            }),
        )
        .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _, cx| {
            if this.desktop_down {
                if let Some(p) = this.pointers.first_mut() {
                    *p = (0, e.position.x.into(), e.position.y.into());
                }
                cx.notify();
            }
        }))
        .on_mouse_up(
            gpui::MouseButton::Left,
            cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                this.desktop_down = false;
                this.pointers.clear();
                cx.notify();
            }),
        )
}

/// Android: no mouse fallback — the fork feed is authoritative.
#[cfg(target_os = "android")]
fn with_mouse_handlers(canvas: Div, _cx: &mut Context<TouchScreen>) -> Div {
    canvas
}

/// Hint shown when no pointer is down.
#[cfg(target_os = "android")]
fn empty_hint() -> &'static str {
    "put fingers on the screen"
}

/// Hint shown when no pointer is down (desktop).
#[cfg(not(target_os = "android"))]
fn empty_hint() -> &'static str {
    "press and drag to add a pointer"
}

impl Render for TouchScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        // Canvas origin (logical px): captured last frame via the bounds
        // element below. Pointer coords are window-space; positions inside
        // the canvas div must be canvas-local.
        let (ox, oy) = self
            .canvas_bounds
            .get()
            .map(|b| (b.origin.x.into(), b.origin.y.into()))
            .unwrap_or((0., 0.));
        // Canvas-local (logical x, logical y) + palette colour per pointer.
        let pts: Vec<((f32, f32), u32)> = self
            .pointers
            .iter()
            .enumerate()
            .map(|(i, (_, x, y))| ((x / scale - ox, y / scale - oy), PALETTE[i % PALETTE.len()]))
            .collect();

        let canvas = div()
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .bg(gpui::rgb(0x000000));

        let mut canvas = with_mouse_handlers(canvas, cx);

        // Bounds-capture element (same trick as the zoom slider): covers the
        // canvas and records its absolute bounds during prepaint for the
        // next frame's offset math.
        let bounds_cell = self.canvas_bounds.clone();
        canvas = canvas.child(
            gpui::canvas(
                move |b: Bounds<Pixels>, _, _| bounds_cell.set(Some(b)),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        );

        // Counter + coordinate dump, top-left overlay.
        let mut info = div()
            .absolute()
            .top(px(8.))
            .left(px(8.))
            .flex()
            .flex_col()
            .text_size(px(13.))
            .text_color(gpui::rgb(0xffffff));
        info = info.child(format!("Touches detected: {}", pts.len()));
        for ((x, y), _) in &pts {
            info = info.child(format!("{:.0}, {:.0}", x, y));
        }
        if pts.is_empty() {
            info = info.child(empty_hint());
        }
        canvas = canvas.child(info);

        // Crosshair lines + circle per pointer.
        for ((x, y), color) in &pts {
            let (x, y) = (*x, *y);
            let c = *color;
            canvas = canvas
                .child(
                    div()
                        .absolute()
                        .top(px(y))
                        .left(px(0.))
                        .w_full()
                        .h(px(2.))
                        .bg(gpui::rgba((c << 8) | 0x88)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(0.))
                        .h_full()
                        .w(px(2.))
                        .bg(gpui::rgba((c << 8) | 0x88)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(x - 36.))
                        .top(px(y - 36.))
                        .size(px(72.))
                        .rounded_full()
                        .bg(gpui::rgb(c))
                        .border_4()
                        .border_color(gpui::rgb(darken(c, 0.6))),
                );
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::rgb(0x101014))
            .text_color(gpui::rgb(0xe9e9ee))
            .child(canvas)
    }
}
