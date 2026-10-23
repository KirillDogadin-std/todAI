# Agent integration

todai has to work with any coding agent, and must never change how an agent
behaves unless the user asked for it. That gives three layers, each optional:

1. **The CLI** is the protocol. Any agent that can run shell commands can post
   decisions and todos and read answers.
2. **A skill** tells the agent the CLI exists. It is an
   [Agent Skills](https://agentskills.io) `SKILL.md`, read by Claude Code,
   Codex, pi, OpenCode and others. It only says "run `todai guide`"; the real
   instructions ship inside the binary, so they always match the installed
   version.
3. **An awareness adapter** pushes the operator's answers into the agent
   while it works. There is no cross-agent standard for this, so each agent
   gets its own thin adapter. All of them call back into `todai`.

## Opt-in at three levels

- **Installed only on request.** `todai setup` writes nothing until the user
  confirms, and only at the scope they pick: the current project, or their
  user. Every write is recorded in `~/.local/state/todai/installs.json` and
  `todai setup --remove` reverts it (files edited since install are kept).
- **Active only when attached.** Installed adapters do nothing in a session
  until it runs `todai attach`. Agents load the skill only when the user asks
  to use todai.
- **Scoped per project.** Project installs go to project directories; Claude
  Code hooks go to the personal, uncommitted `.claude/settings.local.json` so
  teammates are unaffected.

## The protocol

`todai guide` prints it; source: [`src/agent/guide.md`](../src/agent/guide.md).
In short:

1. `todai attach [NAME]` creates `.todai/NAME.jsonl` at the repo root, opens
   it, and prints `todai: attached <path>`.
2. `todai add-decision` / `todai add-todo` post items and print their ids.
3. Operator updates arrive as a block starting `todai: operator updates`,
   either injected by an adapter or fetched with `todai inbox` (between steps)
   or `todai wait` (when blocked).
4. Elaboration requests (`rN`) are answered with `todai elaborate`.

Reading updates marks them as seen (an `ack` event in the board), so each
update is delivered once, whichever path delivers it.

## How a session finds its board

Adapters look for the line `todai: attached <path>` in the agent's shell tool
output. That is the only coupling: the agent never has to tell the adapter
anything, and sessions that never attach are never touched.

## Claude Code

Installed by `todai setup --agent claude`:

| Scope | Skill | Hooks |
|---|---|---|
| project | `.claude/skills/todai/SKILL.md` | `.claude/settings.local.json` |
| user | `~/.claude/skills/todai/SKILL.md` | `~/.claude/settings.json` |

Hooks (`todai hook claude <event>`, implemented in
[`src/agent/claude.rs`](../src/agent/claude.rs)):

- **PostToolUse** (all tools): if a Bash result contains the attach line,
  binds the session id to that board in
  `~/.local/state/todai/claude-sessions.json`. For a bound session, unseen
  operator updates are returned as `additionalContext`, so the agent sees them
  right after its current tool call, without interruption.
- **Stop**: unseen updates block the stop and are handed over as the reason.
  If items are still open and no `todai wait` is running for the board, the
  agent is asked once (per stop attempt) to start `todai wait BOARD` as a
  background command; when the operator answers, that command exits and
  Claude Code wakes the agent.

Hooks never fail the agent: errors are reported on stderr and the hook exits 0.

## pi

Installed by `todai setup --agent pi`:

| Scope | Skill | Extension |
|---|---|---|
| project | `.agents/skills/todai/SKILL.md` | `.pi/extensions/todai.ts` |
| user | `~/.agents/skills/todai/SKILL.md` | `~/.pi/agent/extensions/todai.ts` |

The extension ([`src/agent/pi-extension.ts`](../src/agent/pi-extension.ts))
watches `tool_result` events of the bash tool for the attach line, then keeps
one `todai wait BOARD` child running. Each time it prints updates, the
extension calls `pi.sendMessage(..., { triggerTurn: true, deliverAs: "steer" })`:
a working agent receives it as steering, an idle agent starts a turn. The
child is killed on `session_shutdown`.

## Other agents

`todai setup --agent generic` installs the skill into `.agents/skills`
(project) or `~/.agents/skills` (user), which Codex, OpenCode and other Agent
Skills readers scan. Without an adapter, the guide tells the agent to poll
with `todai inbox` between steps and to run `todai wait` (in the background if
the harness supports it) when blocked.

## Writing a new adapter

An adapter needs three things, all available from the CLI:

1. Spot `todai: attached <path>` in shell output to learn the board.
2. Get updates: run `todai wait BOARD` in a loop (it prints and acks, then
   exits), or call `todai inbox BOARD` at convenient points (it prints nothing
   useful when there is nothing new: `todai: no operator updates`).
3. Deliver the printed block into the agent's context with whatever the
   harness offers.

Add it as an `Agent` variant in [`src/agent/setup.rs`](../src/agent/setup.rs)
so `todai setup` can install and remove it.
