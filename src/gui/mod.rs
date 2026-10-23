mod board;
mod tab;

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use egui::{Key, KeyboardShortcut, Modifiers, Panel, RichText, TextStyle};
use egui_commonmark::CommonMarkCache;

use crate::config::{Config, Theme};
use crate::store::Kind;
use tab::Tab;

/// Width, in body-text characters, at which elaborations move into their own column.
const SPLIT_CHARS: f32 = 240.0;

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
        }
    }

    fn toggle_theme(&mut self, ctx: &egui::Context) {
        self.config.theme = match self.config.theme {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::Dark,
        };
        apply_theme(ctx, self.config.theme);
        if let Err(e) = self.config.save()
            && let Some(t) = self.tabs.get_mut(self.active)
        {
            t.error = Some(format!("saving config: {e:#}"));
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let shortcut = |m, k| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        let n = self.tabs.len();
        if n > 0 {
            if shortcut(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab) {
                self.active = (self.active + n - 1) % n;
            } else if shortcut(Modifiers::COMMAND, Key::Tab) {
                self.active = (self.active + 1) % n;
            }
            let digits = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
            for (i, k) in digits.into_iter().enumerate() {
                if shortcut(Modifiers::COMMAND, k) && i < n {
                    self.active = i;
                }
            }
            if shortcut(Modifiers::COMMAND, Key::W) {
                self.tabs.remove(self.active);
                self.active = self.active.min(self.tabs.len().saturating_sub(1));
            }
        }
        if shortcut(Modifiers::COMMAND | Modifiers::SHIFT, Key::L) {
            self.toggle_theme(ctx);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.shortcuts(&ctx);
        for t in &mut self.tabs {
            t.reload_if_dirty();
        }

        let mut toggle = false;
        Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, t) in self.tabs.iter().enumerate() {
                    let pending = t.state.decisions.iter().filter(|d| d.status == crate::store::DecisionStatus::Pending).count()
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
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = match self.config.theme {
                        Theme::Dark => "Light",
                        Theme::Light => "Dark",
                    };
                    toggle = ui.small_button(label).on_hover_text("Ctrl+Shift+L").clicked();
                });
            });
        });
        if toggle {
            self.toggle_theme(&ctx);
        }

        let Some(tab) = self.tabs.get_mut(self.active) else {
            egui::CentralPanel::default_margins().show(ui, |ui| {
                ui.centered_and_justified(|ui| ui.weak("No files open. Run: todai FILE"));
            });
            return;
        };

        if let Some(err) = &tab.error {
            Panel::bottom("status").show(ui, |ui| {
                ui.colored_label(ui.visuals().error_fg_color, RichText::new(err).monospace());
            });
        }

        let font = TextStyle::Body.resolve(ui.style());
        let char_w = ctx.fonts_mut(|f| f.glyph_width(&font, 'n'));
        let split = ui.available_width() >= SPLIT_CHARS * char_w;

        if split {
            let half = ui.available_width() / 2.0;
            Panel::right("elaborations").resizable(true).default_size(half).show(ui, |ui| {
                ui.add_space(4.0);
                ui.heading("Elaborations");
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| board::elaborations(ui, &tab.state, &mut self.md));
            });
        }
        let actions = egui::CentralPanel::default_margins()
            .show(ui, |ui| board::left_column(ui, &tab.state, &mut tab.drafts, &mut self.md, !split))
            .inner;

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
}

fn apply_theme(ctx: &egui::Context, theme: Theme) {
    ctx.set_theme(match theme {
        Theme::Dark => egui::Theme::Dark,
        Theme::Light => egui::Theme::Light,
    });
}
