mod board;
mod nav;
mod tab;
mod textnav;

use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use anyhow::{Result, anyhow};
use egui::{Align, Key, KeyboardShortcut, Layout, Modifiers, Panel, RichText, TextEdit, TextStyle};
use egui_commonmark::CommonMarkCache;

use crate::config::{Config, InputMode, Theme};
use crate::ipc::{self, Incoming, Reply, Request};
use crate::session::Session;
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
            ("j k  Down Up", "next / previous"),
            ("gg  G", "first / last"),
            ("[  ]", "previous / next section"),
            ("Ctrl+d  Ctrl+u", "5 down / up"),
            ("h  Left", "to section header, collapse; back to left column"),
            ("l  Right", "expand section; to elaborations column"),
        ],
    ),
    (
        "Act",
        &[
            ("Enter  i  a", "answer item / toggle section / jump to elaborated item"),
            ("1-9", "pick option N, then Enter to submit"),
            ("x", "todo done / undone; reopen decision"),
            ("space", "mark / unmark a finished item or elaboration (multi-select)"),
            ("d", "remove marked items, or this one, from the board"),
            ("o  za", "on an elaboration: expand / collapse it"),
            ("zR  zM", "expand / collapse all elaborations (or the selected ones)"),
            ("Esc", "clear the selection"),
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
            ("Ctrl+Shift+M", "vim / mouse mode"),
            ("Ctrl+Shift+L", "dark / light"),
            ("Ctrl+Tab  Ctrl+1-9", "switch tab"),
            ("Ctrl+W", "close tab"),
            ("Ctrl+Q", "close window (asks first; tabs are restored next time)"),
            ("Ctrl +  Ctrl -", "zoom (also moves the two-column breakpoint)"),
        ],
    ),
];

/// Runs the window until it is closed. `listener` is the already-bound
/// single-instance socket; it is removed on exit.
pub fn run(listener: UnixListener, socket: PathBuf, paths: Vec<PathBuf>) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("todai")
            .with_app_id("todai")
            .with_inner_size([900.0, 700.0]),
        ..Default::default()
    };
    let result =
        eframe::run_native("todai", options, Box::new(move |cc| Ok(Box::new(App::new(cc, listener, paths)))))
            .map_err(|e| anyhow!("{e}"));
    let _ = std::fs::remove_file(&socket);
    result
}

