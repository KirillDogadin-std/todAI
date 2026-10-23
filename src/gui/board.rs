//! The board itself: the accordion of pending and done items, and the
//! elaboration list (a right column when wide, an accordion section when not).
//! Rendering records every focusable thing into `Nav` in display order.

use std::collections::HashMap;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{
    Align, Button, Color32, Frame, Galley, Label, Layout, RichText, ScrollArea,
    Sense, Stroke, TextBuffer, TextEdit, TextStyle, Ui,
};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use super::nav::{self, Focus, FocusRequest, MouseSelection, Nav, Section, Weight};
use crate::store::{Decision, DecisionStatus, Elab, Kind, State, Todo};

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
        // Withdrawn decisions stay visible, struck through, so the history is complete.
        let made: Vec<_> = state.decisions.iter().filter(|d| d.status != DecisionStatus::Pending).collect();
        let done: Vec<_> = state.todos.iter().filter(|t| t.done.is_some()).collect();

        ScrollArea::vertical().id_salt("left").auto_shrink(false).show(ui, |ui| {
            let count = (pending.len() + todos.len()).to_string();
            self.section(ui, Section::Pending, "Pending", &count, |b, ui| {
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
                let count = elab_count(state);
                self.section(ui, Section::Elabs, "Elaborations", &count, |b, ui| b.elaborations(ui, false));
            }

            self.nav.marked.retain(|id| state.can_trash(id));
            let count = (made.len() + done.len()).to_string();
            self.section(ui, Section::Done, "Done", &count, |b, ui| {
                if !made.is_empty() || !done.is_empty() {
                    b.select_all_row(ui, made.iter().map(|d| &d.id).chain(done.iter().map(|t| &t.id)));
                }
                b.nav.mark_groups.push(made.iter().map(|d| d.id.clone()).chain(done.iter().map(|t| t.id.clone())).collect());
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

    /// `right` records the cards into the right-hand focus list. One card per
    /// thread, its turns oldest first like a discussion.
    pub fn elaborations(&mut self, ui: &mut Ui, right: bool) {
        let state = self.state;
        self.nav.marked.retain(|id| state.can_trash(id));
        if state.elabs.is_empty() {
            ui.weak("No elaborations requested.");
            return;
        }
        let threads = state.threads();
        self.nav.mark_groups.push(threads.iter().map(|t| t[0].rid.clone()).collect());
        for thread in threads {
            let root = thread[0];
            let newest = thread[thread.len() - 1];
            let focus = Focus::Elab(root.rid.clone());
            if right {
                self.nav.right.push(focus.clone());
            } else {
                self.nav.left.push(focus.clone());
            }
            let open = self.nav.elab_is_open(state, &root.rid);
            let waiting = thread.iter().any(|r| r.answer.is_none());
            self.card(ui, &focus, |b, ui| {
                ui.horizontal(|ui| {
                    b.mark_checkbox(ui, &root.rid);
                    ui.label(RichText::new(&root.rid).monospace().weak());
                    ui.label(RichText::new(format!("on {}", root.target)).monospace().weak());
                    if thread.len() > 1 {
                        ui.label(RichText::new(format!("{} turns", thread.len())).small().weak());
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Zoom").on_hover_text("Open large (K · double-click)").clicked() {
                            b.nav.open_zoom(&root.rid);
                        }
                        let (label, hint) = if open { ("less", "Collapse (o)") } else { ("more", "Expand (o)") };
                        if ui.small_button(label).on_hover_text(hint).clicked() {
                            b.nav.elab_open.insert(root.rid.clone(), !open);
                        }
                        if waiting {
                            ui.weak("waiting");
                            ui.spinner();
                        }
                        ui.add(Label::new(RichText::new(item_title(state, &root.target)).weak()).truncate());
                    });
                });
                if !open {
                    ui.add(Label::new(RichText::new(format!("“{}”", root.selection)).italics()).truncate());
                    // The newest answer as a preview; clicking it expands the thread.
                    if let Some(a) = thread.iter().rev().find_map(|r| r.answer.as_deref()) {
                        let preview = ui.add(Label::new(preview_job(ui, a)).sense(Sense::click()));
                        if preview.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            b.nav.elab_open.insert(root.rid.clone(), true);
                        }
                    }
                    return;
                }
                if b.visual_on(&focus) {
                    ui.separator();
                    b.item_text(ui, &focus);
                } else {
                    for (i, r) in thread.iter().enumerate() {
                        if i > 0 {
                            ui.separator();
                        }
                        turn(ui, b.md, state, r);
                    }
                }
                if newest.answer.is_some() {
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        if ui.small_button("Follow up").on_hover_text("e · or v to select").clicked() {
                            b.nav.focus(focus.clone());
                            b.nav.follow_up(state, &focus);
                        }
                    });
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
                let buttons_w = 225.0;
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
                let buttons_w = 225.0;
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

    /// Select all / Clear / Trash selected, above the finished items.
    /// "Select all" above the finished items; acting on the selection
    /// happens in the action bar.
    fn select_all_row<'s>(&mut self, ui: &mut Ui, all: impl Iterator<Item = &'s String>) {
        if ui.small_button("Select all").on_hover_text("Select every finished item").clicked() {
            self.nav.marked.extend(all.cloned());
        }
        ui.add_space(2.0);
    }

    fn mark_checkbox(&mut self, ui: &mut Ui, id: &str) {
        // The checkbox flips its own copy; `click_mark` decides what changes.
        let mut marked = self.nav.marked.contains(id);
        let r = ui.checkbox(&mut marked, "").on_hover_text("Select (space) · shift-click selects a range");
        if r.clicked() {
            self.nav.click_mark(id, ui.input(|i| i.modifiers.shift));
        }
    }

    /// A resolved or withdrawn decision in the Done section.
    fn resolved_card(&mut self, ui: &mut Ui, d: &Decision) {
        let focus = Focus::Item(d.id.clone());
        self.nav.left.push(focus.clone());
        self.card(ui, &focus, |b, ui| {
            let selected = b.item_header(ui, &d.id, &focus);
            match &d.status {
                DecisionStatus::Resolved { text, .. } => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("»").weak());
                        ui.add(Label::new(RichText::new(text).strong()).wrap());
                    });
                }
                _ => {
                    ui.label(RichText::new("withdrawn by the agent").weak().italics());
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                b.elaborate_button(ui, &focus, selected);
                let resolved = matches!(d.status, DecisionStatus::Resolved { .. });
                if resolved && ui.small_button("Reopen").on_hover_text("x").clicked() {
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
        let markable = self.state.is_finished(id);
        ui.horizontal_top(|ui| {
            if markable {
                self.mark_checkbox(ui, id);
            }
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
        let right_click = ui.input(|i| i.pointer.secondary_pressed());
        let out = TextEdit::multiline(&mut buf)
            .frame(Frame::NONE)
            .margin(0.0)
            .desired_rows(1)
            .desired_width(width)
            .layouter(&mut layouter)
            .show(ui);
        let resp = &out.response.response;
        #[cfg(test)]
        self.nav.text_rects.push((focus.clone(), resp.rect));

        let mine = self.nav.mouse_selection.as_ref().filter(|m| &m.focus == focus);
        if let Some(m) = mine
            && ((right_click && resp.hovered()) || resp.context_menu_opened())
        {
            // egui moves the caret on any button press; undo that for
            // right-click so the menu can act on the selection.
            let mut state = out.state.clone();
            let (a, b) = m.range;
            state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
            state.store(ui.ctx(), resp.id);
        } else if resp.has_focus() {
            // The cursor range is only reported while the text has focus, so
            // the selection is remembered in `nav` for the button, Enter, and menu.
            let sel = out.cursor_range.and_then(|r| {
                let (a, b) = (r.primary.index.0, r.secondary.index.0);
                (a != b).then(|| MouseSelection {
                    focus: focus.clone(),
                    text: text.chars().skip(a.min(b)).take(a.abs_diff(b)).collect(),
                    range: (a.min(b), a.max(b)),
                })
            });
            match sel {
                Some(sel) => self.nav.mouse_selection = Some(sel),
                None if mine.is_some() => self.nav.mouse_selection = None,
                None => {}
            }
        }
        let selected = self.selection_for(focus);

        resp.context_menu(|ui| {
            if let Some(sel) = &selected
                && ui.button("Elaborate on selection").clicked()
            {
                self.elaborate(focus, Some(sel.clone()));
                ui.close();
            }
            if ui.button("Elaborate on whole item").clicked() {
                self.elaborate(focus, None);
                ui.close();
            }
            if let Some(sel) = &selected
                && ui.button("Copy").clicked()
            {
                ui.ctx().copy_text(sel.clone());
                ui.close();
            }
        });
        selected
    }

    fn selection_for(&self, focus: &Focus) -> Option<String> {
        self.nav.mouse_selection.as_ref().filter(|m| &m.focus == focus).map(|m| m.text.clone())
    }

    fn elaborate(&mut self, focus: &Focus, selection: Option<String>) {
        let selection = selection.unwrap_or_else(|| nav::item_text(self.state, focus));
        if let Some(target) = nav::elab_target(self.state, focus, &selection) {
            self.nav.focus(focus.clone());
            self.nav.open_prompt(target, selection.trim().to_owned());
        }
    }

    /// Elaborate and Zoom, the item card buttons every card has.
    fn elaborate_button(&mut self, ui: &mut Ui, focus: &Focus, selected: Option<String>) {
        let hover = match selected {
            Some(_) => "Elaborate on the selected text (Enter · right-click)",
            None => "Elaborate on the whole item, or select text first (e · v)",
        };
        if ui.small_button("Elaborate").on_hover_text(hover).clicked() {
            self.elaborate(focus, selected);
        }
        if let Focus::Item(id) = focus
            && ui.small_button("Zoom").on_hover_text("Open large (K)").clicked()
        {
            self.nav.open_zoom(id);
        }
    }

    fn visual_on(&self, focus: &Focus) -> bool {
        self.nav.visual.as_ref().is_some_and(|v| &v.target == focus)
    }

    /// A compact, clickable section row; the body renders only while open,
    /// so hidden items never enter the focus list.
    fn section(&mut self, ui: &mut Ui, s: Section, name: &str, count: &str, body: impl FnOnce(&mut Self, &mut Ui)) {
        let focus = Focus::Section(s);
        self.nav.left.push(focus.clone());
        let open = self.nav.is_open(s);
        let header = section_row(ui, name, count, Some(open));
        if header.clicked() {
            self.nav.set_open(s, !open);
            self.nav.cursor = Some(focus.clone());
        }
        if self.vim && self.nav.cursor.as_ref() == Some(&focus) {
            let stroke = Stroke::new(1.5, ui.visuals().selection.stroke.color);
            ui.painter().rect_stroke(header.rect, 4.0, stroke, egui::StrokeKind::Inside);
            if self.nav.scroll_to_cursor {
                header.scroll_to_me(Some(Align::Center));
            }
        }
        if open {
            ui.add_space(2.0);
            body(self, ui);
            ui.add_space(4.0);
        }
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
            if self.nav.mouse_selection.as_ref().is_some_and(|m| &m.focus != focus) {
                self.nav.mouse_selection = None;
            }
        }
        if let Focus::Elab(rid) = focus
            && ui.rect_contains_pointer(rect)
            && ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary))
        {
            self.nav.open_zoom(rid);
        }
        if focused && self.nav.scroll_to_cursor {
            ui.scroll_to_rect(rect, Some(Align::Center));
        }
        ui.add_space(4.0);
    }
}

/// "3" or "3 · 1 waiting".
pub fn elab_count(state: &State) -> String {
    let threads = state.threads().len();
    match state.elabs.iter().filter(|r| r.answer.is_none()).count() {
        0 => threads.to_string(),
        n => format!("{threads} · {n} waiting"),
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
            Weight::Weak | Weight::Struck => v.weak_text_color(),
        };
        let strikethrough = match weight {
            Weight::Struck => Stroke::new(1.0, color),
            _ => Stroke::NONE,
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
                job.append(&run, 0.0, TextFormat { font_id: font.clone(), color, background: run_bg, strikethrough, ..Default::default() });
                run.clear();
            }
            run_bg = bg;
            run.push(ch);
            idx += 1;
        }
        if !run.is_empty() {
            job.append(&run, 0.0, TextFormat { font_id: font.clone(), color, background: run_bg, strikethrough, ..Default::default() });
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
    ui.label(RichText::new(text).strong());
    ui.add_space(1.0);
}

/// A slim header row: fold triangle (when `open` is given), small uppercase
/// name, count on the right. Also used for the elaborations column title.
pub fn section_row(ui: &mut Ui, name: &str, count: &str, open: Option<bool>) -> egui::Response {
    let height = ui.text_style_height(&TextStyle::Body) + 8.0;
    let sense = if open.is_some() { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), sense);
    let v = ui.visuals().clone();
    if open.is_some() && resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, v.widgets.hovered.weak_bg_fill);
    }
    let mut x = rect.left() + 4.0;
    if let Some(open) = open {
        let icon = egui::Rect::from_center_size(egui::pos2(x + 5.0, rect.center().y), egui::vec2(10.0, 10.0));
        egui::collapsing_header::paint_default_icon(ui, if open { 1.0 } else { 0.0 }, &resp.clone().with_new_rect(icon));
        x += 16.0;
    }
    let font = egui::FontId::proportional(12.5);
    let painter = ui.painter();
    painter.text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, name.to_uppercase(), font.clone(), v.strong_text_color());
    painter.text(egui::pos2(rect.right() - 6.0, rect.center().y), egui::Align2::RIGHT_CENTER, count, font, v.weak_text_color());
    painter.hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color));
    resp
}

