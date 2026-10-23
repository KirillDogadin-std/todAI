mod board;
mod nav;
mod tab;
mod textnav;

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use egui::{Align, Key, KeyboardShortcut, Layout, Modifiers, Panel, RichText, TextEdit, TextStyle};
use egui_commonmark::CommonMarkCache;

use crate::config::{Config, InputMode, Theme};
use crate::store::{DecisionStatus, Kind};
use board::Board;
use nav::{FocusRequest, Mode};
use tab::Tab;

/// Width, in body-text characters, at which elaborations move into their own column.
const SPLIT_CHARS: f32 = 120.0;

/// Shown in the cheatsheet overlay; keep in sync with `nav.rs`.
const CHEATSHEET: &[(&str, &[(&str, &str)])] = &[
    (
        "Move",
        &[
            ("j k  ↓ ↑", "next / previous"),
            ("gg  G", "first / last"),
            ("[  ]", "previous / next section"),
            ("Ctrl+d  Ctrl+u", "5 down / up"),
            ("h  ←", "to section header, collapse; back to left column"),
            ("l  →", "expand section; to elaborations column"),
        ],
    ),
    (
        "Act",
        &[
            ("Enter  i  a", "answer item / toggle section / jump to elaborated item"),
            ("1-9", "pick option N, then Enter to submit"),
            ("x", "todo done ↔ undone; reopen decision"),
            ("e", "elaborate on the whole item"),
            ("v  V", "select text / select all (visual)"),
            ("za  space", "toggle section (zo / zc open / close)"),
            ("gt  gT", "next / previous tab"),
        ],
    ),
    (
        "Visual (selecting text)",
        &[
            ("h l  w b e", "char / word"),
            ("0  ^  $", "line start / first char / end"),
            ("j k", "row down / up"),
            ("gg  G", "text start / end"),
            ("v", "set / drop selection anchor"),
            ("o", "swap selection ends"),
            ("Enter", "elaborate on selection (word under caret if none)"),
            ("Esc", "cancel"),
        ],
    ),
    (
        "Typing (answers, notes, elaborate message)",
        &[("Enter", "submit"), ("Shift+Enter", "newline"), ("Esc", "stop typing / cancel elaborate")],
    ),
    (
        "Anywhere",
        &[
            ("?  F1", "this cheatsheet"),
            ("Ctrl+Shift+M", "vim ↔ mouse mode"),
            ("Ctrl+Shift+L", "dark ↔ light"),
            ("Ctrl+Tab  Ctrl+1-9", "switch tab"),
            ("Ctrl+W", "close tab"),
            ("Ctrl +  Ctrl -", "zoom (also moves the two-column breakpoint)"),
        ],
    ),
];

