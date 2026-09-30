//! Root view: screen tabs + FPS HUD.

use gpui::prelude::*;
use gpui::{div, px, App, Context, Render, Window, IntoElement, ParentElement, SharedString, Styled, StyledImage, ImageSource};

use crate::{gallery::GalleryScreen, notes::NotesScreen, orbit::OrbitScreen};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Notes,
    Gallery,
    Orbit,
}

pub struct LabApp {
    screen: Screen,
    frames: u32,
    fps: u32,
    fps_tick: std::time::Instant,
}

impl LabApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // 1-second FPS ticker
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                if this.update(cx, |s, cx| {
                    s.fps = s.frames;
                    s.frames = 0;
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
            fps_tick: std::time::Instant::now(),
        }
    }

    fn tab(&self, label: &str, screen: Screen) -> impl IntoElement {
        let active = self.screen == screen;
        div()
            .id(SharedString::from(format!("tab-{label}")))
            .px(4)
            .py(1.5)
            .rounded(6.)
            .cursor_pointer()
            .text_size(px(14.))
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
}

impl Render for LabApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frames += 1;
        let fps = self.fps;

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::rgb(0x101014))
            .text_color(gpui::rgb(0xe9e9ee))
            // header
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px(3)
                    .py(2)
                    .border_b_1()
                    .border_color(gpui::rgb(0x2c2c34))
                    .child(self.tab("Notes", Screen::Notes))
                    .child(self.tab("Gallery", Screen::Gallery))
                    .child(self.tab("Orbit", Screen::Orbit))
                    .child(
                        div()
                            .flex_1()
                            .text_right()
                            .text_size(px(12.))
                            .text_color(gpui::rgb(0x6b6b74))
                            .child(format!("{fps} fps")),
                    ),
            )
            // active screen
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(match self.screen {
                        Screen::Notes => cx.new(|cx| NotesScreen::new(cx)).into_any_element(),
                        Screen::Gallery => cx.new(|cx| GalleryScreen::new(cx)).into_any_element(),
                        Screen::Orbit => cx.new(|cx| OrbitScreen::new(cx)).into_any_element(),
                    }),
            )
    }
}
