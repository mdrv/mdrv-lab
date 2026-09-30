//! Notes — mdrv-db CRUD (Fjall envelope + Turso port), exercising durable
//! journal/fsync writes from the UI thread.

use gpui::prelude::*;
use gpui::{div, px, App, Context, IntoElement, ParentElement, Render, Styled, Window};
use mdrv_db::engine::{Engine, EngineConfig, MutateRequest};
use mdrv_db::entry::{Op, PortValue, SqlKind};
use mdrv_db::port_turso::TursoPort;

use std::sync::OnceLock;

static ANDROID_DATA_DIR: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Called from `android_main` before the platform starts.
pub fn set_android_data_dir(dir: Option<std::path::PathBuf>) {
    if let Some(dir) = dir {
        let _ = ANDROID_DATA_DIR.set(dir);
    }
}

fn data_root() -> std::path::PathBuf {
    if let Some(dir) = ANDROID_DATA_DIR.get() {
        return dir.join("mdrv-lab");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::Path::new(&home)
        .join(".local/state/mdrv-lab")
        .join("mdrv-lab")
}

pub struct NotesScreen {
    engine: Option<Engine>,
    rows: Vec<(i64, String)>,
    err: Option<String>,
    counter: u32,
    lsn: u64,
}

impl NotesScreen {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut s = Self {
            engine: None,
            rows: Vec::new(),
            err: None,
            counter: 0,
            lsn: 0,
        };
        s.refresh();
        s.open_db(cx);
        s
    }

    fn open_db(&mut self, cx: &mut Context<Self>) {
        let root = data_root();
        let db_path = root.join("turso.sqlite");
        let port = match TursoPort::open(db_path) {
            Ok(p) => p,
            Err(e) => {
                self.err = Some(format!("port: {e}"));
                cx.notify();
                return;
            }
        };
        match Engine::open(&root, "mdrv-lab", Box::new(port), EngineConfig::default()) {
            Ok(mut engine) => {
                if let Err(e) = engine.bootstrap(&[format!(
                    "CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, title TEXT NOT NULL, created_ms INTEGER NOT NULL)"
                )]) {
                    self.err = Some(format!("bootstrap: {e}"));
                }
                self.engine = Some(engine);
                self.refresh_rows(cx);
            }
            Err(e) => self.err = Some(format!("open: {e}")),
        }
        cx.notify();
    }

    /// Recount/refresh without DB.
    fn refresh(&mut self) {}

    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        match engine.query("SELECT id, title FROM notes ORDER BY id", vec![]) {
            Ok(v) => {
                let mut rows = Vec::new();
                if let Some(arr) = v.as_array() {
                    for r in arr {
                        let id = r.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                        let title = r
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("?")
                            .to_string();
                        rows.push((id, title));
                    }
                }
                self.rows = rows;
                self.err = None;
            }
            Err(e) => self.err = Some(format!("query: {e}")),
        }
        cx.notify();
    }

    fn add_note(&mut self, cx: &mut Context<Self>) {
        self.counter += 1;
        self.mutate(
            cx,
            vec![Op::Sql {
                kind: SqlKind::Insert,
                table: "notes".into(),
                pk_col: "id".into(),
                columns: vec!["id".into(), "title".into(), "created_ms".into()],
                values: vec![
                    PortValue::Lsn,
                    PortValue::Text(format!("Note #{} (lab)", self.counter)),
                    PortValue::Int(mdrv_db::now_ms()),
                ],
                pk: PortValue::Lsn,
            }],
        );
    }

    fn rename_first(&mut self, cx: &mut Context<Self>) {
        let Some((id, title)) = self.rows.first().cloned() else {
            return;
        };
        self.mutate(
            cx,
            vec![Op::Sql {
                kind: SqlKind::Update,
                table: "notes".into(),
                pk_col: "id".into(),
                columns: vec!["title".into()],
                values: vec![PortValue::Text(format!("{title} ✎"))],
                pk: PortValue::Int(id),
            }],
        );
    }

    fn delete_first(&mut self, cx: &mut Context<Self>) {
        let Some((id, _)) = self.rows.first().cloned() else {
            return;
        };
        self.mutate(
            cx,
            vec![Op::Sql {
                kind: SqlKind::Delete,
                table: "notes".into(),
                pk_col: "id".into(),
                columns: vec![],
                values: vec![],
                pk: PortValue::Int(id),
            }],
        );
    }

    fn mutate(&mut self, cx: &mut Context<Self>, ops: Vec<Op>) {
        let Some(engine) = self.engine.as_mut() else {
            self.err = Some("db not open".into());
            cx.notify();
            return;
        };
        let t0 = std::time::Instant::now();
        match engine.execute(MutateRequest {
            actor: "ui".into(),
            ops,
            idem_key: None,
            response: None,
        }) {
            Ok(out) => {
                self.lsn = out.lsn;
                self.err = Some(format!("ok: lsn {} in {:?}", out.lsn, t0.elapsed()));
                self.refresh_rows(cx);
            }
            Err(e) => {
                self.err = Some(format!("execute: {e}"));
                cx.notify();
            }
        }
    }

    fn btn(
        &self,
        id: &'static str,
        label: &str,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .px(3)
            .py(1.5)
            .mr_2()
            .rounded(6.)
            .bg(gpui::rgb(0x1f2937))
            .cursor_pointer()
            .text_size(px(14.))
            .child(label)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| f(this, cx)),
            )
    }
}

impl Render for NotesScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div()
            .id("notes-list")
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .overflow_y_scroll();
        for (id, title) in &self.rows {
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .px(2)
                    .py(1)
                    .rounded(6.)
                    .bg(gpui::rgb(0x17171b))
                    .text_size(px(14.))
                    .child(div().text_color(gpui::rgb(0x6b6b74)).child(format!("{id}")))
                    .child(title.clone()),
            );
        }
        if self.rows.is_empty() {
            list = list.child(
                div()
                    .p_2()
                    .text_size(px(13.))
                    .text_color(gpui::rgb(0x6b6b74))
                    .child("no notes — tap add"),
            );
        }

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
                    .child(self.btn("add", "+ add", cx, |s, cx| s.add_note(cx)))
                    .child(self.btn("ren", "rename 1st", cx, |s, cx| s.rename_first(cx)))
                    .child(self.btn("del", "delete 1st", cx, |s, cx| s.delete_first(cx)))
                    .child(
                        div()
                            .flex_1()
                            .text_right()
                            .text_size(px(12.))
                            .text_color(gpui::rgb(0x6b6b74))
                            .child(format!("lsn {}", self.lsn)),
                    ),
            )
            .child(
                div()
                    .px(2)
                    .text_size(px(12.))
                    .text_color(match &self.err {
                        Some(e) if e.starts_with("ok") => gpui::rgb(0x4ade80),
                        Some(_) => gpui::rgb(0xf87171),
                        None => gpui::rgb(0x6b6b74),
                    })
                    .child(self.err.clone().unwrap_or_else(|| "db idle".into())),
            )
            .child(list)
    }
}
