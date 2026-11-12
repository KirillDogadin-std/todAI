//! The board list overlay ("tracked boards"): drawing and the window's side
//! of its actions. State and keys are `todai_client::boardlist`, shared with
//! the TUI.

use egui::{Align, Color32, Layout, RichText};
use todai_client::boardlist::{Action, BoardList, Row, Sort, ago, now_ms};

/// What the list asked the window for, from a click (keys come through
/// `input::handle_boards` as the same [`Action`]s).
pub fn show(ctx: &egui::Context, list: &mut BoardList, vim: bool) -> Option<Action> {
    let screen = ctx.content_rect();
    let width = (screen.width() - 32.0).clamp(300.0, 820.0);
    let mut action = None;
    let mut open = true;
    egui::Window::new("Boards")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 40.0])
        .fixed_size([width, (screen.height() - 100.0).max(160.0)])
        .show(ctx, |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Tracked boards: the ones todai opened or an agent attached. Boards never opened through todai aren't listed.",
                    )
                    .weak(),
                )
                .wrap(),
            );
            if let Some(n) = &list.notice {
                ui.add(egui::Label::new(RichText::new(n).color(ui.visuals().hyperlink_color)).wrap());
            }
            for n in list.remote_notes() {
                ui.add(egui::Label::new(RichText::new(n).weak()).wrap());
            }
            ui.add_space(4.0);
            toolbar(ui, list, vim, &mut action);
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| rows(ui, list, &mut action));
        });
    if !open {
        action = Some(Action::Close);
    }
    action
}

fn toolbar(ui: &mut egui::Ui, list: &mut BoardList, vim: bool, action: &mut Option<Action>) {
    ui.horizontal_wrapped(|ui| {
        if let Some((next_to, name)) = &list.naming {
            ui.label(format!("New board in {}:", next_to.place()));
            ui.label(RichText::new(format!("{name}▏")).monospace().strong());
            ui.weak("Enter create · Esc cancel");
            return;
        }
        let filter = if list.filter.is_empty() && !list.filtering {
            if vim { "/ to filter".to_owned() } else { "type to filter".to_owned() }
        } else {
            format!("filter: {}▏", list.filter)
        };
        ui.label(RichText::new(filter).monospace());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let stale = list.stale();
            let label = format!("Archive stale ({})", stale.len());
            if ui.add_enabled(!stale.is_empty(), egui::Button::new(label).small()).on_hover_text("S").clicked() {
                *action = Some(Action::ArchiveStale(stale));
            }
            let sort = match list.sort {
                Sort::Project => "by project",
                Sort::Activity => "by activity",
            };
            if ui.small_button(sort).on_hover_text("Sort (s)").clicked() {
                list.input(&todai_client::nav::Input::Text("s".into()), true);
            }
            let archived = if list.show_archived { "hide archived" } else { "show archived" };
            if ui.small_button(archived).on_hover_text(".").clicked() {
                list.input(&todai_client::nav::Input::Text(".".into()), true);
            }
            if ui.small_button("New").on_hover_text("n: a board in the selected board's project").clicked() {
                list.start_new();
            }
            let targets = list.targets();
            if !list.marked.is_empty() {
                if ui.small_button(format!("Delete {}", targets.len())).on_hover_text("d").clicked() {
                    *action = Some(Action::Delete(targets.clone()));
                }
                if ui.small_button(format!("Archive {}", targets.len())).on_hover_text("a").clicked() {
                    *action = Some(Action::ToggleArchive(targets));
                }
            }
        });
    });
}

fn rows(ui: &mut egui::Ui, list: &mut BoardList, action: &mut Option<Action>) {
    let visible: Vec<Row> = list.visible().into_iter().cloned().collect();
    if visible.is_empty() {
        ui.weak(if list.rows.is_empty() {
            "No tracked boards yet. They appear once an agent runs `todai attach`, or you run `todai FILE`."
        } else {
            "No board matches."
        });
        return;
    }
    let now = now_ms();
    let mut project: Option<String> = None;
    for (i, row) in visible.iter().enumerate() {
        let s = row;
        if list.sort == Sort::Project && project.as_deref() != Some(row.place().as_str()) {
            project = Some(row.place());
            ui.add_space(if i == 0 { 0.0 } else { 6.0 });
            ui.label(RichText::new(row.place()).strong());
        }
        let cursor = i == list.cursor;
        let frame = egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 2)).corner_radius(4.0);
        let frame = if cursor { frame.fill(ui.visuals().selection.bg_fill.gamma_multiply(0.35)) } else { frame };
        let r = frame
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut marked = list.marked.contains(&s.source);
                    if ui.checkbox(&mut marked, "").changed() {
                        list.toggle_mark(&s.source);
                    }
                    let mut name = RichText::new(&s.name);
                    if row.open {
                        name = name.strong();
                    }
                    if row.archived {
                        name = name.weak();
                    }
                    if list.sort == Sort::Activity {
                        ui.weak(format!("{} /", s.place()));
                    }
                    let n = ui.add(egui::Label::new(name).sense(egui::Sense::click()));
                    if n.on_hover_text(s.location().label()).clicked() {
                        *action = Some(Action::Open(s.location()));
                    }
                    if s.waiting > 0 {
                        ui.label(RichText::new(format!("{} waiting", s.waiting)).color(ui.visuals().warn_fg_color));
                    }
                    if s.asked > 0 {
                        ui.weak(format!("{} asked", s.asked));
                    }
                    for (on, badge) in [(row.open, "open"), (row.stale, "stale"), (row.archived, "archived")] {
                        if on {
                            ui.label(RichText::new(badge).small().color(Color32::GRAY));
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // A server only archives: no delete for its boards.
                        if s.host.is_none() && ui.small_button("Delete").clicked() {
                            *action = Some(Action::Delete(vec![s.clone()]));
                        }
                        let label = if row.archived { "Unarchive" } else { "Archive" };
                        if ui.small_button(label).clicked() {
                            *action = Some(Action::ToggleArchive(vec![s.clone()]));
                        }
                        ui.weak(ago(now, s.last));
                    });
                });
            })
            .response;
        if cursor {
            r.scroll_to_me(None);
        }
    }
}
