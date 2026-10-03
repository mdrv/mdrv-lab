//! Sys — device capability switches: rodio sound, TTS read-aloud, haptics,
//! keep-screen-on, immersive fullscreen, and a plain notification. The
//! buttons surface per-call results in a status line, mirroring notes.rs.

use gpui::prelude::*;
use gpui::{div, px, rems, Context, IntoElement, ParentElement, Render, Styled, Window};

use crate::{audio, platform};

pub struct SysScreen {
    status: Option<String>,
    keep_on: bool,
    immersive: bool,
}

impl SysScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            status: None,
            keep_on: false,
            immersive: false,
        }
    }

    fn run(&mut self, label: &str, r: Result<(), String>, cx: &mut Context<Self>) {
        self.status = Some(match r {
            Ok(()) => format!("{label} ok"),
            Err(e) => format!("{label} err: {e}"),
        });
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

impl Render for SysScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child(self.btn("ding", "🔔 ding", cx, |s, cx| {
                        // SoundPool + sonification usage: not muted by MIUI
                        // audio hardening (the rodio media path is).
                        let r = platform::play_sfx("chime");
                        s.run("ding", r, cx);
                    }))
                    .child(self.btn("ding2", "ding (rodio)", cx, |s, cx| {
                        let r = audio::ding();
                        s.run("audio", r, cx);
                    }))
                    .child(self.btn("notify", "notify", cx, |s, cx| {
                        let r = platform::notify("mdrv-lab", "hello from gpui-mobile", None);
                        s.run("notify", r, cx);
                    }))
                    .child(self.btn("speak", "speak", cx, |s, cx| {
                        let r = platform::speak("Hello! gpui mobile can read stories aloud.");
                        s.run("tts", r, cx);
                    }))
                    .child(self.btn("stop", "stop tts", cx, |s, cx| {
                        let r = platform::stop_speak();
                        s.run("tts stop", r, cx);
                    }))
                    .child(self.btn("haptic", "haptic", cx, |s, cx| {
                        let r = platform::haptic();
                        s.run("haptic", r, cx);
                    }))
                    .child(self.btn(
                        "keep",
                        if self.keep_on {
                            "screen on ✓"
                        } else {
                            "screen on"
                        },
                        cx,
                        |s, cx| {
                            s.keep_on = !s.keep_on;
                            let r = platform::keep_screen_on(s.keep_on);
                            s.run("screen", r, cx);
                        },
                    ))
                    .child(self.btn(
                        "imm",
                        if self.immersive {
                            "fullscreen ✓"
                        } else {
                            "fullscreen"
                        },
                        cx,
                        |s, cx| {
                            s.immersive = !s.immersive;
                            let r = platform::immersive(s.immersive);
                            s.run("fullscreen", r, cx);
                        },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .text_right()
                            .text_size(rems(0.75))
                            .text_color(gpui::rgb(0x6b6b74))
                            .child("device capabilities"),
                    ),
            )
            .child(
                div()
                    .px(px(2.))
                    .text_size(rems(0.75))
                    .text_color(match &self.status {
                        Some(s) if s.ends_with("ok") => gpui::rgb(0x4ade80),
                        Some(_) => gpui::rgb(0xf87171),
                        None => gpui::rgb(0x6b6b74),
                    })
                    .child(self.status.clone().unwrap_or_else(|| "idle".into())),
            )
    }
}
