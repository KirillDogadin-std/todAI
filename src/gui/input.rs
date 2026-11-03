//! Glue between egui and the shared navigation in `todai_client::nav`:
//! translating events, waking the window from worker threads, and handing
//! visual mode the text layout.

use std::sync::Arc;

use egui::text::CCursor;
use egui::{Event, Galley};
use todai_client::nav::{Env, Input, Key, Mods, Outcome, Rows};
use todai_client::tab::{Tab, Wake};

/// Feeds this frame's key events to the tab's `Nav` and removes the ones it
/// used, so widgets never see them.
pub fn handle(ctx: &egui::Context, tab: &mut Tab, vim: bool) -> Outcome {
    let mut out = Outcome::default();
    let env = Env { popup_open: egui::Popup::is_any_open(ctx) };
    let events = ctx.input(|i| i.events.clone());
    let mut keep = Vec::with_capacity(events.len());
    for ev in events {
        let used = match input(&ev) {
            Some(i) => tab.nav.input(&i, env, &tab.state, &mut tab.drafts, vim, &mut out),
            None => false,
        };
        if !used {
            keep.push(ev);
        }
    }
    ctx.input_mut(|i| i.events = keep);
    if out.leave_input {
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
    }
    out
}

fn input(ev: &Event) -> Option<Input> {
    match ev {
        Event::Text(t) => Some(Input::Text(t.clone())),
        Event::Key { key, pressed, modifiers: m, .. } => Some(Input::Key {
            key: key_of(*key),
            pressed: *pressed,
            mods: Mods { ctrl: m.ctrl, shift: m.shift, alt: m.alt, command: m.command },
        }),
        _ => None,
    }
}

fn key_of(k: egui::Key) -> Key {
    use egui::Key as E;
    match k {
        E::ArrowUp => Key::Up,
        E::ArrowDown => Key::Down,
        E::ArrowLeft => Key::Left,
        E::ArrowRight => Key::Right,
        E::Enter => Key::Enter,
        E::Escape => Key::Escape,
        E::Tab => Key::Tab,
        E::PageUp => Key::PageUp,
        E::PageDown => Key::PageDown,
        E::Home => Key::Home,
        E::End => Key::End,
        _ => match k.name().chars().collect::<Vec<_>>()[..] {
            [c] if c.is_ascii_alphabetic() => Key::Letter(c.to_ascii_lowercase()),
            _ => Key::Other,
        },
    }
}

/// Wakes the window from a watcher or network thread.
pub fn wake(ctx: &egui::Context) -> Wake {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}

/// Visual mode's `j` / `k` over the galley the text was drawn with.
pub struct GalleyRows(pub Arc<Galley>);

impl Rows for GalleyRows {
    fn down(&self, caret: usize, h: Option<f32>) -> (usize, Option<f32>) {
        let (c, h) = self.0.cursor_down_one_row(&CCursor::new(caret), h);
        (c.index.0, h)
    }

    fn up(&self, caret: usize, h: Option<f32>) -> (usize, Option<f32>) {
        let (c, h) = self.0.cursor_up_one_row(&CCursor::new(caret), h);
        (c.index.0, h)
    }
}

/// Where each item's text was drawn this frame, for tests driving the mouse.
#[cfg(test)]
pub mod text_rects {
    use todai_client::nav::Focus;

    fn id() -> egui::Id {
        egui::Id::new("test-text-rects")
    }

    pub fn clear(ctx: &egui::Context) {
        ctx.data_mut(|d| d.remove::<Vec<(Focus, egui::Rect)>>(id()));
    }

    pub fn push(ctx: &egui::Context, f: Focus, r: egui::Rect) {
        ctx.data_mut(|d| d.get_temp_mut_or_default::<Vec<(Focus, egui::Rect)>>(id()).push((f, r)));
    }

    pub fn get(ctx: &egui::Context, f: &Focus) -> Option<egui::Rect> {
        ctx.data(|d| d.get_temp::<Vec<(Focus, egui::Rect)>>(id())?.iter().find(|(x, _)| x == f).map(|(_, r)| *r))
    }
}
