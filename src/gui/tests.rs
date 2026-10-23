//! Headless keyboard-flow tests: drive the real board with synthetic events
//! and check what lands in the log.

use egui::{Event, Key, Modifiers, Pos2, RawInput, Rect, vec2};
use egui_commonmark::CommonMarkCache;

use super::nav::{Focus, Mode, Section};
use super::tab::Tab;
use super::draw_tab;
use crate::store::{Actor, DecisionStatus, Kind, Log, State};

struct Harness {
    ctx: egui::Context,
    tab: Tab,
    md: CommonMarkCache,
    log: Log,
    vim: bool,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path().join("b.jsonl"));
        let agent = |k: Kind| log.append(Actor::Agent, |_| Ok(k)).unwrap();
        agent(Kind::DecisionAdd {
            id: "d1".into(),
            title: "Use egui or iced?".into(),
            context: None,
            options: vec!["egui".into(), "iced".into()],
        });
        agent(Kind::DecisionAdd { id: "d2".into(), title: "Name it?".into(), context: None, options: vec![] });
        agent(Kind::TodoAdd { id: "t1".into(), title: "Push repo".into(), context: None });
        let ctx = egui::Context::default();
        let tab = Tab::open(log.path().to_owned(), &ctx);
        let mut h = Harness { ctx, tab, md: CommonMarkCache::default(), log, vim: true, _dir: dir };
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(700.0, 900.0))),
            events,
            ..Default::default()
        };
        let (tab, md, vim) = (&mut self.tab, &mut self.md, self.vim);
        let mut events_once = true;
        let _ = self.ctx.run_ui(raw, |ui| {
            tab.reload_if_dirty();
            // Multi-pass frames re-run this closure; keys must apply once.
            let kinds = if std::mem::take(&mut events_once) {
                tab.nav.handle(ui.ctx(), &tab.state, &mut tab.drafts, vim).kinds
            } else {
                Vec::new()
            };
            draw_tab(ui, tab, md, vim, kinds);
        });
    }

    /// Each char is one frame, followed by an idle frame so focus settles.
    fn type_keys(&mut self, s: &str) {
        for c in s.chars() {
            self.frame(vec![Event::Text(c.to_string())]);
            self.frame(vec![]);
        }
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        let ev = |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
        self.frame(vec![ev(true), ev(false)]);
        self.frame(vec![]);
        self.frame(vec![]);
    }

    fn text(&mut self, s: &str) {
        self.frame(vec![Event::Text(s.into())]);
        self.frame(vec![]);
    }

    fn state(&self) -> State {
        self.log.load_state().unwrap().1
    }

    fn cursor(&self) -> Option<Focus> {
        self.tab.nav.cursor.clone()
    }

    fn move_to(&mut self, f: Focus) {
        for _ in 0..20 {
            if self.cursor() == Some(f.clone()) {
                return;
            }
            self.type_keys("j");
        }
        panic!("never reached {f:?}, at {:?}", self.cursor());
    }
}

fn item(id: &str) -> Focus {
    Focus::Item(id.into())
}

#[test]
fn j_k_walk_sections_and_items() {
    let mut h = Harness::new();
    // The cursor starts on the first row.
    assert_eq!(h.cursor(), Some(Focus::Section(Section::Pending)));
    h.type_keys("j");
    assert_eq!(h.cursor(), Some(item("d1")));
    h.type_keys("jj");
    assert_eq!(h.cursor(), Some(item("t1")));
    h.type_keys("k");
    assert_eq!(h.cursor(), Some(item("d2")));
    h.type_keys("gg");
    assert_eq!(h.cursor(), Some(Focus::Section(Section::Pending)));
    h.type_keys("G");
    assert_eq!(h.cursor(), Some(Focus::Section(Section::Done)));
}

#[test]
fn option_then_enter_resolves() {
    let mut h = Harness::new();
    h.move_to(item("d1"));
    h.type_keys("2");
    assert_eq!(h.tab.nav.mode, Mode::Insert);
    h.key(Key::Enter, Modifiers::NONE);
    let s = h.state();
    assert!(matches!(&s.decision("d1").unwrap().status, DecisionStatus::Resolved { text, .. } if text == "iced"));
    assert_eq!(h.tab.nav.mode, Mode::Normal);
}

