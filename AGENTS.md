# todai: notes for agents working on this repo

Native board (Rust + egui) where coding agents post decisions and manual todos
for the operator. Start with `README.md`, then `docs/architecture.md`.

This file is the agent-agnostic `AGENTS.md`. For Claude Code, make a local,
gitignored symlink: `ln -s AGENTS.md CLAUDE.md`.

## Build and test

```sh
cargo build                 # toolchain pinned to stable via rust-toolchain.toml
cargo test                  # store, vim motions, IPC, setup merge, headless GUI tests
cargo build --release       # use this binary for memory or startup measurements
examples/demo.sh            # populated demo board in a real window (--no-open: just build it)
```

Keep `cargo build` warning-free and `cargo test` green before committing.

## Rules that are easy to break

- **Agent integration is agent-agnostic and opt-in.** Never tell users to
  paste into CLAUDE.md or any agent's instruction file, and never install
  anything globally by default. New agent support goes through
  `src/agent/setup.rs` (recorded, reversible, scoped) and must stay inert until
  a session runs `todai attach`. See `docs/agents.md`.
- **Use the agent-agnostic `.agents/` directory** for anything todai puts in
  a project or home dir (boards in `.agents/todai/`, the skill in
  `.agents/skills/todai/`). Agents that only read their own directory get a
  symlink (Claude Code: `.claude/skills/todai`), never a second copy.
- **The protocol text lives in `src/agent/guide.md`** and ships in the binary.
  The installed skill (`SKILL` in `src/agent/mod.rs`) only points at
  `todai guide`; keep it that way so upgrades never need a re-setup.
- **`todai: attached <path>`** (`ATTACHED_MARKER`) is how every adapter finds a
  session's board. Changing its format breaks the Claude hooks and the pi
  extension.
- **Hooks must never fail the agent.** `todai hook ...` reports errors on stderr
  and exits 0.
- **All writes go through `Log::append`** (flock + validate + append). Never
  rewrite a board file.
- **Keys are documented in three places**: `CHEATSHEET` in `src/gui/mod.rs`,
  `docs/keyboard.md`, and the behavior in `src/gui/nav.rs`. Change them
  together.

## egui 0.35 specifics

The API differs from most examples online:

- `eframe::App::ui(&mut self, ui: &mut Ui, frame)` replaces `update(ctx)`;
  `App::logic` runs even when the window is hidden (socket requests are
  served there).
- Panels: `egui::Panel::top("id").show(ui, ...)`, `Panel::right(..)`,
  `CentralPanel::default_margins().show(ui, ...)`.
- `ctx.fonts_mut(|f| f.glyph_width(...))`, `ctx.content_rect()`.
- eframe is built without default features (`glow`, `wayland`, `x11`,
  `default_fonts` only) to keep memory low; don't re-enable `wgpu` or
  `accesskit` casually.

## GUI tests

`src/gui/tests.rs` drives the real board headlessly (`Context::run_ui` with
synthetic `Event::Text`, `Event::Key`, pointer events) and asserts on the
board file and `Nav` state. Add a test there for any new keyboard or mouse
flow. Run an idle frame after each input so focus changes settle.

## Working on this machine

- Hyprland uses Lua dispatch: `hyprctl dispatch 'hl.dsp.window.float({ action = "enable", window = "address:0x…" })'`.
- Use `pkill -x todai`, not `pkill -f todai` (that matches your own shell).
- To test the window without touching the user's session, set
  `TODAI_SOCKET` and `XDG_STATE_HOME` to scratch paths.

## Commits

- No GPG signing unless overridden by higher level instruction (repo-local `commit.gpgsign=false` is set).
- Small, focused commits with a body explaining why when it isn't obvious.
