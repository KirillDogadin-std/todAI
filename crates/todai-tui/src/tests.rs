//! Headless tests: drive the real app with terminal events, render into a
//! `TestBackend`, and check the board file and the screen.

use std::sync::Arc;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use todai_client::nav::{Focus, Mode, Section};
use todai_client::store::{Actor, DecisionStatus, Kind, Log, State};

use crate::app::App;
use crate::ui;

struct Harness {
    app: App,
    term: Terminal<TestBackend>,
    log: Log,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn new(width: u16, height: u16) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path().join("b.jsonl"));
        let agent = |k: Kind| log.append(Actor::Agent, |_| Ok(k)).unwrap();
        agent(Kind::DecisionAdd {
            id: "d1".into(),
            title: "Use egui or iced?".into(),
            context: Some("We need **low** memory.".into()),
            options: vec!["egui".into(), "iced".into()],
        });
        agent(Kind::DecisionAdd { id: "d2".into(), title: "Name it?".into(), context: None, options: vec![] });
        agent(Kind::TodoAdd { id: "t1".into(), title: "Plug in the keyboard".into(), context: None });
        let mut app = App::new(Arc::new(|| {}), None, false);
        app.open_file(log.path().to_owned());
        let term = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut h = Harness { app, term, log, _dir: dir };
        h.frame();
        h
    }

    fn frame(&mut self) {
        for o in &mut self.app.tabs {
            o.tab.reload_if_dirty_now();
        }
        self.app.tick();
        let app = &mut self.app;
        self.term.draw(|f| ui::draw(f, app)).unwrap();
    }

    /// Each event is followed by a frame, as in the real loop.
    fn send(&mut self, ev: Event) {
        self.app.on_event(ev);
        self.frame();
    }

    fn key(&mut self, code: KeyCode, m: KeyModifiers) {
        self.send(Event::Key(KeyEvent::new(code, m)));
    }

    fn keys(&mut self, s: &str) {
        for c in s.chars() {
            self.key(KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    fn enter(&mut self) {
        self.key(KeyCode::Enter, KeyModifiers::NONE);
    }

    fn esc(&mut self) {
        self.key(KeyCode::Esc, KeyModifiers::NONE);
    }

    fn click(&mut self, x: u16, y: u16) {
        let m = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE };
        self.send(Event::Mouse(m));
    }

    fn state(&self) -> State {
        self.log.load_state().unwrap().1
    }

    fn nav(&self) -> &todai_client::nav::Nav {
        &self.app.tabs[0].tab.nav
    }

    fn cursor(&self) -> Option<Focus> {
        self.nav().cursor.clone()
    }

    fn screen(&self) -> String {
        let buf = self.term.backend().buffer();
        let mut s = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        s
    }

    /// Where `needle` is on screen: column and row of its first char.
    fn find(&self, needle: &str) -> (u16, u16) {
        for (y, line) in self.screen().lines().enumerate() {
            let chars: Vec<char> = line.chars().collect();
            let n: Vec<char> = needle.chars().collect();
            if let Some(x) = chars.windows(n.len()).position(|w| w == n.as_slice()) {
                return (x as u16, y as u16);
            }
        }
        panic!("{needle:?} not on screen:\n{}", self.screen());
    }
}

fn item(id: &str) -> Focus {
    Focus::Item(id.into())
}

#[test]
fn answers_a_decision_by_option_and_enter() {
    let mut h = Harness::new(80, 30);
    assert_eq!(h.cursor(), Some(Focus::Section(Section::Pending)));
    h.keys("j");
    assert_eq!(h.cursor(), Some(item("d1")));
    h.keys("2");
    assert_eq!(h.nav().mode, Mode::Insert, "picking an option starts typing");
    h.enter();
    assert!(matches!(&h.state().decision("d1").unwrap().status, DecisionStatus::Resolved { text, .. } if text == "iced"));
    assert_eq!(h.nav().mode, Mode::Normal);
}

#[test]
fn types_a_resolution_with_a_newline() {
    let mut h = Harness::new(80, 30);
    h.keys("jji");
    assert_eq!(h.cursor(), Some(item("d2")));
    assert_eq!(h.nav().mode, Mode::Insert);
    // Letters are text while typing, not commands (x would mark a todo done).
    h.keys("tod x");
    h.key(KeyCode::Enter, KeyModifiers::ALT);
    h.keys("ai");
    h.enter();
    assert!(matches!(&h.state().decision("d2").unwrap().status, DecisionStatus::Resolved { text, .. } if text == "tod x\nai"));
}

#[test]
fn esc_keeps_the_draft_and_x_marks_a_todo_done() {
    let mut h = Harness::new(80, 30);
    h.keys("jjjia note");
    h.esc();
    assert_eq!(h.nav().mode, Mode::Normal);
    assert_eq!(h.app.tabs[0].tab.drafts.get("t1").map(String::as_str), Some("a note"));
    assert!(h.screen().contains("› a note"), "{}", h.screen());
    h.keys("x");
    assert_eq!(h.state().todo("t1").unwrap().done, Some(Some("a note".into())));
}

#[test]
fn visual_selection_becomes_an_elaboration() {
    let mut h = Harness::new(80, 30);
    h.keys("jvwe");
    assert_eq!(h.nav().mode, Mode::Visual);
    h.enter();
    assert_eq!(h.nav().mode, Mode::Prompt);
    assert_eq!(h.nav().prompt.as_ref().unwrap().selection, "egui");
    assert!(h.screen().contains("Elaborate on d1"), "{}", h.screen());
    h.keys("why?");
    h.enter();
    let s = h.state();
    let r = s.elab("r1").expect("request sent");
    assert_eq!((r.target.as_str(), r.selection.as_str(), r.note.as_deref()), ("d1", "egui", Some("why?")));
}

#[test]
fn wide_terminals_split_elaborations_into_a_column() {
    let mut h = Harness::new(140, 30);
    h.log.append(Actor::Operator, |s| Ok(Kind::ElabRequest { rid: s.next_elab_id(), target: "d1".into(), selection: "low".into(), note: None, parent: None })).unwrap();
    h.log.append(Actor::Agent, |_| Ok(Kind::ElabAdd { rid: "r1".into(), text_md: "## Why\n\n- egui is *immediate*".into() })).unwrap();
    h.frame();
    let (x, _) = h.find("ELABORATIONS");
    assert!(x >= 70, "elaborations on the right:\n{}", h.screen());
    assert!(h.screen().contains("• egui is immediate"), "{}", h.screen());
    h.keys("jl");
    assert_eq!(h.cursor(), Some(Focus::Elab("r1".into())));

    let mut narrow = Harness::new(80, 40);
    narrow.log.append(Actor::Operator, |s| Ok(Kind::ElabRequest { rid: s.next_elab_id(), target: "d1".into(), selection: "low".into(), note: None, parent: None })).unwrap();
    narrow.frame();
    assert!(narrow.nav().left.contains(&Focus::Section(Section::Elabs)), "elaborations as a section");
}

#[test]
fn zoom_opens_scrolls_and_closes() {
    let mut h = Harness::new(80, 20);
    h.keys("jK");
    assert_eq!(h.nav().zoom.as_deref(), Some("d1"));
    assert!(h.screen().contains("decision · pending"), "{}", h.screen());
    h.keys("n");
    assert_eq!(h.nav().zoom.as_deref(), Some("d2"));
    h.keys("q");
    assert!(h.nav().zoom.is_none());
    assert!(!h.app.confirm_quit, "q closes the zoom, not the app");
    h.keys("q");
    assert!(h.app.confirm_quit);
    h.esc();
    assert!(!h.app.confirm_quit && !h.app.quit);
}

#[test]
fn trash_asks_first() {
    let mut h = Harness::new(80, 30);
    h.keys("jjjx"); // t1 done
    h.keys("GlGd"); // Done starts collapsed, as in the window
    assert_eq!(h.app.tabs[0].tab.confirm_trash.as_deref(), Some(&["t1".to_owned()][..]), "{:?}", h.cursor());
    assert!(h.screen().contains("Remove t1 from the board?"));
    h.keys("y");
    assert!(h.state().todo("t1").is_none());
}

#[test]
fn mouse_clicks_focus_cards_and_pick_options() {
    let mut h = Harness::new(80, 30);
    let (x, y) = h.find("Plug in the keyboard");
    h.click(x, y);
    assert_eq!(h.cursor(), Some(item("t1")));
    let (x, y) = h.find("2 iced");
    h.click(x + 1, y);
    assert_eq!(h.cursor(), Some(item("d1")));
    assert_eq!(h.nav().mode, Mode::Insert);
    h.enter();
    assert!(matches!(&h.state().decision("d1").unwrap().status, DecisionStatus::Resolved { text, .. } if text == "iced"));
    let (x, y) = h.find("PENDING");
    h.click(x, y);
    assert!(!h.nav().is_open(Section::Pending));
}

#[test]
fn global_keys_switch_close_and_quit() {
    let mut h = Harness::new(80, 30);
    let other = h._dir.path().join("other.jsonl");
    h.app.open_file(other);
    h.frame();
    assert_eq!(h.app.active, 1);
    h.key(KeyCode::Char('1'), KeyModifiers::ALT);
    assert_eq!(h.app.active, 0);
    h.keys("gt");
    assert_eq!(h.app.active, 1);
    h.key(KeyCode::Char('w'), KeyModifiers::CONTROL);
    assert_eq!(h.app.tabs.len(), 1);
    h.keys("?");
    assert!(h.app.cheatsheet.is_some());
    h.esc();
    h.key(KeyCode::Char('q'), KeyModifiers::CONTROL);
    h.keys("y");
    assert!(h.app.quit);
}

/// Prints the demo-like board, for eyeballing with `--nocapture`.
#[test]
fn snapshot() {
    let mut h = Harness::new(130, 32);
    h.log.append(Actor::Operator, |s| Ok(Kind::ElabRequest { rid: s.next_elab_id(), target: "d1".into(), selection: "low".into(), note: Some("how low?".into()), parent: None })).unwrap();
    h.log.append(Actor::Agent, |_| Ok(Kind::ElabAdd { rid: "r1".into(), text_md: "## Short\n\nAbout **40 MB**.\n\n| ui | MB |\n|---|--:|\n| egui | 40 |\n| iced | 90 |\n\n```rust\nfn main() {}\n```".into() })).unwrap();
    h.frame();
    h.keys("j");
    println!("{}", h.screen());
}
