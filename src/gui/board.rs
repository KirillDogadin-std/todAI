//! The board itself: the accordion of pending and done items, and the
//! elaboration list (a right column when wide, an accordion section when not).
//! Rendering records every focusable thing into `Nav` in display order.

use std::collections::HashMap;
use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align, Button, CollapsingHeader, Color32, Frame, Galley, Label, Layout, RichText, ScrollArea,
    Sense, Stroke, TextBuffer, TextEdit, TextStyle, Ui,
};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use super::nav::{self, Focus, FocusRequest, Nav, Section, Weight};
use crate::store::{Decision, DecisionStatus, Kind, State, Todo};

pub struct Board<'a> {
    pub state: &'a State,
    pub drafts: &'a mut HashMap<String, String>,
    pub nav: &'a mut Nav,
    pub md: &'a mut CommonMarkCache,
    pub vim: bool,
    pub out: Vec<Kind>,
}

impl Board<'_> {
    pub fn left_column(&mut self, ui: &mut Ui, with_elabs: bool) {
        let state = self.state;
        let pending: Vec<_> = state.decisions.iter().filter(|d| d.status == DecisionStatus::Pending).collect();
        let todos: Vec<_> = state.todos.iter().filter(|t| t.done.is_none()).collect();
        let made: Vec<_> =
            state.decisions.iter().filter(|d| matches!(d.status, DecisionStatus::Resolved { .. })).collect();
        let done: Vec<_> = state.todos.iter().filter(|t| t.done.is_some()).collect();

        ScrollArea::vertical().id_salt("left").auto_shrink(false).show(ui, |ui| {
            let title = format!("Pending  {}", pending.len() + todos.len());
            self.section(ui, Section::Pending, &title, |b, ui| {
                subheading(ui, "Decisions");
                if pending.is_empty() {
                    ui.weak("Nothing to decide.");
                }
                for d in &pending {
                    b.decision_card(ui, d);
                }
                ui.add_space(8.0);
                subheading(ui, "Todos");
                if todos.is_empty() {
                    ui.weak("Nothing to do by hand.");
                }
                for t in &todos {
                    b.todo_card(ui, t);
                }
            });

            if with_elabs {
                let waiting = state.elabs.iter().filter(|r| r.answer.is_none()).count();
                let title = match waiting {
                    0 => format!("Elaborations  {}", state.elabs.len()),
                    n => format!("Elaborations  {}  ({n} waiting)", state.elabs.len()),
                };
                self.section(ui, Section::Elabs, &title, |b, ui| b.elaborations(ui, false));
            }

            let title = format!("Done  {}", made.len() + done.len());
            self.section(ui, Section::Done, &title, |b, ui| {
                subheading(ui, "Decisions made");
                if made.is_empty() {
                    ui.weak("None yet.");
                }
                for d in &made {
                    b.resolved_card(ui, d);
                }
                ui.add_space(8.0);
                subheading(ui, "Done");
                if done.is_empty() {
                    ui.weak("None yet.");
                }
                for t in &done {
                    b.done_card(ui, t);
                }
            });
        });
    }

    /// `right` records the cards into the right-hand focus list.
    pub fn elaborations(&mut self, ui: &mut Ui, right: bool) {
        let state = self.state;
        if state.elabs.is_empty() {
            ui.weak("No elaborations requested.");
            return;
        }
        let title = |id: &str| {
            state
                .decision(id)
                .map(|d| d.title.as_str())
                .or_else(|| state.todo(id).map(|t| t.title.as_str()))
                .unwrap_or("")
        };
        for r in state.elabs.iter().rev() {
            let focus = Focus::Elab(r.rid.clone());
            if right {
                self.nav.right.push(focus.clone());
            } else {
                self.nav.left.push(focus.clone());
            }
            self.card(ui, &focus, |b, ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&r.rid).monospace().weak());
                    ui.label(RichText::new(format!("on {}", r.target)).monospace().weak());
                    ui.add(Label::new(RichText::new(title(&r.target)).weak()).wrap());
                });
                ui.add(Label::new(RichText::new(format!("“{}”", r.selection)).italics()).wrap());
                if let Some(n) = &r.note {
                    ui.add(Label::new(RichText::new(n).small()).wrap());
                }
                ui.separator();
                match &r.answer {
                    Some(_) if b.visual_on(&focus) => {
                        b.item_text(ui, &focus);
                    }
                    Some(a) => {
                        ui.push_id(&r.rid, |ui| CommonMarkViewer::new().show(ui, b.md, a));
                        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                            if ui.small_button("Follow up").on_hover_text("e · or v to select").clicked() {
                                b.nav.focus(focus.clone());
                                b.nav.open_prompt(r.target.clone(), a.trim().to_owned());
                            }
                        });
                    }
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.weak("waiting for agent");
                        });
                    }
                }
            });
        }
    }

    fn decision_card(&mut self, ui: &mut Ui, d: &Decision) {
        let focus = Focus::Item(d.id.clone());
        self.nav.left.push(focus.clone());
        self.card(ui, &focus, |b, ui| {
            let selected = b.item_header(ui, &d.id, &focus);
            if !d.options.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    let draft = b.drafts.entry(d.id.clone()).or_default();
                    for (i, o) in d.options.iter().enumerate() {
                        let label = if i < 9 { format!("{} {o}", i + 1) } else { o.clone() };
                        if ui.add(Button::new(label).small().selected(draft == o)).clicked() {
                            *draft = o.clone();
                        }
                    }
                });
            }
            ui.horizontal(|ui| {
                let buttons_w = 170.0;
                let draft = b.drafts.entry(d.id.clone()).or_default();
                let edit = TextEdit::multiline(draft)
                    .desired_rows(1)
                    .hint_text("resolution  (Enter submits, Shift+Enter newline)")
                    .desired_width(ui.available_width() - buttons_w);
                let resp = ui.add(edit);
                track_input(b.nav, &resp, &d.id);
                let ready = !draft.trim().is_empty();
                if ui.add_enabled(ready, Button::new("Resolve")).clicked() {
                    b.out.push(Kind::DecisionResolve { id: d.id.clone(), text: draft.trim().to_owned() });
                }
                b.elaborate_button(ui, &focus, selected);
            });
        });
    }

    fn todo_card(&mut self, ui: &mut Ui, t: &Todo) {
        let focus = Focus::Item(t.id.clone());
        self.nav.left.push(focus.clone());
        self.card(ui, &focus, |b, ui| {
            let selected = b.item_header(ui, &t.id, &focus);
            ui.horizontal(|ui| {
                let buttons_w = 170.0;
                let note = b.drafts.entry(t.id.clone()).or_default();
                let edit = TextEdit::singleline(note)
                    .hint_text("note for the agent (optional)")
                    .desired_width(ui.available_width() - buttons_w);
                let resp = ui.add(edit);
                track_input(b.nav, &resp, &t.id);
                if ui.button("Done").clicked() {
                    let note = Some(note.trim().to_owned()).filter(|n| !n.is_empty());
                    b.out.push(Kind::TodoDone { id: t.id.clone(), note });
                }
                b.elaborate_button(ui, &focus, selected);
            });
        });
    }

    fn resolved_card(&mut self, ui: &mut Ui, d: &Decision) {
        let DecisionStatus::Resolved { text, .. } = &d.status else { return };
        let focus = Focus::Item(d.id.clone());
        self.nav.left.push(focus.clone());
        self.card(ui, &focus, |b, ui| {
            let selected = b.item_header(ui, &d.id, &focus);
            ui.horizontal(|ui| {
                ui.label(RichText::new("→").weak());
                ui.add(Label::new(RichText::new(text).strong()).wrap());
            });
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                b.elaborate_button(ui, &focus, selected);
                if ui.small_button("Reopen").on_hover_text("x").clicked() {
                    b.out.push(Kind::DecisionReopen { id: d.id.clone() });
                }
            });
        });
    }

    fn done_card(&mut self, ui: &mut Ui, t: &Todo) {
        let focus = Focus::Item(t.id.clone());
        self.nav.left.push(focus.clone());
        self.card(ui, &focus, |b, ui| {
            let selected = b.item_header(ui, &t.id, &focus);
            if let Some(Some(n)) = &t.done {
                ui.add(Label::new(RichText::new(format!("note: {n}")).weak()).wrap());
            }
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                b.elaborate_button(ui, &focus, selected);
                if ui.small_button("Undo").on_hover_text("x").clicked() {
                    b.out.push(Kind::TodoUndone { id: t.id.clone() });
                }
            });
        });
    }

    /// Item id plus its selectable text. Returns the mouse selection, if any.
    fn item_header(&mut self, ui: &mut Ui, id: &str, focus: &Focus) -> Option<String> {
        ui.horizontal_top(|ui| {
            ui.label(RichText::new(id).monospace().weak());
            self.item_text(ui, focus)
        })
        .inner
    }

    /// Visual mode draws its own selection; otherwise the text is a read-only
    /// TextEdit so the mouse can select it.
    fn item_text(&mut self, ui: &mut Ui, focus: &Focus) -> Option<String> {
        let parts = nav::item_parts(self.state, focus);
        let width = ui.available_width();
        if let Some(v) = self.nav.visual.as_mut().filter(|v| &v.target == focus) {
            let job = styled_job(ui, &parts, v.range(), Some(v.caret), width);
            let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
            let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
            ui.painter().galley(rect.min, galley.clone(), Color32::PLACEHOLDER);
            v.galley = Some(galley);
            return None;
        }
        let text: String = parts.iter().map(|(s, _)| s.as_str()).collect();
        let mut buf: &str = &text;
        let mut layouter = |ui: &Ui, _: &dyn TextBuffer, wrap: f32| -> Arc<Galley> {
            let job = styled_job(ui, &parts, None, None, wrap);
            ui.ctx().fonts_mut(|f| f.layout_job(job))
        };
        let out = TextEdit::multiline(&mut buf)
            .frame(Frame::NONE)
            .margin(0.0)
            .desired_rows(1)
            .desired_width(width)
            .layouter(&mut layouter)
            .show(ui);
        let range = out.cursor_range?;
        let (a, b) = (range.primary.index.0, range.secondary.index.0);
        (a != b).then(|| text.chars().skip(a.min(b)).take(a.abs_diff(b)).collect())
    }

    fn elaborate_button(&mut self, ui: &mut Ui, focus: &Focus, selected: Option<String>) {
        let hover = "Elaborate on the selected text, or the whole item (e · v)";
        if ui.small_button("Elaborate").on_hover_text(hover).clicked()
            && let Some(target) = nav::elab_target(self.state, focus)
        {
            let selection = selected.unwrap_or_else(|| nav::item_text(self.state, focus));
            self.nav.focus(focus.clone());
            self.nav.open_prompt(target, selection.trim().to_owned());
        }
    }

    fn visual_on(&self, focus: &Focus) -> bool {
        self.nav.visual.as_ref().is_some_and(|v| &v.target == focus)
    }

    fn section(&mut self, ui: &mut Ui, s: Section, title: &str, body: impl FnOnce(&mut Self, &mut Ui)) {
        let focus = Focus::Section(s);
        self.nav.left.push(focus.clone());
        let open = self.nav.is_open(s);
        let resp = CollapsingHeader::new(RichText::new(title).heading())
            .id_salt(s)
            .open(Some(open))
            // Skip the body while the close animation runs, so hidden items
            // never enter the focus list.
            .show(ui, |ui| {
                if open {
                    body(self, ui)
                }
            });
        let header = &resp.header_response;
        if header.clicked() {
            self.nav.set_open(s, !open);
            self.nav.cursor = Some(focus.clone());
        }
        if self.vim && self.nav.cursor.as_ref() == Some(&focus) {
            let stroke = Stroke::new(1.5, ui.visuals().selection.stroke.color);
            ui.painter().rect_stroke(header.rect.expand(2.0), 4.0, stroke, egui::StrokeKind::Outside);
            if self.nav.scroll_to_cursor {
                header.scroll_to_me(Some(Align::Center));
            }
        }
        ui.add_space(6.0);
    }

    fn card(&mut self, ui: &mut Ui, focus: &Focus, body: impl FnOnce(&mut Self, &mut Ui)) {
        let focused = self.nav.cursor.as_ref() == Some(focus);
        let mut frame = Frame::group(ui.style()).inner_margin(10.0);
        if focused && self.vim {
            frame = frame.stroke(Stroke::new(1.5, ui.visuals().selection.stroke.color));
        }
        let rect = frame
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                body(self, ui);
            })
            .response
            .rect;
        // Any click inside the card moves the cursor there, without stealing
        // the click from the widgets inside it.
        if ui.rect_contains_pointer(rect) && ui.input(|i| i.pointer.primary_pressed()) {
            self.nav.cursor = Some(focus.clone());
        }
        if focused && self.nav.scroll_to_cursor {
            ui.scroll_to_rect(rect, Some(Align::Center));
        }
        ui.add_space(4.0);
    }
}