/// The first couple of lines of an answer as plain text, for collapsed cards.
fn preview_job(ui: &Ui, markdown: &str) -> LayoutJob {
    let plain: String = markdown
        .lines()
        .map(|l| l.trim().trim_start_matches(['#', '>', '-', '*', ' ']).replace(['*', '`'], ""))
        .filter(|l| !l.is_empty() && !l.starts_with("```"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut job = LayoutJob::simple(
        plain.chars().take(400).collect(),
        TextStyle::Body.resolve(ui.style()),
        ui.visuals().weak_text_color(),
        ui.available_width(),
    );
    job.wrap.max_rows = 2;
    job.wrap.overflow_character = Some('…');
    job
}

pub fn item_title<'s>(state: &'s State, id: &str) -> &'s str {
    state
        .decision(id)
        .map(|d| d.title.as_str())
        .or_else(|| state.todo(id).map(|t| t.title.as_str()))
        .unwrap_or("")
}

/// One turn of a thread: what the operator asked about, their note, and the
/// agent's answer. A follow-up on a whole answer doesn't repeat it.
pub fn turn(ui: &mut Ui, md: &mut CommonMarkCache, state: &State, r: &Elab) {
    let parent_answer = r.parent.as_deref().and_then(|p| state.elab(p)).and_then(|p| p.answer.as_deref());
    let whole = parent_answer.is_some_and(|a| a.trim() == r.selection.trim());
    ui.horizontal_wrapped(|ui| {
        if r.parent.is_some() {
            ui.label(RichText::new(&r.rid).monospace().weak());
        }
        if whole {
            ui.label(RichText::new("follow-up on the answer above").italics().weak());
        } else {
            ui.add(Label::new(RichText::new(format!("“{}”", r.selection)).italics()).wrap());
        }
    });
    if let Some(n) = &r.note {
        ui.add(Label::new(RichText::new(n).strong()).wrap());
    }
    match &r.answer {
        Some(a) => {
            ui.add_space(2.0);
            ui.push_id(&r.rid, |ui| CommonMarkViewer::new().show(ui, md, a));
        }
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("waiting for the agent's answer");
            });
        }
    }
}

