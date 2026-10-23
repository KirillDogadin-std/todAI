//! Headless keyboard-flow tests: drive the real board with synthetic events
//! and check what lands in the log.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, vec2};
use egui_commonmark::CommonMarkCache;

use super::nav::{Focus, Mode, MouseSelection, Section};
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
                let o = tab.nav.handle(ui.ctx(), &tab.state, &mut tab.drafts, vim);
                // As App::ui does: a trash request waits for confirmation.
                if o.trash.is_some() {
                    tab.confirm_trash = o.trash;
                }
                o.kinds
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

#[test]
fn enter_on_mouse_selection_opens_prompt() {
    for vim in [true, false] {
        let mut h = Harness::new();
        h.vim = vim;
        h.tab.nav.mouse_selection =
            Some(MouseSelection { focus: item("d1"), text: "iced".into(), range: (13, 17) });
        h.key(Key::Enter, Modifiers::NONE);
        let p = h.tab.nav.prompt.as_ref().expect("prompt opened");
        assert_eq!((p.target.as_str(), p.selection.as_str()), ("d1", "iced"));
        assert_eq!(h.tab.nav.mode, Mode::Prompt);
        h.text("why?");
        h.key(Key::Enter, Modifiers::NONE);
        let r = h.state().elab("r1").cloned().expect("request sent");
        assert_eq!((r.selection.as_str(), r.note.as_deref()), ("iced", Some("why?")));
    }
}

#[test]
fn mouse_drag_select_then_enter() {
    for vim in [true, false] {
        let mut h = Harness::new();
        h.vim = vim;
        let rect = h.tab.nav.text_rects.iter().find(|(f, _)| *f == item("d1")).unwrap().1;
        let start = rect.left_center() + vec2(1.0, 0.0);
        let end = rect.left_center() + vec2(25.0, 0.0);
        let button = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        h.frame(vec![Event::PointerMoved(start)]);
        h.frame(vec![button(start, true)]);
        for i in 1..=5 {
            h.frame(vec![Event::PointerMoved(start + (end - start) * (i as f32 / 5.0))]);
        }
        h.frame(vec![button(end, false)]);
        h.frame(vec![]);
        let sel = h.tab.nav.mouse_selection.clone();
        assert!(matches!(&sel, Some(m) if m.focus == item("d1") && "Use egui or iced?".starts_with(m.text.as_str()) && !m.text.is_empty()),
            "vim={vim}: selection {sel:?}");

        // Right-click on the selection must not clear it.
        let secondary = |pressed| Event::PointerButton { pos: start, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE };
        h.frame(vec![Event::PointerMoved(start)]);
        h.frame(vec![secondary(true)]);
        h.frame(vec![secondary(false)]);
        h.frame(vec![]);
        let after = h.tab.nav.mouse_selection.clone().map(|m| m.text);
        assert_eq!(after, sel.map(|m| m.text), "vim={vim}: right-click kept selection");
        h.key(Key::Escape, Modifiers::NONE); // close the context menu

        h.key(Key::Enter, Modifiers::NONE);
        assert_eq!(h.tab.nav.mode, Mode::Prompt, "vim={vim}");
        h.text("why?");
        h.key(Key::Enter, Modifiers::NONE);
        assert!(h.state().elab("r1").is_some(), "vim={vim}");
    }
}

#[test]
fn withdrawn_decision_shows_in_done_without_reopen() {
    let mut h = Harness::new();
    h.log.append(Actor::Agent, |_| Ok(Kind::DecisionWithdraw { id: "d2".into() })).unwrap();
    h.frame(vec![]);
    h.frame(vec![]);
    h.type_keys("G"); // Done header
    h.type_keys("l"); // expand
    h.move_to(item("d2")); // reachable in Done
    h.type_keys("x"); // reopen is not allowed for withdrawn: no event
    assert_eq!(h.state().decision("d2").unwrap().status, DecisionStatus::Withdrawn);
    assert!(super::nav::item_parts(&h.tab.state, &item("d2")).iter().all(|p| p.1 == super::nav::Weight::Struck));
}