pub fn run(paths: Vec<PathBuf>) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("todai")
            .with_app_id("todai")
            .with_inner_size([900.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native("todai", options, Box::new(move |cc| Ok(Box::new(App::new(cc, paths)))))
        .map_err(|e| anyhow!("{e}"))
}

struct App {
    tabs: Vec<Tab>,
    active: usize,
    config: Config,
    md: CommonMarkCache,
    cheatsheet: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, paths: Vec<PathBuf>) -> App {
        let config = Config::load();
        apply_theme(&cc.egui_ctx, config.theme);
        App {
            tabs: paths.into_iter().map(|p| Tab::open(p, &cc.egui_ctx)).collect(),
            active: 0,
            config,
            md: CommonMarkCache::default(),
            cheatsheet: false,
        }
    }

    fn vim(&self) -> bool {
        self.config.input == InputMode::Vim
    }

    fn save_config(&mut self) {
        if let Err(e) = self.config.save()
            && let Some(t) = self.tabs.get_mut(self.active)
        {
            t.error = Some(format!("saving config: {e:#}"));
        }
    }

    fn toggle_theme(&mut self, ctx: &egui::Context) {
        self.config.theme = match self.config.theme {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::Dark,
        };
        apply_theme(ctx, self.config.theme);
        self.save_config();
    }

    fn toggle_input(&mut self) {
        self.config.input = match self.config.input {
            InputMode::Vim => InputMode::Mouse,
            InputMode::Mouse => InputMode::Vim,
        };
        self.save_config();
    }

    fn step_tab(&mut self, d: i32) {
        let n = self.tabs.len() as i32;
        if n > 0 {
            self.active = (self.active as i32 + d).rem_euclid(n) as usize;
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let shortcut = |m, k| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        if shortcut(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab) {
            self.step_tab(-1);
        } else if shortcut(Modifiers::COMMAND, Key::Tab) {
            self.step_tab(1);
        }
        let digits = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
        for (i, k) in digits.into_iter().enumerate() {
            if shortcut(Modifiers::COMMAND, k) && i < self.tabs.len() {
                self.active = i;
            }
        }
        if shortcut(Modifiers::COMMAND, Key::W) && !self.tabs.is_empty() {
            self.tabs.remove(self.active);
            self.active = self.active.min(self.tabs.len().saturating_sub(1));
        }
        if shortcut(Modifiers::COMMAND | Modifiers::SHIFT, Key::L) {
            self.toggle_theme(ctx);
        }
        if shortcut(Modifiers::COMMAND | Modifiers::SHIFT, Key::M) {
            self.toggle_input();
        }
        if shortcut(Modifiers::NONE, Key::F1) {
            self.cheatsheet = !self.cheatsheet;
        }
        if self.cheatsheet && shortcut(Modifiers::NONE, Key::Escape) {
            self.cheatsheet = false;
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let (mut theme, mut input, mut help) = (false, false, false);
        Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, t) in self.tabs.iter().enumerate() {
                    let pending = t.state.decisions.iter().filter(|d| d.status == DecisionStatus::Pending).count()
                        + t.state.todos.iter().filter(|t| t.done.is_none()).count();
                    let label = match pending {
                        0 => t.title(),
                        n => format!("{}  {n}", t.title()),
                    };
                    let r = ui.selectable_label(i == self.active, label);
                    if r.on_hover_text(t.log.path().display().to_string()).clicked() {
                        self.active = i;
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let label = match self.config.theme {
                        Theme::Dark => "Light",
                        Theme::Light => "Dark",
                    };
                    theme = ui.small_button(label).on_hover_text("Ctrl+Shift+L").clicked();
                    let label = match self.config.input {
                        InputMode::Vim => "vim",
                        InputMode::Mouse => "mouse",
                    };
                    input = ui
                        .small_button(label)
                        .on_hover_text("Input mode. Mouse works in both; vim adds single-key commands. Ctrl+Shift+M")
                        .clicked();
                    help = ui.small_button("?").on_hover_text("Keyboard cheatsheet (? or F1)").clicked();
                });
            });
        });
        if theme {
            self.toggle_theme(ui.ctx());
        }
        if input {
            self.toggle_input();
        }
        if help {
            self.cheatsheet = !self.cheatsheet;
        }
    }

    fn cheatsheet(&mut self, ctx: &egui::Context) {
        let mut open = self.cheatsheet;
        egui::Window::new("Keyboard")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                if !self.vim() {
                    ui.weak("Mouse mode: single-key commands are off. Ctrl+Shift+M to turn them on.");
                    ui.add_space(6.0);
                }
                ui.columns(2, |cols| {
                    for (i, (title, rows)) in CHEATSHEET.iter().enumerate() {
                        let ui = &mut cols[usize::from(i >= 2)];
                        ui.label(RichText::new(*title).strong());
                        egui::Grid::new(title).num_columns(2).spacing([16.0, 2.0]).show(ui, |ui| {
                            for (keys, what) in *rows {
                                ui.label(RichText::new(*keys).monospace());
                                ui.label(*what);
                                ui.end_row();
                            }
                        });
                        ui.add_space(8.0);
                    }
                });
            });
        self.cheatsheet = open;
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.shortcuts(&ctx);
        for t in &mut self.tabs {
            t.reload_if_dirty();
        }

        let vim = self.vim();
        let mut outcome = None;
        if let Some(tab) = self.tabs.get_mut(self.active) {
            outcome = Some(tab.nav.handle(&ctx, &tab.state, &mut tab.drafts, vim));
        }
        if let Some(o) = &outcome {
            if o.toggle_cheatsheet {
                self.cheatsheet = !self.cheatsheet;
            }
            if o.close_cheatsheet {
                self.cheatsheet = false;
            }
            self.step_tab(o.tab_step);
        }

        self.top_bar(ui);
        if self.cheatsheet {
            self.cheatsheet(&ctx);
        }

        let Some(tab) = self.tabs.get_mut(self.active) else {
            egui::CentralPanel::default_margins().show(ui, |ui| {
                ui.centered_and_justified(|ui| ui.weak("No files open. Run: todai FILE"));
            });
            return;
        };
        // Actions from a frame that also switched tabs belonged to the old tab; drop them.
        let actions = outcome.filter(|o| o.tab_step == 0).map(|o| o.kinds).unwrap_or_default();

        draw_tab(ui, tab, &mut self.md, vim, actions);
    }
}