/// An item in the zoom overlay: its title, status, context rendered as
/// markdown, options, and the elaboration threads about it.
pub fn item_zoom(ui: &mut Ui, md: &mut CommonMarkCache, state: &State, id: &str) {
    ui.add(Label::new(RichText::new(item_title(state, id)).heading()).wrap());
    let status = match (state.decision(id), state.todo(id)) {
        (Some(d), _) => match &d.status {
            DecisionStatus::Pending => "decision · pending".to_owned(),
            DecisionStatus::Resolved { text, .. } => format!("decision · resolved: {text}"),
            DecisionStatus::Withdrawn => "decision · withdrawn by the agent".to_owned(),
        },
        (_, Some(t)) => match &t.done {
            None => "todo · pending".to_owned(),
            Some(None) => "todo · done".to_owned(),
            Some(Some(n)) => format!("todo · done: {n}"),
        },
        _ => return,
    };
    ui.add(Label::new(RichText::new(status).weak()).wrap());
    let (context, options) = match (state.decision(id), state.todo(id)) {
        (Some(d), _) => (d.context.as_deref(), d.options.as_slice()),
        (_, Some(t)) => (t.context.as_deref(), &[][..]),
        _ => (None, &[][..]),
    };
    if let Some(c) = context {
        ui.add_space(6.0);
        ui.push_id(("ctx", id), |ui| CommonMarkViewer::new().show(ui, md, c));
    }
    if !options.is_empty() {
        ui.add_space(6.0);
        subheading(ui, "Options");
        for (i, o) in options.iter().enumerate() {
            ui.add(Label::new(format!("{}. {o}", i + 1)).wrap());
        }
    }
    let mut threads = state.threads();
    threads.retain(|t| t[0].target == id);
    if threads.is_empty() {
        return;
    }
    ui.add_space(6.0);
    subheading(ui, "Elaborations");
    for thread in threads.iter().rev() {
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            for (i, r) in thread.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                turn(ui, md, state, r);
            }
        });
    }
}