#[test]
fn notifications_only_for_new_agent_posts() {
    let mut h = Harness::new();
    assert!(h.tab.new_posts.is_empty(), "existing items on open never notify");
    h.log
        .append(Actor::Agent, |s| {
            Ok(Kind::DecisionAdd { id: s.next_decision_id(), title: "New?".into(), context: None, options: vec![] })
        })
        .unwrap();
    h.log.append(Actor::Operator, |_| Ok(Kind::TodoDone { id: "t1".into(), note: None })).unwrap();
    h.tab.reload_if_dirty_now();
    assert_eq!(h.tab.new_posts, ["Decision d3: New?"], "operator events never notify");
}

/// Resolves d1 and finishes t1 so the Done section has two items, opened.
fn harness_with_done() -> Harness {
    let mut h = Harness::new();
    let op = |k: Kind| h.log.append(Actor::Operator, |_| Ok(k)).unwrap();
    op(Kind::DecisionResolve { id: "d1".into(), text: "egui".into() });
    op(Kind::TodoDone { id: "t1".into(), note: None });
    h.tab.reload_if_dirty_now();
    h.frame(vec![]);
    h.type_keys("G"); // Done header
    h.type_keys("l"); // expand
    h
}

#[test]
fn space_marks_and_d_trashes_after_confirm() {
    let mut h = harness_with_done();
    h.move_to(item("d1"));
    h.type_keys(" ");
    h.move_to(item("t1"));
    h.type_keys(" ");
    assert_eq!(h.tab.nav.marked.len(), 2);
    h.type_keys("d");
    assert_eq!(h.tab.confirm_trash.as_deref(), Some(&["d1".to_owned(), "t1".to_owned()][..]));
    // Board::frame doesn't run the app's dialog; confirm the way it does.
    let ids = h.tab.confirm_trash.take().unwrap();
    assert!(h.tab.send(Kind::Trash { ids }));
    let s = h.state();
    assert!(s.decision("d1").is_none() && s.todo("t1").is_none());
    assert!(s.decision("d2").is_some(), "pending items untouched");
}

#[test]
fn d_without_marks_trashes_cursor_item_only_if_finished() {
    let mut h = harness_with_done();
    h.move_to(item("t1"));
    h.type_keys("d");
    assert_eq!(h.tab.confirm_trash.as_deref(), Some(&["t1".to_owned()][..]));

    let mut h = Harness::new();
    h.move_to(item("d1")); // pending
    h.type_keys(" ");
    h.type_keys("d");
    assert!(h.tab.nav.marked.is_empty() && h.tab.confirm_trash.is_none(), "pending items can't be marked");
}

/// d1 resolved with an answered elaboration r1 (old), plus an answered r2
/// on pending d2 and an unanswered r3 on pending d2.
fn harness_with_elabs() -> Harness {
    let mut h = Harness::new();
    let op = |k: Kind| h.log.append(Actor::Operator, |_| Ok(k)).unwrap();
    let agent = |k: Kind| h.log.append(Actor::Agent, |_| Ok(k)).unwrap();
    let ask = |rid: &str, target: &str| Kind::ElabRequest { rid: rid.into(), target: target.into(), selection: "x".into(), note: None };
    op(ask("r1", "d1"));
    agent(Kind::ElabAdd { rid: "r1".into(), text_md: "**old** answer".into() });
    op(Kind::DecisionResolve { id: "d1".into(), text: "egui".into() });
    op(ask("r2", "d2"));
    agent(Kind::ElabAdd { rid: "r2".into(), text_md: "newer".into() });
    op(ask("r3", "d2"));
    h.tab.reload_if_dirty_now();
    h.frame(vec![]);
    h.frame(vec![]);
    h
}

#[test]
fn elaborations_auto_collapse_only_old_answers() {
    let h = harness_with_elabs();
    let open = |rid| super::nav::elab_auto_open(&h.tab.state, rid);
    assert!(!open("r1"), "answered, item resolved, not newest: collapsed");
    assert!(open("r2"), "newest answer stays open");
    assert!(open("r3"), "unanswered stays open");
}

