//! Pics — upload images into mdrv-db (content-addressed blobs + SQL rows),
//! view and animate them (slow Ken Burns pan/zoom, for kids), and post them
//! as big-picture Android notifications.
//!
//! The engine gets its own data root (`<data>/media`) so it never contends
//! with the notes database. Image bytes live in the blob store; rows in the
//! `images` table reference them by sha-256 hash.

use gpui::prelude::*;
use gpui::{
    div, px, rems, Context, ImageSource, IntoElement, ParentElement, Render, RenderImage, Styled,
    Window,
};
use mdrv_db::engine::{Engine, EngineConfig, MutateRequest};
use mdrv_db::entry::{Op, PortValue, SqlKind};
use mdrv_db::port_turso::TursoPort;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{notes::data_root, platform};

pub struct PicsScreen {
    engine: Option<Engine>,
    /// (id, hash, name)
    images: Vec<(i64, String, String)>,
    idx: usize,
    render: Option<Arc<RenderImage>>,
    dims: (u32, u32),
    anim: bool,
    anim_start: std::time::Instant,
    status: Option<String>,
    awaiting_pick: bool,
}

/// Slow ambient pan/zoom ("Ken Burns") — gentle motion for kids.
fn ken_burns(t: f32) -> (f32, f32, f32) {
    let tau = std::f32::consts::TAU;
    let z = 1.0 + 0.10 * (0.5 - 0.5 * (t * tau / 9.0).cos());
    let dx = 24.0 * (t * tau / 11.0).sin();
    let dy = 14.0 * (t * tau / 13.0).sin();
    (z, dx, dy)
}

impl PicsScreen {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Animation driver: re-render at ~30 fps while anim is on.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(33))
                    .await;
                if this
                    .update(cx, |s, cx| {
                        if s.anim {
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

        // Poll for the Android photo-picker result while one is pending.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(400))
                    .await;
                if this
                    .update(cx, |s, cx| {
                        if !s.awaiting_pick {
                            return;
                        }
                        if let Some(path) = platform::take_picked_image() {
                            s.awaiting_pick = false;
                            let name = std::path::Path::new(&path)
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "picked".into());
                            s.import_file(PathBuf::from(&path), name, cx);
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();

        let mut s = Self {
            engine: None,
            images: Vec::new(),
            idx: 0,
            render: None,
            dims: (1, 1),
            anim: false,
            anim_start: std::time::Instant::now(),
            status: None,
            awaiting_pick: false,
        };
        s.open_db(cx);
        s
    }

    fn open_db(&mut self, cx: &mut Context<Self>) {
        let root = data_root().join("media");
        if let Err(e) = std::fs::create_dir_all(&root) {
            self.status = Some(format!("mkdir: {e}"));
            cx.notify();
            return;
        }
        let port = match TursoPort::open(root.join("turso.sqlite")) {
            Ok(p) => p,
            Err(e) => {
                self.status = Some(format!("port: {e}"));
                cx.notify();
                return;
            }
        };
        match Engine::open(&root, "mdrv-lab-media", Box::new(port), EngineConfig::default()) {
            Ok(engine) => {
                if let Err(e) = engine.bootstrap(&[format!(
                    "CREATE TABLE IF NOT EXISTS images (id INTEGER PRIMARY KEY, hash TEXT NOT NULL, name TEXT NOT NULL, created_ms INTEGER NOT NULL)"
                )]) {
                    self.status = Some(format!("bootstrap: {e}"));
                }
                self.engine = Some(engine);
                self.refresh(cx);
            }
            Err(e) => self.status = Some(format!("open: {e}")),
        }
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        match engine.query("SELECT id, hash, name FROM images ORDER BY id", vec![]) {
            Ok(v) => {
                let mut rows = Vec::new();
                if let Some(arr) = v.as_array() {
                    for r in arr {
                        rows.push((
                            r.get("id").and_then(|v| v.as_i64()).unwrap_or(0),
                            r.get("hash").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            r.get("name").and_then(|v| v.as_str()).unwrap_or("?").to_string(),
                        ));
                    }
                }
                self.images = rows;
                if self.idx >= self.images.len() {
                    self.idx = self.images.len().saturating_sub(1);
                }
                self.load_current();
            }
            Err(e) => self.status = Some(format!("query: {e}")),
        }
        cx.notify();
    }

    /// Import a file: bytes → content-addressed blob → SQL row.
    pub fn import_file(&mut self, path: PathBuf, name: String, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.as_ref() else {
            self.status = Some("db not open".into());
            cx.notify();
            return;
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                self.status = Some(format!("read {}: {e}", path.display()));
                cx.notify();
                return;
            }
        };
        let (hash, n) = match engine.put_blob(&bytes) {
            Ok(v) => v,
            Err(e) => {
                self.status = Some(format!("put_blob: {e}"));
                cx.notify();
                return;
            }
        };
        self.execute_insert(hash, name, n, cx);
    }

    fn execute_insert(
        &mut self,
        hash: String,
        name: String,
        byte_len: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        let t0 = std::time::Instant::now();
        match engine.execute(MutateRequest {
            actor: "ui".into(),
            ops: vec![
                // Journal the blob reference; execute() promotes the staged
                // file to its final content-addressed path post-commit.
                Op::BlobPut {
                    hash_hex: hash.clone(),
                },
                Op::Sql {
                    kind: SqlKind::Insert,
                    table: "images".into(),
                    pk_col: "id".into(),
                    columns: vec!["id".into(), "hash".into(), "name".into(), "created_ms".into()],
                    values: vec![
                        PortValue::Lsn,
                        PortValue::Text(hash.clone()),
                        PortValue::Text(name.clone()),
                        PortValue::Int(mdrv_db::now_ms()),
                    ],
                    pk: PortValue::Lsn,
                },
            ],
            idem_key: None,
            response: None,
        }) {
            Ok(out) => {
                self.status = Some(format!(
                    "imported {name} ({} KiB → {}, lsn {} in {:?})",
                    byte_len / 1024,
                    &hash[..8],
                    out.lsn,
                    t0.elapsed()
                ));
            }
            Err(e) => self.status = Some(format!("execute: {e}")),
        }
        self.refresh(cx);
    }

    /// Decode the current image's blob into a RenderImage (BGRA for gpui).
    fn load_current(&mut self) {
        self.render = None;
        self.dims = (1, 1);
        let Some((_, hash, _)) = self.images.get(self.idx) else {
            return;
        };
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        let path = engine.blobs.final_path(hash);
        let result = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| image::load_from_memory(&bytes).map_err(|e| e.to_string()));
        match result {
            Ok(img) => {
                let mut rgba = img.to_rgba8();
                self.dims = rgba.dimensions();
                for p in rgba.pixels_mut() {
                    p.0.swap(0, 2); // gpui wants BGRA (see decode_static_image)
                }
                self.render = Some(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])));
            }
            Err(e) => self.status = Some(format!("decode: {e}")),
        }
    }

    fn step(&mut self, dir: i32, cx: &mut Context<Self>) {
        if self.images.is_empty() {
            return;
        }
        let n = self.images.len() as i32;
        self.idx = (self.idx as i32 + dir).rem_euclid(n) as usize;
        self.load_current();
        cx.notify();
    }

    fn notify_current(&mut self, cx: &mut Context<Self>) {
        let Some((_, hash, name)) = self.images.get(self.idx).cloned() else {
            self.status = Some("no image selected".into());
            cx.notify();
            return;
        };
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        let path = engine.blobs.final_path(&hash);
        match platform::notify("mdrv-lab", &name, Some(&path)) {
            Ok(()) => self.status = Some(format!("notify img ok: {name}")),
            Err(e) => self.status = Some(format!("notify: {e}")),
        }
        cx.notify();
    }

    fn btn(
        &self,
        id: &'static str,
        label: impl Into<gpui::SharedString>,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .px(rems(0.19))
            .py(rems(0.09))
            .mr_2()
            .mb_1()
            .rounded(px(6.))
            .bg(gpui::rgb(0x1f2937))
            .cursor_pointer()
            .text_size(rems(0.875))
            .child(label.into())
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| f(this, cx)),
            )
    }
}