/// Lays out `parts` with an optional inclusive selection and block caret.
fn styled_job(
    ui: &Ui,
    parts: &[(String, Weight)],
    selection: Option<(usize, usize)>,
    caret: Option<usize>,
    wrap: f32,
) -> LayoutJob {
    let v = ui.visuals();
    let font = TextStyle::Body.resolve(ui.style());
    let sel_bg = v.selection.bg_fill;
    let caret_bg = v.selection.stroke.color;
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    let mut idx = 0;
    for (text, weight) in parts {
        let color = match weight {
            Weight::Strong => v.strong_text_color(),
            Weight::Normal => v.text_color(),
            Weight::Weak => v.weak_text_color(),
        };
        let mut run = String::new();
        let mut run_bg = Color32::TRANSPARENT;
        for ch in text.chars() {
            let bg = if caret == Some(idx) {
                caret_bg
            } else if selection.is_some_and(|(a, b)| (a..=b).contains(&idx)) {
                sel_bg
            } else {
                Color32::TRANSPARENT
            };
            if bg != run_bg && !run.is_empty() {
                job.append(&run, 0.0, TextFormat { font_id: font.clone(), color, background: run_bg, ..Default::default() });
                run.clear();
            }
            run_bg = bg;
            run.push(ch);
            idx += 1;
        }
        if !run.is_empty() {
            job.append(&run, 0.0, TextFormat { font_id: font.clone(), color, background: run_bg, ..Default::default() });
        }
    }
    job
}

/// Applies a pending focus request for input `id` and reports its focus.
fn track_input(nav: &mut Nav, resp: &egui::Response, id: &str) {
    if let Some(FocusRequest::Input(want)) = &nav.focus_request
        && want == id
    {
        resp.request_focus();
        nav.focus_request = None;
    }
    if resp.has_focus() {
        nav.input_focused = Some(id.to_owned());
    }
}

fn subheading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).strong().size(15.0));
    ui.add_space(2.0);
}