#[test]
fn insert_typing_and_escape_keeps_draft() {
    let mut h = Harness::new();
    h.move_to(item("d2"));
    h.key(Key::Enter, Modifiers::NONE);
    assert_eq!(h.tab.nav.mode, Mode::Insert);
    h.text("todai");
    h.key(Key::Escape, Modifiers::NONE);
    assert_eq!(h.tab.nav.mode, Mode::Normal);
    assert_eq!(h.tab.drafts.get("d2").map(String::as_str), Some("todai"));
    // Back in normal mode letters are commands again, not text.
    h.type_keys("k");
    assert_eq!(h.cursor(), Some(item("d1")));
    assert_eq!(h.tab.drafts.get("d2").map(String::as_str), Some("todai"));

    h.move_to(item("d2"));
    h.type_keys("i");
    h.key(Key::Enter, Modifiers::NONE);
    assert!(matches!(&h.state().decision("d2").unwrap().status, DecisionStatus::Resolved { text, .. } if text == "todai"));
}

#[test]
fn x_marks_todo_done_and_back() {
    let mut h = Harness::new();
    h.move_to(item("t1"));
    h.type_keys("x");
    assert!(h.state().todo("t1").unwrap().done.is_some());
    // The todo moved into the collapsed Done section; open it and undo there.
    h.type_keys("G");
    h.type_keys("l");
    h.move_to(item("t1"));
    h.type_keys("x");
    assert!(h.state().todo("t1").unwrap().done.is_none());
}

#[test]
fn visual_selection_elaborate_with_message() {
    let mut h = Harness::new();
    h.move_to(item("d1"));
    h.type_keys("w"); // not a normal-mode command: nothing happens
    h.type_keys("v");
    assert_eq!(h.tab.nav.mode, Mode::Visual);
    h.type_keys("w"); // caret to "egui"
    h.type_keys("v"); // anchor
    h.type_keys("e"); // end of "egui"
    h.key(Key::Enter, Modifiers::NONE);
    assert_eq!(h.tab.nav.mode, Mode::Prompt);
    assert_eq!(h.tab.nav.prompt.as_ref().unwrap().selection, "egui");
    h.text("what is it?");
    h.key(Key::Enter, Modifiers::NONE);

    let s = h.state();
    let r = s.elab("r1").expect("request sent");
    assert_eq!((r.target.as_str(), r.selection.as_str(), r.note.as_deref()), ("d1", "egui", Some("what is it?")));
    assert_eq!(h.tab.nav.mode, Mode::Normal);
}

#[test]
fn e_elaborates_whole_item_and_esc_cancels() {
    let mut h = Harness::new();
    h.move_to(item("d2"));
    h.type_keys("e");
    assert_eq!(h.tab.nav.prompt.as_ref().unwrap().selection, "Name it?");
    h.key(Key::Escape, Modifiers::NONE);
    assert!(h.tab.nav.prompt.is_none());
    assert!(h.state().elabs.is_empty());
}

#[test]
fn h_l_collapse_and_expand_sections() {
    let mut h = Harness::new();
    h.move_to(item("d1"));
    h.type_keys("h");
    assert_eq!(h.cursor(), Some(Focus::Section(Section::Pending)));
    h.type_keys("h");
    assert!(!h.tab.nav.is_open(Section::Pending));
    h.type_keys("j");
    assert_ne!(h.cursor(), Some(item("d1")), "collapsed items are skipped");
    h.type_keys("k");
    h.type_keys("l");
    assert!(h.tab.nav.is_open(Section::Pending));
}

#[test]
fn mouse_mode_ignores_letters() {
    let mut h = Harness::new();
    h.vim = false;
    h.type_keys("jjx");
    assert_eq!(h.cursor(), Some(Focus::Section(Section::Pending)));
    assert!(h.state().todo("t1").unwrap().done.is_none());
}