struct App {
    tabs: Vec<Tab>,
    active: usize,
    config: Config,
    md: CommonMarkCache,
    cheatsheet: bool,
    confirm_quit: bool,
    /// Set once the user confirmed, so our own Close isn't intercepted.
    closing: bool,
    requests: mpsc::Receiver<Incoming>,
    saved: Session,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, listener: UnixListener, paths: Vec<PathBuf>) -> App {
        let config = Config::load();
        apply_theme(&cc.egui_ctx, config.theme);
        let (tx, requests) = mpsc::channel();
        let ctx = cc.egui_ctx.clone();
        ipc::serve(listener, tx, move || ctx.request_repaint());

        let saved = Session::load();
        let mut app = App {
            tabs: Vec::new(),
            active: 0,
            config,
            md: CommonMarkCache::default(),
            cheatsheet: false,
            confirm_quit: false,
            closing: false,
            requests,
            saved: saved.clone(),
        };
        // Restore last session, skipping boards that were deleted since.
        for p in saved.tabs.iter().filter(|p| p.exists()) {
            app.open_or_focus(&cc.egui_ctx, p);
        }
        app.active = saved.active.min(app.tabs.len().saturating_sub(1));
        for p in &paths {
            app.open_or_focus(&cc.egui_ctx, p);
        }
        app
    }

    fn open_or_focus(&mut self, ctx: &egui::Context, path: &Path) {
        match self.tabs.iter().position(|t| t.log.path() == path) {
            Some(i) => self.active = i,
            None => {
                self.tabs.push(Tab::open(path.to_owned(), ctx));
                self.active = self.tabs.len() - 1;
            }
        }
    }

    fn serve_requests(&mut self, ctx: &egui::Context) {
        while let Ok((req, reply)) = self.requests.try_recv() {
            let r = match req {
                Request::Open { path } => {
                    self.open_or_focus(ctx, &path);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    Reply::ok()
                }
                Request::Focus => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    Reply::ok()
                }
                Request::List => Reply {
                    ok: true,
                    tabs: self.tabs.iter().map(|t| t.log.path().to_owned()).collect(),
                    active: self.active,
                    error: None,
                },
            };
            let _ = reply.send(r);
        }
    }

    /// Desktop notification for agent posts that arrived while the window
    /// is unfocused. Silently does nothing without `notify-send`.
    fn notify(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.viewport().focused) == Some(true);
        for t in &mut self.tabs {
            let posts = std::mem::take(&mut t.new_posts);
            if posts.is_empty() || focused {
                continue;
            }
            let mut body: Vec<String> = posts.iter().take(5).cloned().collect();
            if posts.len() > 5 {
                body.push(format!("and {} more", posts.len() - 5));
            }
            let summary = format!("todai · {}", t.title());
            let child = std::process::Command::new("notify-send")
                .args(["--app-name=todai", &summary, &body.join("\n")])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            // Reap it off-thread so no zombie is left behind.
            if let Ok(mut c) = child {
                std::thread::spawn(move || c.wait());
            }
        }
    }

    fn save_session(&mut self) {
        let now = Session { tabs: self.tabs.iter().map(|t| t.log.path().to_owned()).collect(), active: self.active };
        if now != self.saved {
            if let Err(e) = now.save()
                && let Some(t) = self.tabs.get_mut(self.active)
            {
                t.error = Some(format!("saving session: {e:#}"));
            }
            self.saved = now;
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
        if shortcut(Modifiers::COMMAND, Key::W) {
            self.close_tab(self.active);
        }
        if shortcut(Modifiers::COMMAND, Key::Q) {
            self.confirm_quit = true;
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

    fn close_tab(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.tabs.remove(i);
            self.active = active_after_close(self.active, i, self.tabs.len());
        }
    }

    /// Confirmation for closing the window. Returns true while it is shown;
    /// it takes all keys so nothing reaches the board underneath.
    fn quit_dialog(&mut self, ctx: &egui::Context) -> bool {
        if !self.confirm_quit {
            return false;
        }
        let body = "Open tabs come back the next time you run todai.";
        match confirm(ctx, "quit", "Close todai?", body, "Close window") {
            Some(true) => {
                self.closing = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Some(false) => self.confirm_quit = false,
            None => {}
        }
        true
    }

    fn trash_dialog(&mut self, ctx: &egui::Context) -> bool {
        let Some(tab) = self.tabs.get_mut(self.active) else { return false };
        let Some(ids) = tab.confirm_trash.clone() else { return false };
        let title = match ids.len() {
            1 => format!("Remove {} from the board?", ids[0]),
            n => format!("Remove {n} items from the board?"),
        };
        let body = format!("{}\nThey are gone from the board for good; the agent is not told.", ids.join(", "));
        match confirm(ctx, "trash", &title, &body, "Remove") {
            Some(true) => {
                tab.confirm_trash = None;
                if tab.send(Kind::Trash { ids }) {
                    tab.nav.marked.clear();
                }
            }
            Some(false) => tab.confirm_trash = None,
            None => {}
        }
        true
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let (mut theme, mut input, mut help, mut close) = (false, false, false, None);
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
                    let x = ui.add(egui::Button::new(RichText::new("×").weak()).small().frame(false));
                    if x.on_hover_text("Close tab (Ctrl+W)").clicked() {
                        close = Some(i);
                    }
                    ui.add_space(6.0);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("Quit").on_hover_text("Close the window (Ctrl+Q)").clicked() {
                        self.confirm_quit = true;
                    }
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
        if let Some(i) = close {
            self.close_tab(i);
        }
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
        let screen = ctx.content_rect();
        let font = egui::FontId::monospace(13.0);
        let char_w = ctx.fonts_mut(|f| f.glyph_width(&font, 'n'));
        let key_w = 20.0 * char_w;
        let width = (screen.width() - 32.0).clamp(240.0, 1000.0);
        // Two columns only when each still fits the key column plus a readable description.
        let columns = if width >= 2.0 * (key_w + 40.0 * char_w) { 2 } else { 1 };

        let mut open = self.cheatsheet;
        egui::Window::new("Keyboard")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size([width, (screen.height() - 80.0).max(120.0)])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    if !self.vim() {
                        ui.add(
                            egui::Label::new(
                                RichText::new("Mouse mode: single-key commands are off. Ctrl+Shift+M turns them on.")
                                    .weak(),
                            )
                            .wrap(),
                        );
                        ui.add_space(6.0);
                    }
                    let section = |ui: &mut egui::Ui, title: &str, rows: &[(&str, &str)]| {
                        ui.label(RichText::new(title).strong());
                        for (keys, what) in rows {
                            ui.horizontal_top(|ui| {
                                ui.add_sized(
                                    [key_w, ui.spacing().interact_size.y],
                                    egui::Label::new(RichText::new(*keys).font(font.clone())).halign(Align::LEFT),
                                );
                                ui.add(egui::Label::new(*what).wrap());
                            });
                        }
                        ui.add_space(8.0);
                    };
                    if columns == 2 {
                        ui.columns(2, |cols| {
                            for (i, (title, rows)) in CHEATSHEET.iter().enumerate() {
                                section(&mut cols[usize::from(i >= 2)], title, rows);
                            }
                        });
                    } else {
                        for (title, rows) in CHEATSHEET {
                            section(ui, title, rows);
                        }
                    }
                });
            });
        self.cheatsheet = open;
    }
}