#[test]
fn o_and_za_fold_an_elaboration_and_v_unfolds() {
    let mut h = harness_with_elabs();
    h.move_to(Focus::Elab("r1".into()));
    assert!(!h.tab.nav.elab_is_open(&h.tab.state, "r1"));
    h.type_keys("o");
    assert!(h.tab.nav.elab_is_open(&h.tab.state, "r1"));
    h.type_keys("za");
    assert!(!h.tab.nav.elab_is_open(&h.tab.state, "r1"), "za folds the elaboration, not the section");
    assert!(h.tab.nav.is_open(Section::Elabs));
    h.type_keys("v");
    assert!(h.tab.nav.elab_is_open(&h.tab.state, "r1"), "selecting text unfolds it");
}

#[test]
fn elaborations_can_be_marked_and_trashed() {
    let mut h = harness_with_elabs();
    h.move_to(Focus::Elab("r3".into())); // unanswered: trashing cancels it
    h.type_keys(" ");
    h.move_to(Focus::Elab("r1".into()));
    h.type_keys(" ");
    h.type_keys("d");
    let ids = h.tab.confirm_trash.take().expect("asks to confirm");
    assert_eq!(ids, ["r1", "r3"]);
    assert!(h.tab.send(Kind::Trash { ids }));
    let s = h.state();
    assert!(s.elab("r1").is_none() && s.elab("r3").is_none() && s.elab("r2").is_some());
}

#[test]
fn shift_click_marks_a_range_within_one_group() {
    let mut h = harness_with_done(); // Done: d1, then t1
    h.log.append(Actor::Operator, |_| Ok(Kind::DecisionResolve { id: "d2".into(), text: "x".into() })).unwrap();
    h.tab.reload_if_dirty_now();
    h.frame(vec![]); // Done order is now d1, d2, t1
    assert_eq!(h.tab.nav.mark_groups[0], ["d1", "d2", "t1"]);

    h.tab.nav.click_mark("d1", false);
    h.tab.nav.click_mark("t1", true);
    h.frame(vec![]);
    assert_eq!(h.tab.nav.marked.iter().collect::<Vec<_>>(), ["d1", "d2", "t1"]);

    // Shift-click into another group only toggles that item.
    h.tab.nav.marked.clear();
    h.tab.nav.click_mark("t1", false);
    h.tab.nav.click_mark("r9", true);
    h.frame(vec![]);
    assert_eq!(h.tab.nav.marked.iter().collect::<Vec<_>>(), ["r9", "t1"]);
}

#[test]
fn closing_tabs_keeps_active_index_valid() {
    use super::active_after_close as after;
    assert_eq!(after(2, 0, 2), 1, "closing a tab left of the active one shifts it");
    assert_eq!(after(2, 2, 2), 1, "closing the last, active tab selects the new last");
    assert_eq!(after(0, 0, 2), 0, "closing the first, active tab selects the next");
    assert_eq!(after(0, 2, 2), 0, "closing a tab right of the active one keeps it");
    assert_eq!(after(0, 0, 0), 0, "closing the only tab");
}

#[test]
fn default_fonts_have_glyphs_we_use() {
    let ctx = egui::Context::default();
    let _ = ctx.run_ui(RawInput::default(), |_| {});
    let font = egui::FontId::proportional(14.0);
    // Every symbol drawn outside plain text. Many arrows and checkmarks
    // (→ ✓ ✔ ▶) are NOT in egui's default fonts and render as boxes.
    let src = [include_str!("mod.rs"), include_str!("board.rs")].concat();
    let used: String = src.chars().filter(|c| !c.is_ascii()).collect();
    let missing: String = used.chars().filter(|c| !ctx.fonts_mut(|f| f.has_glyph(&font, *c))).collect();
    assert!(missing.is_empty(), "default fonts lack: {missing}");
}