/// One frame of a tab's board, after its keys were handled. Sends `actions`
/// plus whatever the widgets produced.
fn draw_tab(ui: &mut egui::Ui, tab: &mut Tab, md: &mut CommonMarkCache, vim: bool, mut actions: Vec<Kind>) {
    tab.nav.begin_render();
    status_bar(ui, tab, vim);
    prompt_panel(ui, tab, &mut actions);

    let font = TextStyle::Body.resolve(ui.style());
    let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&font, 'n'));
    let split = ui.available_width() >= SPLIT_CHARS * char_w;

    let mut board = Board { state: &tab.state, drafts: &mut tab.drafts, nav: &mut tab.nav, md, vim, out: Vec::new() };
    if split {
        let half = ui.available_width() / 2.0;
        Panel::right("elaborations").resizable(true).default_size(half).show(ui, |ui| {
            ui.add_space(4.0);
            ui.heading("Elaborations");
            ui.add_space(4.0);
            egui::ScrollArea::vertical().id_salt("right").auto_shrink(false).show(ui, |ui| board.elaborations(ui, true));
        });
    }
    egui::CentralPanel::default_margins().show(ui, |ui| board.left_column(ui, !split));
    actions.extend(board.out);
    tab.nav.end_render();

    for kind in actions {
        let draft_key = match &kind {
            Kind::DecisionResolve { id, .. } | Kind::TodoDone { id, .. } => Some(id.clone()),
            _ => None,
        };
        if tab.send(kind)
            && let Some(k) = draft_key
        {
            tab.drafts.remove(&k);
        }
    }
}

fn status_bar(ui: &mut egui::Ui, tab: &Tab, vim: bool) {
    Panel::bottom("status").show(ui, |ui| {
        ui.horizontal(|ui| {
            let mode = match (vim, tab.nav.mode) {
                (_, Mode::Insert) => "INSERT",
                (_, Mode::Prompt) => "ELABORATE",
                (true, Mode::Visual) => "VISUAL",
                (true, _) => "NORMAL",
                (false, _) => "MOUSE",
            };
            ui.label(RichText::new(mode).monospace().strong());
            if let Some(p) = tab.nav.prefix {
                ui.label(RichText::new(p.to_string()).monospace());
            }
            if let Some(err) = &tab.error {
                ui.colored_label(ui.visuals().error_fg_color, RichText::new(err).monospace());
            }
            if let Some(nav::MouseSelection { text: sel, .. }) = &tab.nav.mouse_selection {
                let short: String = sel.chars().take(40).collect();
                let more = if sel.chars().count() > 40 { "…" } else { "" };
                ui.label(format!("“{short}{more}”  Enter or right-click: elaborate · Esc: clear"));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.weak("? keys · Ctrl+Shift+M vim/mouse");
            });
        });
    });
}

/// The elaboration message editor, shown while a request is being written.
fn prompt_panel(ui: &mut egui::Ui, tab: &mut Tab, actions: &mut Vec<Kind>) {
    let state = &tab.state;
    let nav = &mut tab.nav;
    let Some(p) = nav.prompt.as_mut() else { return };
    let mut send = false;
    let mut cancel = false;
    Panel::bottom("prompt").show(ui, |ui| {
        ui.add_space(4.0);
        let title = state
            .decision(&p.target)
            .map(|d| d.title.as_str())
            .or_else(|| state.todo(&p.target).map(|t| t.title.as_str()))
            .unwrap_or("");
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Elaborate on").strong());
            ui.label(RichText::new(&p.target).monospace().weak());
            ui.weak(title);
        });
        let quote: String = p.selection.chars().take(300).collect();
        let ellipsis = if p.selection.chars().count() > 300 { "…" } else { "" };
        ui.label(RichText::new(format!("“{quote}{ellipsis}”")).italics());
        ui.horizontal(|ui| {
            let edit = TextEdit::multiline(&mut p.note)
                .desired_rows(2)
                .hint_text("message for the agent (optional) · Enter sends · Shift+Enter newline · Esc cancels")
                .desired_width(ui.available_width() - 130.0);
            let resp = ui.add(edit);
            if matches!(nav.focus_request, Some(FocusRequest::Prompt)) {
                resp.request_focus();
                nav.focus_request = None;
            }
            nav.prompt_focused = resp.has_focus();
            send = ui.button("Send").clicked();
            cancel = ui.button("Cancel").clicked();
        });
        ui.add_space(4.0);
    });
    if send && let Some(p) = nav.prompt.take() {
        let note = Some(p.note.trim().to_owned()).filter(|n| !n.is_empty());
        actions.push(Kind::ElabRequest { rid: String::new(), target: p.target, selection: p.selection, note });
    }
    if cancel {
        nav.prompt = None;
    }
}

fn apply_theme(ctx: &egui::Context, theme: Theme) {
    ctx.set_theme(match theme {
        Theme::Dark => egui::Theme::Dark,
        Theme::Light => egui::Theme::Light,
    });
}

#[cfg(test)]
mod tests;
