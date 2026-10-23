//! The board itself: the accordion of pending and done items, and the
//! elaboration list (a right column when wide, an accordion section when not).

use std::collections::HashMap;

use egui::{
    Button, CollapsingHeader, Frame, Key, KeyboardShortcut, Label, Modifiers, RichText, ScrollArea,
    TextEdit, Ui,
};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::store::{Decision, DecisionStatus, Kind, State, Todo};

const SUBMIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter);

/// Draws the left column and returns the operator events the user triggered.
pub fn left_column(
    ui: &mut Ui,
    state: &State,
    drafts: &mut HashMap<String, String>,
    md: &mut CommonMarkCache,
    with_elabs: bool,
) -> Vec<Kind> {
    let mut out = Vec::new();
    let pending: Vec<_> = state.decisions.iter().filter(|d| d.status == DecisionStatus::Pending).collect();
    let todos: Vec<_> = state.todos.iter().filter(|t| t.done.is_none()).collect();
    let made: Vec<_> = state
        .decisions
        .iter()
        .filter(|d| matches!(d.status, DecisionStatus::Resolved { .. }))
        .collect();
    let done: Vec<_> = state.todos.iter().filter(|t| t.done.is_some()).collect();

    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        section(ui, "pending", &format!("Pending  {}", pending.len() + todos.len()), true, |ui| {
            subheading(ui, "Decisions");
            if pending.is_empty() {
                ui.weak("Nothing to decide.");
            }
            for d in &pending {
                decision_card(ui, d, drafts, &mut out);
            }
            ui.add_space(8.0);
            subheading(ui, "Todos");
            if todos.is_empty() {
                ui.weak("Nothing to do by hand.");
            }
            for t in &todos {
                todo_card(ui, t, drafts, &mut out);
            }
        });

        if with_elabs {
            let waiting = state.elabs.iter().filter(|r| r.answer.is_none()).count();
            let title = match waiting {
                0 => format!("Elaborations  {}", state.elabs.len()),
                n => format!("Elaborations  {}  ({n} waiting)", state.elabs.len()),
            };
            section(ui, "elabs", &title, true, |ui| elaborations(ui, state, md));
        }

        section(ui, "done", &format!("Done  {}", made.len() + done.len()), false, |ui| {
            subheading(ui, "Decisions made");
            if made.is_empty() {
                ui.weak("None yet.");
            }
            for d in &made {
                resolved_card(ui, d, &mut out);
            }
            ui.add_space(8.0);
            subheading(ui, "Done");
            if done.is_empty() {
                ui.weak("None yet.");
            }
            for t in &done {
                done_row(ui, t, &mut out);
            }
        });
    });
    out
}

pub fn elaborations(ui: &mut Ui, state: &State, md: &mut CommonMarkCache) {
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
        card(ui, |ui| {
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
                Some(a) => {
                    ui.push_id(&r.rid, |ui| CommonMarkViewer::new().show(ui, md, a));
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

fn decision_card(ui: &mut Ui, d: &Decision, drafts: &mut HashMap<String, String>, out: &mut Vec<Kind>) {
    card(ui, |ui| {
        item_header(ui, &d.id, &d.title);
        if let Some(c) = &d.context {
            ui.add(Label::new(RichText::new(c).weak()).wrap());
        }
        let draft = drafts.entry(d.id.clone()).or_default();
        if !d.options.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for o in &d.options {
                    if ui.add(Button::new(o).small().selected(draft == o)).clicked() {
                        *draft = o.clone();
                    }
                }
            });
        }
        let edit_id = ui.make_persistent_id(("resolve", &d.id));
        // Consume before the TextEdit sees it, so Ctrl+Enter doesn't insert a newline.
        let submit = ui.memory(|m| m.has_focus(edit_id)) && ui.input_mut(|i| i.consume_shortcut(&SUBMIT));
        ui.horizontal(|ui| {
            let button_w = 80.0;
            ui.add(
                TextEdit::multiline(draft)
                    .id(edit_id)
                    .desired_rows(1)
                    .hint_text("resolution  (Ctrl+Enter)")
                    .desired_width(ui.available_width() - button_w),
            );
            let ready = !draft.trim().is_empty();
            if (ui.add_enabled(ready, Button::new("Resolve")).clicked() || submit) && ready {
                out.push(Kind::DecisionResolve { id: d.id.clone(), text: draft.trim().to_owned() });
            }
        });
    });
}

fn todo_card(ui: &mut Ui, t: &Todo, drafts: &mut HashMap<String, String>, out: &mut Vec<Kind>) {
    card(ui, |ui| {
        item_header(ui, &t.id, &t.title);
        if let Some(c) = &t.context {
            ui.add(Label::new(RichText::new(c).weak()).wrap());
        }
        let note = drafts.entry(t.id.clone()).or_default();
        let edit_id = ui.make_persistent_id(("note", &t.id));
        let submit = ui.memory(|m| m.has_focus(edit_id)) && ui.input_mut(|i| i.consume_shortcut(&SUBMIT));
        ui.horizontal(|ui| {
            let button_w = 80.0;
            ui.add(
                TextEdit::singleline(note)
                    .id(edit_id)
                    .hint_text("note for the agent (optional)")
                    .desired_width(ui.available_width() - button_w),
            );
            if ui.button("Done").clicked() || submit {
                let note = Some(note.trim().to_owned()).filter(|n| !n.is_empty());
                out.push(Kind::TodoDone { id: t.id.clone(), note });
            }
        });
    });
}

fn resolved_card(ui: &mut Ui, d: &Decision, out: &mut Vec<Kind>) {
    let DecisionStatus::Resolved { text, .. } = &d.status else { return };
    card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&d.id).monospace().weak());
            ui.add(Label::new(RichText::new(&d.title).weak()).wrap());
        });
        ui.horizontal(|ui| {
            ui.add(Label::new(RichText::new(text).strong()).wrap());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("reopen").clicked() {
                    out.push(Kind::DecisionReopen { id: d.id.clone() });
                }
            });
        });
    });
}

fn done_row(ui: &mut Ui, t: &Todo, out: &mut Vec<Kind>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("✔").weak());
        ui.label(RichText::new(&t.id).monospace().weak());
        ui.add(Label::new(&t.title).wrap());
        if let Some(Some(n)) = &t.done {
            ui.add(Label::new(RichText::new(format!("({n})")).weak()).wrap());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("undo").clicked() {
                out.push(Kind::TodoUndone { id: t.id.clone() });
            }
        });
    });
}

fn section(ui: &mut Ui, id: &str, title: &str, open: bool, body: impl FnOnce(&mut Ui)) {
    CollapsingHeader::new(RichText::new(title).heading())
        .id_salt(id)
        .default_open(open)
        .show(ui, body);
    ui.add_space(6.0);
}

fn subheading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).strong().size(15.0));
    ui.add_space(2.0);
}

fn item_header(ui: &mut Ui, id: &str, title: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(id).monospace().weak());
        ui.add(Label::new(RichText::new(title).strong()).wrap());
    });
}

fn card(ui: &mut Ui, body: impl FnOnce(&mut Ui)) {
    Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui);
    });
    ui.add_space(4.0);
}
