//! Gallery — bundled image with drag-pan / wheel-zoom / 90° rotate.
//! Exercises input events (incl. touch→mouse translation on Android) and the
//! img atlas pipeline under transforms. Rotation rebuilds the RenderImage
//! (CPU pipeline + atlas re-upload — itself a useful stress path).

use gpui::prelude::*;
use gpui::{
    div, px, rems, Context, ImageSource, IntoElement, ParentElement, Render, RenderImage, Styled,
    Window,
};
use std::path::PathBuf;
use std::sync::Arc;

pub fn asset_path(name: &str) -> PathBuf {
    if let Ok(dir) = std::env::var("MDRV_LAB_ASSETS") {
        return PathBuf::from(dir).join(name);
    }
    // Android (lab): assets pushed to /data/local/tmp/lab-assets by deploy.sh.
    if cfg!(target_os = "android") {
        return PathBuf::from("/data/local/tmp/lab-assets").join(name);
    }
    // Desktop dev: repo assets/ dir.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name)
}

pub struct GalleryScreen {
    source_rgba: Option<image::RgbaImage>,
    render: Option<Arc<RenderImage>>,
    rotation_steps: u8,
    /// Accumulated two-finger twist (degrees) not yet committed to a
    /// 90° rotation step (Android pinch gesture).
    twist: f32,
    zoom: f32,
    pan: gpui::Point<gpui::Pixels>,
    dragging: Option<gpui::Point<gpui::Pixels>>,
    err: Option<String>,
}

impl GalleryScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let mut s = Self {
            source_rgba: None,
            render: None,
            rotation_steps: 0,
            twist: 0.0,
            zoom: 1.0,
            pan: gpui::point(gpui::px(0.), gpui::px(0.)),
            dragging: None,
            err: None,
        };
        s.load();
        s
    }

    fn load(&mut self) {
        let path = asset_path("lab.png");
        match image::open(&path) {
            Ok(img) => {
                self.source_rgba = Some(img.to_rgba8());
                self.rebuild();
            }
            Err(e) => self.err = Some(format!("load {}: {e}", path.display())),
        }
    }

    /// Bump the image id so the atlas re-uploads after rotation.
    fn rebuild(&mut self) {
        let Some(rgba) = self.source_rgba.as_ref() else {
            return;
        };
        let rotated = match self.rotation_steps % 4 {
            0 => rgba.clone(),
            1 => image::imageops::rotate90(rgba),
            2 => image::imageops::rotate180(rgba),
            _ => image::imageops::rotate270(rgba),
        };
        // gpui expects BGRA (see decode_static_image)
        let mut bgra = rotated;
        for px in bgra.pixels_mut() {
            px.0.swap(0, 2);
        }
        self.render = Some(Arc::new(RenderImage::new(vec![image::Frame::new(bgra)])));
        self.err = None;
    }

    fn btn(
        &self,
        id: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .px(px(3.))
            .py(px(1.5))
            .mr_2()
            .rounded(px(6.))
            .bg(gpui::rgb(0x1f2937))
            .cursor_pointer()
            .text_size(px(14.))
            .child(label)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                    f(this);
                    cx.notify();
                }),
            )
    }
}

impl Render for GalleryScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Android: drain two-finger pinch/rotate from the fork's gesture
        // tracker (fed by AndroidWindow::handle_touch).
        #[cfg(target_os = "android")]
        {
            let s = gpui_mobile::android::gesture::take_scale();
            if s != 1.0 {
                self.zoom = (self.zoom * s).clamp(0.2, 6.);
            }
            let rot = gpui_mobile::android::gesture::take_rotation_delta();
            if rot.abs() > 1.0 {
                self.twist += rot;
                let quarters = (self.twist / 45.0) as i32;
                if quarters != 0 {
                    self.twist -= quarters as f32 * 45.0;
                    self.rotation_steps = if quarters > 0 {
                        self.rotation_steps.wrapping_add(quarters as u8)
                    } else {
                        self.rotation_steps.wrapping_sub((-quarters) as u8)
                    };
                    self.rebuild();
                }
            }
            if gpui_mobile::android::gesture::is_pinching() {
                cx.notify(); // keep live-updating while fingers are down
            }
        }

        let zoom = self.zoom;
        let pan = self.pan;
        let steps = self.rotation_steps % 4;
        let (w, h) = self
            .source_rgba
            .as_ref()
            .map(|i| i.dimensions())
            .unwrap_or((900, 600));
        let (w, h) = if steps % 2 == 1 { (h, w) } else { (w, h) };
        let disp_w = 460. * zoom;
        let disp_h = disp_w * h as f32 / w as f32;

        let img_row = match self.render.clone() {
            Some(render) => div()
                .ml(px(40. + f32::from(pan.x)))
                .mt(px(20. + f32::from(pan.y)))
                .child(
                    gpui::img(ImageSource::Render(render))
                        .w(px(disp_w))
                        .h(px(disp_h))
                        .rounded(px(8.)),
                ),
            None => div()
                .ml(px(20.))
                .mt(px(20.))
                .text_size(rems(0.8125))
                .text_color(gpui::rgb(0xf87171))
                .child(self.err.clone().unwrap_or_else(|| "no image".into())),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .p_2()
                    .child(self.btn("z+", "zoom +", cx, |s| s.zoom = (s.zoom * 1.2).min(6.)))
                    .child(self.btn("z-", "zoom -", cx, |s| s.zoom = (s.zoom / 1.2).max(0.2)))
                    .child(self.btn("rot", "rotate", cx, |s| {
                        s.rotation_steps = s.rotation_steps.wrapping_add(1);
                        s.rebuild();
                    }))
                    .child(self.btn("rst", "reset", cx, |s| {
                        s.zoom = 1.0;
                        s.pan = gpui::point(gpui::px(0.), gpui::px(0.));
                        s.rotation_steps = 0;
                        s.rebuild();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .text_right()
                            .text_size(px(12.))
                            .text_color(gpui::rgb(0x6b6b74))
                            .child(format!("{:.0}% / {}°", zoom * 100., steps as f32 * 90.)),
                    ),
            )
            .child(
                div()
                    .id("canvas")
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .bg(gpui::rgb(0x0a0a0d))
                    .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _, cx| {
                        if let Some(start) = this.dragging {
                            this.pan = gpui::point(
                                this.pan.x + (e.position.x - start.x),
                                this.pan.y + (e.position.y - start.y),
                            );
                            this.dragging = Some(e.position);
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| {
                            this.dragging = Some(e.position);
                            cx.notify();
                        }),
                    )
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                            this.dragging = None;
                            cx.notify();
                        }),
                    )
                    .on_scroll_wheel(cx.listener(|this, e: &gpui::ScrollWheelEvent, _, cx| {
                        let delta = e.delta.pixel_delta(px(1.));
                        let factor = if delta.y > px(0.) { 1.1 } else { 1. / 1.1 };
                        this.zoom = (this.zoom * factor).clamp(0.2, 6.);
                        cx.notify();
                    }))
                    .child(img_row),
            )
    }
}