/// Index of the active tab after closing tab `closed`, given the count left.
fn active_after_close(active: usize, closed: usize, remaining: usize) -> usize {
    if closed < active || active >= remaining { active.saturating_sub(1) } else { active }
}

impl eframe::App for App {
    // Runs even while the window is hidden, so socket requests never stall.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // A close from the compositor (or the title bar) asks first, like Quit.
        if ctx.input(|i| i.viewport().close_requested()) && !self.closing {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_quit = true;
        }
        self.serve_requests(ctx);
        for t in &mut self.tabs {
            t.reload_if_dirty();
        }
        self.notify(ctx);
        self.save_session();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.shortcuts(&ctx);

        let vim = self.vim();
        let dialog = self.quit_dialog(&ctx) || self.trash_dialog(&ctx);
        let mut outcome = None;
        if !dialog && let Some(tab) = self.tabs.get_mut(self.active) {
            outcome = Some(tab.nav.handle(&ctx, &tab.state, &mut tab.drafts, vim));
        }
        if let Some(o) = &outcome {
            if o.toggle_cheatsheet {
                self.cheatsheet = !self.cheatsheet;
            }
            if o.close_cheatsheet {
                self.cheatsheet = false;
            }
            if let (Some(ids), Some(tab)) = (&o.trash, self.tabs.get_mut(self.active)) {
                tab.confirm_trash = Some(ids.clone());
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
    action_bar(ui, tab);

    let font = TextStyle::Body.resolve(ui.style());
    let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&font, 'n'));
    let split = ui.available_width() >= SPLIT_CHARS * char_w;

    let mut board =
        Board { state: &tab.state, drafts: &mut tab.drafts, nav: &mut tab.nav, md, vim, out: Vec::new() };
    if split {
        let half = ui.available_width() / 2.0;
        Panel::right("elaborations").resizable(true).default_size(half).show(ui, |ui| {
            ui.add_space(2.0);
            board::section_row(ui, "Elaborations", &board::elab_count(board.state), None);
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

/// What you can do with the selection; shown only while something is selected.
fn action_bar(ui: &mut egui::Ui, tab: &mut Tab) {
    let n = tab.nav.marked.len();
    if n == 0 {
        return;
    }
    let elabs = tab.nav.marked.iter().filter(|id| tab.state.elab(id).is_some()).count();
    Panel::bottom("actions").show(ui, |ui| {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{n} selected")).strong());
            ui.separator();
            if ui.button("🗑 Delete").on_hover_text("Remove from the board (d)").clicked() {
                tab.confirm_trash = Some(tab.nav.marked.iter().cloned().collect());
            }
            let fold_hint = |what: &str| format!("{what} the selected elaborations (z{})", if what == "Expand" { 'R' } else { 'M' });
            if ui.add_enabled(elabs > 0, egui::Button::new("Expand")).on_hover_text(fold_hint("Expand")).clicked() {
                tab.nav.fold_elabs(&tab.state, true);
            }
            if ui.add_enabled(elabs > 0, egui::Button::new("Collapse")).on_hover_text(fold_hint("Collapse")).clicked() {
                tab.nav.fold_elabs(&tab.state, false);
            }
            if ui.button("Clear").on_hover_text("Clear the selection (Esc)").clicked() {
                tab.nav.marked.clear();
            }
        });
        ui.add_space(2.0);
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

/// A yes/no dialog that owns the keyboard while shown: `Enter`/`y` confirm,
/// `Esc`/`n` cancel. Returns `None` while undecided.
fn confirm(ctx: &egui::Context, id: &str, title: &str, body: &str, yes_label: &str) -> Option<bool> {
    let (mut yes, mut no) = (false, false);
    ctx.input_mut(|i| {
        i.events.retain(|ev| match ev {
            egui::Event::Key { key, pressed, .. } => {
                yes |= *pressed && *key == Key::Enter;
                no |= *pressed && *key == Key::Escape;
                false
            }
            egui::Event::Text(t) => {
                yes |= t == "y";
                no |= t == "n";
                false
            }
            _ => true,
        });
    });
    let modal = egui::Modal::new(egui::Id::new(id)).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.heading(title);
        ui.add_space(4.0);
        ui.add(egui::Label::new(body).wrap());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            yes |= ui.button(yes_label).clicked();
            no |= ui.button("Cancel").clicked();
            ui.weak("Enter / y · Esc / n");
        });
    });
    if yes {
        Some(true)
    } else if no || modal.should_close() {
        Some(false)
    } else {
        None
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