impl Render for PicsScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (z, dx, dy) = if self.anim {
            ken_burns(self.anim_start.elapsed().as_secs_f32())
        } else {
            (1.0, 0.0, 0.0)
        };
        let (bw, bh) = (self.dims.0.max(1) as f32, self.dims.1.max(1) as f32);
        let disp_w = 420. * z;
        let disp_h = disp_w * bh / bw;

        let image_area = match self.render.clone() {
            Some(render) => div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .bg(gpui::rgb(0x0a0a0d))
                .child(
                    div().ml(px(dx)).mt(px(dy)).child(
                        gpui::img(ImageSource::Render(render))
                            .w(px(disp_w))
                            .h(px(disp_h))
                            .rounded(px(8.)),
                    ),
                ),
            None => div()
                .flex_1()
                .min_h_0()
                .p_2()
                .text_size(rems(0.8125))
                .text_color(gpui::rgb(0x6b6b74))
                .child(
                    self.status
                        .clone()
                        .unwrap_or_else(|| "no images yet — import lab.png or pick from the phone".into()),
                ),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .p_2()
                    .child(self.btn("imp", "+ lab.png", cx, |s, cx| {
                        let p = crate::gallery::asset_path("lab.png");
                        s.import_file(p, "lab.png".into(), cx);
                    }))
                    .child(self.btn("pick", "pick…", cx, |s, cx| {
                        #[cfg(target_os = "android")]
                        {
                            match platform::pick_image() {
                                Ok(()) => {
                                    s.awaiting_pick = true;
                                    s.status = Some("waiting for pick…".into());
                                }
                                Err(e) => s.status = Some(format!("pick: {e}")),
                            }
                            cx.notify();
                        }
                        #[cfg(not(target_os = "android"))]
                        {
                            s.status = Some("picker is android-only".into());
                            cx.notify();
                        }
                    }))
                    .child(self.btn("prev", "◀", cx, |s, cx| s.step(-1, cx)))
                    .child(self.btn("next", "▶", cx, |s, cx| s.step(1, cx)))
                    .child(
                        self.btn(
                            "anim",
                            if self.anim { "anim on" } else { "anim off" },
                            cx,
                            |s, cx| {
                                s.anim = !s.anim;
                                if s.anim {
                                    s.anim_start = std::time::Instant::now();
                                }
                                cx.notify();
                            },
                        ),
                    )
                    .child(self.btn("nimg", "notify img", cx, |s, cx| s.notify_current(cx)))
                    .child(
                        div()
                            .flex_1()
                            .text_right()
                            .text_size(rems(0.75))
                            .text_color(gpui::rgb(0x6b6b74))
                            .child(if self.images.is_empty() {
                                "0".to_string()
                            } else {
                                format!("{}/{}", self.idx + 1, self.images.len())
                            }),
                    ),
            )
            .child(
                div()
                    .px(px(2.))
                    .text_size(rems(0.75))
                    .text_color(match &self.status {
                        Some(e) if e.starts_with("imported") || e.starts_with("notify") => gpui::rgb(0x4ade80),
                        Some(_) => gpui::rgb(0xf87171),
                        None => gpui::rgb(0x6b6b74),
                    })
                    .child(self.status.clone().unwrap_or_else(|| "db idle".into())),
            )
            .child(image_area)
    }
}
