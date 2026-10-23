//! Keyboard navigation: a vim-like modal state machine over the board.
//!
//! Each frame, `handle` consumes key events using the focus lists recorded by
//! the previous frame's render; the board then re-records them. Mouse works in
//! every mode; `vim == false` only turns off the single-key commands.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use egui::{Event, Galley, Key, Modifiers};

use super::textnav as tn;
use crate::store::{DecisionStatus, Kind, State};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Focus {
    Section(Section),
    Item(String),
    Elab(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Pending,
    Elabs,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    Prompt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weight {
    Strong,
    Normal,
    Weak,
    /// Withdrawn by the agent: weak and struck through.
    Struck,
}

/// Text selection inside one item (vim visual mode).
pub struct Visual {
    pub target: Focus,
    pub text: Vec<char>,
    pub caret: usize,
    pub anchor: Option<usize>,
    h_pos: Option<f32>,
    /// Layout from the last frame, for moving by wrapped rows.
    pub galley: Option<Arc<Galley>>,
}

impl Visual {
    /// Inclusive selected char range, if an anchor is set.
    pub fn range(&self) -> Option<(usize, usize)> {
        self.anchor.map(|a| (a.min(self.caret), a.max(self.caret)))
    }
}

/// Text selected with the mouse. Kept here because egui only reports the
/// selection while the text has focus, and any click resets it.
#[derive(Clone, Debug)]
pub struct MouseSelection {
    pub focus: Focus,
    pub text: String,
    /// Char range in the item text, for restoring the highlight.
    pub range: (usize, usize),
}

/// An elaboration request being written.
pub struct Prompt {
    pub target: String,
    pub selection: String,
    pub note: String,
}

pub enum FocusRequest {
    Input(String),
    Prompt,
}

#[derive(Default)]
pub struct Outcome {
    pub kinds: Vec<Kind>,
    pub tab_step: i32,
    pub toggle_cheatsheet: bool,
    pub close_cheatsheet: bool,
    /// Items to remove from the board, once the operator confirms.
    pub trash: Option<Vec<String>>,
}

pub struct Nav {
    pub mode: Mode,
    pub cursor: Option<Focus>,
    open: [bool; 3],
    /// Focusable things in render order, recorded each frame.
    pub left: Vec<Focus>,
    pub right: Vec<Focus>,
    pub scroll_to_cursor: bool,
    pub visual: Option<Visual>,
    pub prompt: Option<Prompt>,
    pub focus_request: Option<FocusRequest>,
    /// Set by the board when one of our text inputs has keyboard focus.
    pub input_focused: Option<String>,
    pub prompt_focused: bool,
    pub mouse_selection: Option<MouseSelection>,
    /// Finished items and elaborations marked for removal (multi-select).
    pub marked: BTreeSet<String>,
    /// Elaborations the operator expanded or collapsed by hand; the rest
    /// follow `elab_auto_open`.
    pub elab_open: HashMap<String, bool>,
    /// Last item marked by itself; shift-click marks from here.
    mark_anchor: Option<String>,
    /// Markable ids per group (Done items, elaborations) in display order,
    /// recorded each frame.
    pub mark_groups: Vec<Vec<String>>,
    /// A shift-click waiting for the frame's groups to be complete.
    range_target: Option<String>,
    /// Where each item's text was drawn last frame (for tests driving the mouse).
    #[cfg(test)]
    pub text_rects: Vec<(Focus, egui::Rect)>,
    pub prefix: Option<char>,
    prev_index: usize,
}

impl Default for Nav {
    fn default() -> Nav {
        Nav {
            mode: Mode::Normal,
            cursor: None,
            open: [true, true, false],
            left: Vec::new(),
            right: Vec::new(),
            scroll_to_cursor: false,
            visual: None,
            prompt: None,
            focus_request: None,
            input_focused: None,
            prompt_focused: false,
            mouse_selection: None,
            marked: BTreeSet::new(),
            elab_open: HashMap::new(),
            mark_anchor: None,
            mark_groups: Vec::new(),
            range_target: None,
            #[cfg(test)]
            text_rects: Vec::new(),
            prefix: None,
            prev_index: 0,
        }
    }
}

/// The text of an item as shown on its card, split into styled parts. Visual
/// mode and the mouse-selectable text both index into the concatenation.
pub fn item_parts(state: &State, focus: &Focus) -> Vec<(String, Weight)> {
    let with_context = |title: &str, ctx: &Option<String>| {
        let mut v = vec![(title.to_owned(), Weight::Strong)];
        if let Some(c) = ctx {
            v.push((format!("\n{c}"), Weight::Weak));
        }
        v
    };
    match focus {
        Focus::Item(id) => {
            if let Some(d) = state.decision(id) {
                let mut parts = with_context(&d.title, &d.context);
                if d.status == DecisionStatus::Withdrawn {
                    parts.iter_mut().for_each(|p| p.1 = Weight::Struck);
                }
                parts
            } else if let Some(t) = state.todo(id) {
                with_context(&t.title, &t.context)
            } else {
                Vec::new()
            }
        }
        Focus::Elab(rid) => match state.elab(rid) {
            Some(r) => vec![(r.answer.clone().unwrap_or_else(|| r.selection.clone()), Weight::Normal)],
            None => Vec::new(),
        },
        Focus::Section(_) => Vec::new(),
    }
}

pub fn item_text(state: &State, focus: &Focus) -> String {
    item_parts(state, focus).into_iter().map(|(s, _)| s).collect()
}

/// The item an elaboration about `focus` should target.
pub fn elab_target(state: &State, focus: &Focus) -> Option<String> {
    match focus {
        Focus::Item(id) => Some(id.clone()),
        Focus::Elab(rid) => state.elab(rid).map(|r| r.target.clone()),
        Focus::Section(_) => None,
    }
}

impl Nav {
    pub fn is_open(&self, s: Section) -> bool {
        self.open[s as usize]
    }

    pub fn set_open(&mut self, s: Section, open: bool) {
        self.open[s as usize] = open;
        let inside = self.cursor.as_ref().is_some_and(|c| self.section_of(c) == Some(s) && *c != Focus::Section(s));
        if !open && inside {
            self.cursor = Some(Focus::Section(s));
        }
    }

    pub fn focus(&mut self, f: Focus) {
        self.cursor = Some(f);
        self.scroll_to_cursor = true;
    }

    pub fn open_prompt(&mut self, target: String, selection: String) {
        self.visual = None;
        self.mouse_selection = None;
        self.prompt = Some(Prompt { target, selection, note: String::new() });
        self.focus_request = Some(FocusRequest::Prompt);
    }

    /// Call before the board records this frame's focus lists.
    pub fn begin_render(&mut self) {
        self.prev_index = self.cursor_index().map_or(0, |(_, i)| i);
        self.left.clear();
        self.right.clear();
        self.input_focused = None;
        self.prompt_focused = false;
        self.mark_groups.clear();
        #[cfg(test)]
        self.text_rects.clear();
    }

    /// Call after the board rendered: syncs the mode with widget focus and
    /// keeps the cursor on something that still exists.
    pub fn end_render(&mut self) {
        if let Some(target) = self.range_target.take() {
            self.mark_range(target);
        }
        if self.prompt_focused {
            self.mode = Mode::Prompt;
        } else if let Some(id) = self.input_focused.take() {
            self.mode = Mode::Insert;
            self.cursor = Some(Focus::Item(id));
        } else if matches!(self.mode, Mode::Insert | Mode::Prompt) {
            self.mode = Mode::Normal;
        }
        if self.mode == Mode::Visual && self.visual.is_none() {
            self.mode = Mode::Normal;
        }

        let exists = self.cursor.as_ref().is_some_and(|c| self.left.contains(c) || self.right.contains(c));
        if !exists && !self.left.is_empty() {
            let i = self.prev_index.min(self.left.len() - 1);
            self.cursor = Some(self.left[i].clone());
        }
        self.scroll_to_cursor = false;
    }

    /// Processes this frame's key events and removes the ones it used.
    pub fn handle(
        &mut self,
        ctx: &egui::Context,
        state: &State,
        drafts: &mut HashMap<String, String>,
        vim: bool,
    ) -> Outcome {
        let mut out = Outcome::default();
        let events = ctx.input(|i| i.events.clone());
        let mut keep = Vec::with_capacity(events.len());
        for ev in events {
            let grabs_keys = vim && matches!(self.mode, Mode::Normal | Mode::Visual);
            let used = match &ev {
                Event::Text(t) if grabs_keys => {
                    for c in t.chars() {
                        self.on_char(c, state, drafts, &mut out);
                    }
                    true
                }
                Event::Key { key, pressed: true, modifiers, .. } => {
                    self.on_key(ctx, *key, *modifiers, state, drafts, vim, &mut out)
                }
                // Swallow releases of keys we own so no widget sees half a keystroke.
                Event::Key { key, modifiers, .. } => grabs_keys && !modifiers.command && *key != Key::Tab,
                _ => false,
            };
            if !used {
                keep.push(ev);
            }
        }
        ctx.input_mut(|i| i.events = keep);
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn on_key(
        &mut self,
        ctx: &egui::Context,
        key: Key,
        m: Modifiers,
        state: &State,
        drafts: &mut HashMap<String, String>,
        vim: bool,
        out: &mut Outcome,
    ) -> bool {
        if self.mode == Mode::Normal
            && !m.shift
            && !m.command
            && let Some(MouseSelection { focus, text: sel, .. }) = self.mouse_selection.clone()
        {
            match key {
                Key::Enter => {
                    if let Some(target) = elab_target(state, &focus) {
                        self.focus(focus);
                        self.open_prompt(target, sel.trim().to_owned());
                    }
                    return true;
                }
                // Esc closes an open context menu first; egui handles that.
                Key::Escape if !egui::Popup::is_any_open(ctx) => {
                    self.mouse_selection = None;
                    self.leave_input(ctx);
                    return true;
                }
                _ => {}
            }
        }
        match self.mode {
            Mode::Insert | Mode::Prompt => match key {
                Key::Escape => {
                    if self.mode == Mode::Prompt {
                        self.prompt = None;
                    }
                    self.leave_input(ctx);
                    true
                }
                Key::Enter if !m.shift && !m.command => {
                    self.submit(ctx, state, drafts, out);
                    true
                }
                _ => false,
            },
            Mode::Normal | Mode::Visual if vim => {
                if m.command {
                    return match key {
                        Key::D if m.ctrl => {
                            self.step(5);
                            true
                        }
                        Key::U if m.ctrl => {
                            self.step(-5);
                            true
                        }
                        _ => false,
                    };
                }
                let c = match key {
                    Key::ArrowDown => Some('j'),
                    Key::ArrowUp => Some('k'),
                    Key::ArrowLeft => Some('h'),
                    Key::ArrowRight => Some('l'),
                    _ => None,
                };
                if let Some(c) = c {
                    self.on_char(c, state, drafts, out);
                } else if key == Key::Enter {
                    if self.mode == Mode::Visual {
                        self.visual_to_prompt(state);
                    } else {
                        self.activate(state);
                    }
                } else if key == Key::Escape {
                    self.prefix = None;
                    if self.mode == Mode::Visual {
                        self.visual = None;
                        self.mode = Mode::Normal;
                    } else {
                        self.prompt = None;
                        out.close_cheatsheet = true;
                    }
                }
                key != Key::Tab
            }
            _ => false,
        }
    }

    fn on_char(&mut self, c: char, state: &State, drafts: &mut HashMap<String, String>, out: &mut Outcome) {
        if self.mode == Mode::Visual {
            self.visual_char(c, out);
            return;
        }
        if let Some(p) = self.prefix.take() {
            match (p, c) {
                ('g', 'g') => self.jump_edge(false),
                ('g', 't') => out.tab_step = 1,
                ('g', 'T') => out.tab_step = -1,
                // On an elaboration, z-commands fold the elaboration itself.
                ('z', 'a' | 'o' | 'c') if matches!(self.cursor, Some(Focus::Elab(_))) => {
                    let Some(Focus::Elab(rid)) = self.cursor.clone() else { unreachable!() };
                    let open = match c {
                        'o' => true,
                        'c' => false,
                        _ => !self.elab_is_open(state, &rid),
                    };
                    self.elab_open.insert(rid, open);
                }
                ('z', 'a') => self.toggle_section_at_cursor(),
                ('z', 'o') => self.section_at_cursor().into_iter().for_each(|s| self.set_open(s, true)),
                ('z', 'c') => self.section_at_cursor().into_iter().for_each(|s| self.set_open(s, false)),
                _ => {}
            }
            return;
        }
        match c {
            'j' => self.step(1),
            'k' => self.step(-1),
            'G' => self.jump_edge(true),
            'g' | 'z' => self.prefix = Some(c),
            ']' => self.section_jump(1),
            '[' => self.section_jump(-1),
            'h' => self.go_left(state),
            'l' => self.go_right(state),
            'i' | 'a' => self.activate(state),
            ' ' => match self.cursor.clone() {
                Some(Focus::Section(_)) => self.toggle_section_at_cursor(),
                Some(Focus::Item(id) | Focus::Elab(id)) if state.can_trash(&id) => self.toggle_mark(&id),
                _ => {}
            },
            'o' => {
                if let Some(Focus::Elab(rid)) = self.cursor.clone() {
                    let open = !self.elab_is_open(state, &rid);
                    self.elab_open.insert(rid, open);
                }
            }
            'd' => {
                let ids: Vec<String> = match &self.cursor {
                    _ if !self.marked.is_empty() => self.marked.iter().cloned().collect(),
                    Some(Focus::Item(id) | Focus::Elab(id)) if state.can_trash(id) => vec![id.clone()],
                    _ => Vec::new(),
                };
                if !ids.is_empty() {
                    out.trash = Some(ids);
                }
            }
            '1'..='9' => self.pick_option(c as usize - '1' as usize, state, drafts),
            'x' => self.toggle_done(state, drafts, out),
            'v' => self.start_visual(state, false),
            'V' => self.start_visual(state, true),
            'e' => {
                if let Some(f) = self.cursor.clone()
                    && let Some(target) = elab_target(state, &f)
                {
                    let text = item_text(state, &f);
                    self.open_prompt(target, text);
                }
            }
            '?' => out.toggle_cheatsheet = true,
            _ => {}
        }
    }

    fn visual_char(&mut self, c: char, out: &mut Outcome) {
        let Some(v) = self.visual.as_mut() else { return };
        let t = &v.text;
        let n = t.len();
        let prefix = self.prefix.take();
        let mut keep_h = false;
        let caret = match (prefix, c) {
            (Some('g'), 'g') => 0,
            (_, 'g') => {
                self.prefix = Some('g');
                return;
            }
            (_, 'h') => v.caret.saturating_sub(1),
            (_, 'l') => (v.caret + 1).min(n - 1),
            (_, 'w') => tn::word_forward(t, v.caret),
            (_, 'b') => tn::word_back(t, v.caret),
            (_, 'e') => tn::word_end(t, v.caret),
            (_, '0') => tn::line_start(t, v.caret),
            (_, '^') => tn::first_non_blank(t, v.caret),
            (_, '$') => tn::line_end(t, v.caret),
            (_, 'G') => n - 1,
            (_, 'j' | 'k') => {
                keep_h = true;
                match &v.galley {
                    Some(g) => {
                        let cur = egui::text::CCursor::new(v.caret);
                        let (next, h) = if c == 'j' {
                            g.cursor_down_one_row(&cur, v.h_pos)
                        } else {
                            g.cursor_up_one_row(&cur, v.h_pos)
                        };
                        v.h_pos = h;
                        next.index.0.min(n - 1)
                    }
                    None => v.caret,
                }
            }
            (_, 'v') => {
                v.anchor = if v.anchor.is_some() { None } else { Some(v.caret) };
                v.caret
            }
            (_, 'V') => {
                v.anchor = Some(0);
                n - 1
            }
            (_, 'o') => match v.anchor {
                Some(a) => {
                    v.anchor = Some(v.caret);
                    a
                }
                None => v.caret,
            },
            (_, '?') => {
                out.toggle_cheatsheet = true;
                v.caret
            }
            _ => v.caret,
        };
        v.caret = caret;
        if !keep_h {
            v.h_pos = None;
        }
    }

    fn visual_to_prompt(&mut self, state: &State) {
        let Some(v) = self.visual.take() else { return };
        self.mode = Mode::Normal;
        let (a, b) = v.range().unwrap_or_else(|| tn::word_at(&v.text, v.caret));
        let selection: String = v.text[a..=b].iter().collect();
        if let Some(target) = elab_target(state, &v.target) {
            self.open_prompt(target, selection.trim().to_owned());
        }
    }

    fn start_visual(&mut self, state: &State, all: bool) {
        let Some(f) = self.cursor.clone() else { return };
        if let Focus::Elab(rid) = &f {
            self.elab_open.insert(rid.clone(), true); // can't select hidden text
        }
        let text: Vec<char> = item_text(state, &f).chars().collect();
        if text.is_empty() {
            return;
        }
        let n = text.len();
        self.visual = Some(Visual {
            target: f,
            caret: if all { n - 1 } else { 0 },
            anchor: all.then_some(0),
            text,
            h_pos: None,
            galley: None,
        });
        self.mode = Mode::Visual;
    }

    /// Enter / i / a.
    fn activate(&mut self, state: &State) {
        match self.cursor.clone() {
            Some(Focus::Section(s)) => self.set_open(s, !self.is_open(s)),
            Some(Focus::Item(id)) => {
                let pending_decision = state.decision(&id).is_some_and(|d| d.status == DecisionStatus::Pending);
                let pending_todo = state.todo(&id).is_some_and(|t| t.done.is_none());
                if pending_decision || pending_todo {
                    self.focus_request = Some(FocusRequest::Input(id));
                }
            }
            Some(Focus::Elab(rid)) => {
                if let Some(r) = state.elab(&rid) {
                    let target = Focus::Item(r.target.clone());
                    if let Some(s) = self.section_for_item(state, &r.target) {
                        self.set_open(s, true);
                    }
                    self.focus(target);
                }
            }
            None => {}
        }
    }

    pub fn elab_is_open(&self, state: &State, rid: &str) -> bool {
        self.elab_open.get(rid).copied().unwrap_or_else(|| elab_auto_open(state, rid))
    }

    pub fn toggle_mark(&mut self, id: &str) {
        if !self.marked.remove(id) {
            self.marked.insert(id.to_owned());
        }
        self.mark_anchor = Some(id.to_owned());
    }

    /// A checkbox click: toggles one item, or with shift marks everything
    /// from the anchor to `id` when both are in the same group.
    pub fn click_mark(&mut self, id: &str, shift: bool) {
        if shift && self.mark_anchor.is_some() {
            self.range_target = Some(id.to_owned());
        } else {
            self.toggle_mark(id);
        }
    }

    fn mark_range(&mut self, target: String) {
        let anchor = self.mark_anchor.clone().unwrap_or_default();
        let span = self.mark_groups.iter().find_map(|g| {
            let a = g.iter().position(|x| *x == anchor)?;
            let b = g.iter().position(|x| *x == target)?;
            Some(g[a.min(b)..=a.max(b)].to_vec())
        });
        match span {
            Some(ids) => self.marked.extend(ids),
            // Different groups: a plain toggle, which becomes the new anchor.
            None => self.toggle_mark(&target),
        }
    }

    fn pick_option(&mut self, n: usize, state: &State, drafts: &mut HashMap<String, String>) {
        let Some(Focus::Item(id)) = &self.cursor else { return };
        let Some(d) = state.decision(id).filter(|d| d.status == DecisionStatus::Pending) else { return };
        if let Some(o) = d.options.get(n) {
            drafts.insert(id.clone(), o.clone());
            self.focus_request = Some(FocusRequest::Input(id.clone()));
        }
    }

    fn toggle_done(&mut self, state: &State, drafts: &mut HashMap<String, String>, out: &mut Outcome) {
        let Some(Focus::Item(id)) = &self.cursor else { return };
        if let Some(t) = state.todo(id) {
            out.kinds.push(match t.done {
                None => Kind::TodoDone { id: id.clone(), note: take_draft(drafts, id) },
                Some(_) => Kind::TodoUndone { id: id.clone() },
            });
        } else if let Some(d) = state.decision(id)
            && matches!(d.status, DecisionStatus::Resolved { .. })
        {
            out.kinds.push(Kind::DecisionReopen { id: id.clone() });
        }
    }

    fn submit(&mut self, ctx: &egui::Context, state: &State, drafts: &mut HashMap<String, String>, out: &mut Outcome) {
        if self.mode == Mode::Prompt {
            if let Some(p) = self.prompt.take() {
                let note = Some(p.note.trim().to_owned()).filter(|n| !n.is_empty());
                // rid is reassigned under the log lock when sent.
                out.kinds.push(Kind::ElabRequest { rid: String::new(), target: p.target, selection: p.selection, note });
            }
        } else if let Some(Focus::Item(id)) = &self.cursor {
            if state.decision(id).is_some() {
                let text = drafts.get(id).map(|s| s.trim().to_owned()).unwrap_or_default();
                if text.is_empty() {
                    return;
                }
                out.kinds.push(Kind::DecisionResolve { id: id.clone(), text });
            } else if state.todo(id).is_some() {
                out.kinds.push(Kind::TodoDone { id: id.clone(), note: take_draft(drafts, id) });
            }
        }
        self.leave_input(ctx);
    }

    fn leave_input(&mut self, ctx: &egui::Context) {
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
        self.mode = Mode::Normal;
    }

    // ---- cursor movement -------------------------------------------------

    fn cursor_index(&self) -> Option<(bool, usize)> {
        let c = self.cursor.as_ref()?;
        if let Some(i) = self.left.iter().position(|f| f == c) {
            return Some((false, i));
        }
        self.right.iter().position(|f| f == c).map(|i| (true, i))
    }

    fn list(&self, right: bool) -> &[Focus] {
        if right { &self.right } else { &self.left }
    }

    fn step(&mut self, d: isize) {
        let (right, i) = match self.cursor_index() {
            Some(x) => x,
            None => {
                if let Some(f) = self.left.first().cloned() {
                    self.focus(f);
                }
                return;
            }
        };
        let list = self.list(right);
        let j = (i as isize + d).clamp(0, list.len() as isize - 1) as usize;
        let f = list[j].clone();
        self.focus(f);
    }

    fn jump_edge(&mut self, end: bool) {
        let right = self.cursor_index().is_some_and(|(r, _)| r);
        let list = self.list(right);
        let f = if end { list.last() } else { list.first() };
        if let Some(f) = f.cloned() {
            self.focus(f);
        }
    }

    fn section_jump(&mut self, d: isize) {
        let i = match self.cursor_index() {
            Some((false, i)) => i as isize,
            _ => -1,
        };
        let headers: Vec<usize> = (0..self.left.len()).filter(|&k| matches!(self.left[k], Focus::Section(_))).collect();
        let target = if d > 0 {
            headers.iter().find(|&&k| k as isize > i)
        } else {
            headers.iter().rev().find(|&&k| (k as isize) < i)
        };
        if let Some(&k) = target {
            let f = self.left[k].clone();
            self.focus(f);
        }
    }

    fn section_of(&self, f: &Focus) -> Option<Section> {
        if let Focus::Section(s) = f {
            return Some(*s);
        }
        let i = self.left.iter().position(|x| x == f)?;
        self.left[..i].iter().rev().find_map(|x| match x {
            Focus::Section(s) => Some(*s),
            _ => None,
        })
    }

    fn section_at_cursor(&self) -> Option<Section> {
        self.cursor.as_ref().and_then(|c| self.section_of(c))
    }

    fn toggle_section_at_cursor(&mut self) {
        if let Some(s) = self.section_at_cursor() {
            self.set_open(s, !self.is_open(s));
            self.focus(Focus::Section(s));
        }
    }

    fn section_for_item(&self, state: &State, id: &str) -> Option<Section> {
        let pending = state.decision(id).is_some_and(|d| d.status == DecisionStatus::Pending)
            || state.todo(id).is_some_and(|t| t.done.is_none());
        let exists = state.decision(id).is_some() || state.todo(id).is_some();
        exists.then_some(if pending { Section::Pending } else { Section::Done })
    }

    fn go_left(&mut self, state: &State) {
        match (self.cursor.clone(), self.cursor_index()) {
            (Some(Focus::Section(s)), _) => self.set_open(s, false),
            (Some(Focus::Elab(rid)), Some((true, _))) => {
                let target = state.elab(&rid).map(|r| Focus::Item(r.target.clone()));
                if let Some(f) = target.filter(|t| self.left.contains(t)).or_else(|| self.left.first().cloned()) {
                    self.focus(f);
                }
            }
            (Some(f), Some((false, _))) => {
                if let Some(s) = self.section_of(&f) {
                    self.focus(Focus::Section(s));
                }
            }
            _ => {}
        }
    }

    fn go_right(&mut self, state: &State) {
        match (self.cursor.clone(), self.cursor_index()) {
            (Some(Focus::Section(s)), _) if !self.is_open(s) => self.set_open(s, true),
            (Some(Focus::Section(_)), _) => self.step(1),
            (Some(Focus::Item(id)), Some((false, _))) if !self.right.is_empty() => {
                // Prefer the newest elaboration about this item.
                let about = self.right.iter().find(|f| matches!(f, Focus::Elab(r) if state.elab(r).is_some_and(|e| e.target == id)));
                let f = about.unwrap_or(&self.right[0]).clone();
                self.focus(f);
            }
            _ => {}
        }
    }
}

/// Default fold state: open while unanswered, while its item is still
/// pending, or if it is the newest answer; older answers collapse.
pub fn elab_auto_open(state: &State, rid: &str) -> bool {
    let Some(r) = state.elab(rid) else { return false };
    let item_pending = state.decision(&r.target).is_some_and(|d| d.status == DecisionStatus::Pending)
        || state.todo(&r.target).is_some_and(|t| t.done.is_none());
    let newest_answer = state.elabs.iter().rev().find(|e| e.answer.is_some()).is_some_and(|e| e.rid == rid);
    r.answer.is_none() || item_pending || newest_answer
}

fn take_draft(drafts: &mut HashMap<String, String>, id: &str) -> Option<String> {
    drafts.get(id).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}
