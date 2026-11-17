// The todai web app: answer boards from a phone. Talks to the server it is
// served from (/api/v2, docs/server.md) with the token saved at pairing.
// Plain DOM, no build step; text from boards only ever goes through
// textContent (see markdown.js).

import { renderMarkdown, plainText } from "./markdown.js";

const API = "/api/v2";
const TOKEN_KEY = "todai-token";
const LONG_POLL = 25;

const $ = (id) => document.getElementById(id);
const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
};
const button = (label, onClick, cls) => {
  const b = el("button", cls, label);
  b.type = "button";
  b.addEventListener("click", onClick);
  return b;
};

// ---- token and requests ----------------------------------------------------

// A pairing link carries the token in the fragment (#token=...): browsers
// never send that part to the server. Save it and drop it from the address bar.
function takeTokenFromUrl() {
  const m = location.hash.match(/^#token=([0-9a-fA-F]+)$/);
  if (!m) return;
  localStorage.setItem(TOKEN_KEY, m[1]);
  history.replaceState(null, "", location.pathname + "#/");
}

const token = () => localStorage.getItem(TOKEN_KEY);

class AuthError extends Error {}

async function api(path, opts = {}) {
  const res = await fetch(API + path, {
    ...opts,
    headers: { Authorization: `Bearer ${token()}`, ...(opts.body ? { "Content-Type": "application/json" } : {}) },
  });
  if (res.status === 401) throw new AuthError("the token was not accepted");
  const body = res.status === 204 ? null : await res.json().catch(() => null);
  if (!res.ok) {
    const err = new Error(body?.error || `HTTP ${res.status}`);
    err.status = res.status;
    throw err;
  }
  return body;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Keeps the focused field (by data-key) and its caret across a re-render.
function keepingFocus(draw) {
  const a = document.activeElement;
  const key = a?.dataset?.key;
  const caret = key && a.setSelectionRange ? [a.selectionStart, a.selectionEnd] : null;
  draw();
  if (!key) return;
  const again = $("main").querySelector(`[data-key="${CSS.escape(key)}"]`);
  if (again) {
    again.focus({ preventScroll: true });
    if (caret && again.setSelectionRange) again.setSelectionRange(...caret);
  }
}

// ---- the change feed ------------------------------------------------------------
//
// The app follows every board for as long as it is open (docs/server.md):
// pushed over /stream (server-sent events) when that works, else by
// long-polling /changes, e.g. behind a proxy that buffers the stream. Both
// deliver the same answers; the screen on show gets each one
// (`screen.onFeed`), and agent posts become notifications.

const feed = { cursor: null, running: false, abort: null, noStream: false };

// One answer from either transport.
function onFeedAnswer(f) {
  const baseline = !feed.cursor;
  feed.cursor = f.cursor;
  if ($("status").className === "error") status("live", "ok");
  if (!baseline && (f.boards.length || f.removed?.length)) {
    notifyAgentPosts(f);
    screen?.onFeed?.(f);
  }
}

// Reads /stream until it ends. Aborts with "silent" if nothing arrives at
// first (a buffering proxy: use long-poll) and with "stale" if heartbeats
// stop (reconnect).
async function streamOnce(ctl) {
  const q = feed.cursor ? `?cursor=${encodeURIComponent(feed.cursor)}` : "";
  const res = await fetch(`${API}/stream${q}`, { headers: { Authorization: `Bearer ${token()}` }, signal: ctl.signal });
  if (res.status === 401) throw new AuthError("the token was not accepted");
  if (!res.ok || !res.body) {
    const err = new Error(`HTTP ${res.status}`);
    err.status = res.status;
    throw err;
  }
  let got = false;
  let timer = null;
  const watch = () => {
    clearTimeout(timer);
    timer = setTimeout(() => ctl.abort(got ? "stale" : "silent"), got ? 45000 : 10000);
  };
  watch();
  const reader = res.body.pipeThrough(new TextDecoderStream()).getReader();
  let buf = "";
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) return;
      buf += value;
      let i;
      while ((i = buf.indexOf("\n\n")) >= 0) {
        const msg = buf.slice(0, i);
        buf = buf.slice(i + 2);
        got = true;
        watch();
        const data = msg.split("\n").filter((l) => l.startsWith("data: ")).map((l) => l.slice(6)).join("\n");
        if (data && msg.split("\n").includes("event: changes")) onFeedAnswer(JSON.parse(data));
      }
    }
  } finally {
    clearTimeout(timer);
  }
}

async function runFeed() {
  if (feed.running) return;
  feed.running = true;
  while (token()) {
    const ctl = new AbortController();
    feed.abort = ctl;
    try {
      if (!feed.noStream) {
        await streamOnce(ctl);
        continue; // the stream ended (server restart?): reconnect
      }
      const q = feed.cursor ? `?cursor=${encodeURIComponent(feed.cursor)}&wait=${LONG_POLL}` : "";
      onFeedAnswer(await api(`/changes${q}`, { signal: ctl.signal }));
    } catch (e) {
      if (ctl.signal.aborted) {
        if (ctl.signal.reason === "silent") feed.noStream = true;
        continue; // restarted on purpose, or a stalled stream
      }
      if (e instanceof AuthError) break;
      if (e.status === 400) feed.cursor = null; // a cursor this server can't read
      if (e.status === 404) feed.noStream = true; // a server without /stream
      status("offline, retrying…", "error");
      await sleep(3000);
    }
  }
  feed.running = false;
}

// After the phone froze the page, a pending long-poll may be dead: start over.
function restartFeed() {
  feed.abort?.abort();
  runFeed();
}

// ---- notifications --------------------------------------------------------------

const notifyButton = () => $("notify");

function setUpNotifications() {
  const b = notifyButton();
  if (!("Notification" in window) || !window.isSecureContext) return;
  b.hidden = Notification.permission !== "default";
  b.onclick = async () => {
    await Notification.requestPermission();
    b.hidden = Notification.permission !== "default";
  };
}

// One notification per board with what agents posted, unless the operator
// is looking at that board right now.
function notifyAgentPosts(f) {
  if (!("Notification" in window) || Notification.permission !== "granted") return;
  for (const c of f.boards) {
    const lines = c.events
      .filter((e) => e.by === "agent")
      .map((e) =>
        e.type === "decision.add" ? `Decision ${e.id}: ${plainText(e.title).trim()}`
        : e.type === "todo.add" ? `Todo ${e.id}: ${plainText(e.title).trim()}`
        : e.type === "elab.add" ? `Elaboration ${e.rid} answered`
        : null,
      )
      .filter(Boolean);
    if (!lines.length) continue;
    if (!document.hidden && screen?.board === c.board.id) continue;
    showNotification(`${c.board.project} / ${c.board.name}`, lines.join("\n"), c.board.id);
  }
}

async function showNotification(title, body, board) {
  const opts = { body, tag: board, data: { board }, icon: "/icon-192.png" };
  try {
    const reg = await navigator.serviceWorker?.getRegistration();
    if (reg) return await reg.showNotification(title, opts);
    const n = new Notification(title, opts);
    n.onclick = () => {
      window.focus();
      location.hash = `#/b/${board}`;
    };
  } catch {
    // Notifications are a nicety; the board updates either way.
  }
}

// The installable app: a service worker for offline start and notifications.
function setUpWorker() {
  if (!("serviceWorker" in navigator) || !window.isSecureContext) return;
  navigator.serviceWorker.register("/sw.js").catch(() => {});
  // A tapped notification asks the open app to show its board.
  navigator.serviceWorker.addEventListener("message", (e) => {
    if (typeof e.data?.open === "string" && e.data.open.startsWith("/#/")) location.hash = e.data.open.slice(1);
  });
}

// ---- the screen on show -----------------------------------------------------------
//
// { board?: id, refresh(): re-fetch and draw, onFeed?(feed answer) }

let screen = null;

function leave() {
  screen = null;
}

// Coalesces bursts of feed answers into one refresh.
function debounced(fn, ms = 250) {
  let t = null;
  return () => {
    clearTimeout(t);
    t = setTimeout(fn, ms);
  };
}

const act = (board, kind) => api(`/boards/${board}/actions`, { method: "POST", body: JSON.stringify(kind) });

function status(text, kind = "") {
  const s = $("status");
  s.textContent = text;
  s.className = kind;
}

// ---- pairing screen ---------------------------------------------------------

function showLogin(reason) {
  leave();
  $("where").textContent = "";
  const main = $("main");
  main.replaceChildren();
  const form = el("form", "login");
  form.append(
    el("h2", null, "Pair this device"),
    el("p", null, "Scan the QR code from `todai-server pair` on the machine running todai, or paste the token from `todai-server token`."),
  );
  if (reason) form.append(el("p", "error", reason));
  const input = el("input");
  input.type = "password";
  input.autocomplete = "off";
  input.placeholder = "token";
  input.required = true;
  const go = el("button", "primary", "Connect");
  go.type = "submit";
  form.append(input, go);
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    localStorage.setItem(TOKEN_KEY, input.value.trim());
    route();
  });
  main.append(form);
  status("");
}

// ---- board list ---------------------------------------------------------------

// What the list shows, kept across re-renders (filtering doesn't refetch).
const listState = { rows: [], filter: "", showArchived: false, selected: new Set(), notice: "" };

async function showBoards() {
  const me = { refresh: null, onFeed: null };
  me.refresh = async () => {
    const rows = await api("/summaries");
    if (screen !== me) return;
    listState.rows = rows;
    const ids = new Set(rows.map((r) => r.board.id));
    for (const id of listState.selected) if (!ids.has(id)) listState.selected.delete(id);
    keepingFocus(renderBoards);
  };
  // Any change can move a count, an age or the set of boards: re-fetch.
  me.onFeed = debounced(() => me.refresh().catch((e) => status(e.message, "error")));
  screen = me;
  $("where").textContent = "";
  document.title = "todai";
  await me.refresh();
  status("");
}

// "now", "5 min ago", "3 h ago", "2 d ago": as the window and the TUI say it.
function ago(ts) {
  if (!ts) return "never";
  const s = Math.max(0, Math.floor((Date.now() - ts) / 1000));
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86400)} d ago`;
}

// Archive (or unarchive) boards, then show the list again. Reversible, so
// no confirmation for one; the server gives each moved board a new id.
async function relocate(rows, what) {
  const done = [];
  const failed = [];
  for (const r of rows) {
    const verb = r.archived ? "unarchive" : "archive";
    try {
      await api(`/boards/${r.board.id}/${verb}`, { method: "POST", body: "{}" });
      done.push(r.board.name);
    } catch (e) {
      failed.push(`${r.board.name}: ${e.message}`);
    }
  }
  listState.selected.clear();
  listState.notice = [done.length ? `${what} ${done.join(", ")}.` : "", ...failed].filter(Boolean).join(" ");
  await showBoards();
}

// Asks for a name and creates an empty board in `project`, for an agent to
// start with `todai attach NAME`.
async function createBoard(anchor, project) {
  const name = (prompt(`New board in ${project}: name (letters, digits, - and _)`) || "").trim();
  if (!name) return;
  try {
    await api(`/boards/${anchor.board.id}/siblings`, { method: "POST", body: JSON.stringify({ name }) });
    listState.notice = `Created ${name}. Have the agent in that project run: todai attach ${name}`;
  } catch (e) {
    listState.notice = `Couldn't create ${name}: ${e.message}`;
  }
  await showBoards();
}

function renderBoards() {
  const main = $("main");
  main.replaceChildren();
  main.append(el("p", "hint", "Boards this server serves: the ones agents attached or you opened while it ran."));
  const inbox = el("a", "inbox-link", "Everything waiting, on one page →");
  inbox.href = "#/inbox";
  main.append(inbox);
  if (listState.notice) main.append(el("p", "notice", listState.notice));

  const all = listState.rows;
  const tools = el("div", "list-tools");
  const filter = el("input");
  filter.type = "search";
  filter.placeholder = "filter";
  filter.dataset.key = "filter";
  filter.value = listState.filter;
  filter.addEventListener("input", () => {
    listState.filter = filter.value;
    keepingFocus(renderBoards);
  });
  const archived = el("label", "toggle");
  const box = el("input");
  box.type = "checkbox";
  box.checked = listState.showArchived;
  box.addEventListener("change", () => {
    listState.showArchived = box.checked;
    renderBoards();
  });
  archived.append(box, document.createTextNode(" show archived"));
  tools.append(filter, archived);
  const stale = all.filter((r) => r.stale);
  if (stale.length) {
    tools.append(
      button(`Archive stale (${stale.length})`, () => {
        if (confirm(`Archive ${stale.length} stale boards? Nothing waits on them and no agent wrote for a week.`)) {
          relocate(stale, "Archived");
        }
      }),
    );
  }
  const selected = all.filter((r) => listState.selected.has(r.board.id));
  if (selected.length) {
    tools.append(button(`Archive selected (${selected.length})`, () => relocate(selected, "Moved")));
  }
  main.append(tools);

  const needle = listState.filter.toLowerCase();
  const rows = all.filter(
    (r) =>
      (listState.showArchived || !r.archived) &&
      (!needle || r.board.name.toLowerCase().includes(needle) || r.board.project.toLowerCase().includes(needle)),
  );
  if (!rows.length) {
    main.append(
      el(
        "p",
        "empty",
        all.length ? "No board matches." : "No boards yet. Boards show up here once an agent runs `todai attach` while the server runs.",
      ),
    );
    return;
  }
  const byProject = new Map();
  for (const r of rows) byProject.set(r.board.project, [...(byProject.get(r.board.project) || []), r]);
  for (const [project, list] of [...byProject].sort((a, b) => a[0].localeCompare(b[0]))) {
    const head = el("div", "project-head");
    head.append(el("h2", "project", project));
    // New boards go next to one of the project's (prefer a live one): the
    // server places them, the web app never sees paths.
    const anchor = all.find((r) => r.board.project === project && !r.archived) || list[0];
    const add = button("New board", () => createBoard(anchor, project));
    add.className = "small";
    head.append(add);
    main.append(head);
    const ul = el("ul", "boards");
    for (const r of list.sort((x, y) => x.archived - y.archived || x.board.name.localeCompare(y.board.name))) {
      const li = el("li", r.archived ? "archived" : null);
      const pick = el("input");
      pick.type = "checkbox";
      pick.checked = listState.selected.has(r.board.id);
      pick.setAttribute("aria-label", `select ${r.board.name}`);
      pick.addEventListener("change", () => {
        if (pick.checked) listState.selected.add(r.board.id);
        else listState.selected.delete(r.board.id);
        renderBoards();
      });
      const a = el("a", null, r.board.name);
      a.href = `#/b/${r.board.id}`;
      const meta = el("span", "meta");
      meta.append(el("span", r.waiting ? "count" : "count zero", r.waiting ? `${r.waiting} waiting` : "nothing waiting"));
      if (r.asked) meta.append(el("span", "badge", `${r.asked} asked`));
      if (r.stale) meta.append(el("span", "badge", "stale"));
      if (r.archived) meta.append(el("span", "badge", "archived"));
      meta.append(el("span", "ago", ago(r.last)));
      const move = button(r.archived ? "Unarchive" : "Archive", () =>
        relocate([r], r.archived ? "Unarchived" : "Archived"),
      );
      move.className = "small";
      li.append(pick, a, meta, move);
      ul.append(li);
    }
    main.append(ul);
  }
}

// Items that wait for the operator: pending decisions and open todos.
const waiting = (v) => v.decisions.filter((d) => d.status === "pending").length + v.todos.filter((t) => !t.done).length;

// ---- one board ----------------------------------------------------------------

const drafts = new Map(); // input key -> text, kept across re-renders

async function showBoard(id) {
  const me = { board: id, refresh: null, onFeed: null };
  me.refresh = async () => {
    const view = await api(`/boards/${id}`);
    if (screen === me) render(view);
  };
  me.onFeed = (f) => {
    if (f.removed?.includes(id)) {
      status("this board is no longer served (archived or forgotten)", "error");
      return;
    }
    if (f.boards.some((c) => c.board.id === id)) me.refresh().catch((e) => status(e.message, "error"));
  };
  screen = me;
  await me.refresh();
  status("live", "ok");
}

function render(v) {
  keepingFocus(() => drawBoard(v));
}

function drawBoard(v) {
  $("where").textContent = `${v.board.project} / ${v.board.name}`;
  document.title = `${waiting(v) ? `(${waiting(v)}) ` : ""}${v.board.name} · todai`;
  const openDetails = new Set([...document.querySelectorAll("details[open]")].map((d) => d.dataset.key));

  const main = $("main");
  main.replaceChildren();
  const titles = new Map([...v.decisions, ...v.todos].map((i) => [i.id, plainText(i.title).trim()]));

  const pendingD = v.decisions.filter((d) => d.status === "pending");
  const openT = v.todos.filter((t) => !t.done);
  main.append(section("Waiting for you", pendingD.length + openT.length, [
    ...pendingD.map((d) => decisionCard(v, d)),
    ...openT.map((t) => todoCard(v, t)),
  ], "Nothing is waiting for you."));

  const threads = threadsOf(v.elaborations);
  main.append(section("Elaborations", threads.length, threads.map((t) => threadCard(v, t, titles)), "No elaborations requested."));

  const doneD = v.decisions.filter((d) => d.status !== "pending");
  const doneT = v.todos.filter((t) => t.done);
  const done = el("details", "section");
  done.dataset.key = "done";
  done.open = openDetails.has("done");
  const sum = el("summary");
  sum.append(el("h2", null, "Done"), el("span", "count", String(doneD.length + doneT.length)));
  done.append(sum, ...doneD.map((d) => resolvedCard(v, d)), ...doneT.map((t) => doneCard(v, t)));
  main.append(done);
}

// ---- everything waiting, across boards ------------------------------------------

async function showInbox() {
  const views = new Map(); // board id -> view
  const me = { refresh: null, onFeed: null };
  const draw = () => {
    if (screen !== me) return;
    keepingFocus(() => drawInbox([...views.values()]));
  };
  const load = async (id) => {
    try {
      views.set(id, await api(`/boards/${id}`));
    } catch (e) {
      if (e.status === 404) views.delete(id);
      else throw e;
    }
  };
  me.refresh = async () => {
    const rows = await api("/summaries");
    views.clear();
    await Promise.all(rows.filter((r) => !r.archived).map((r) => load(r.board.id)));
    draw();
  };
  me.onFeed = async (f) => {
    for (const id of f.removed || []) views.delete(id);
    await Promise.all(f.boards.map((c) => load(c.board.id))).catch((e) => status(e.message, "error"));
    draw();
  };
  screen = me;
  $("where").textContent = "everything waiting";
  await me.refresh();
  status("live", "ok");
}

function drawInbox(views) {
  const total = views.reduce((n, v) => n + waiting(v), 0);
  document.title = `${total ? `(${total}) ` : ""}waiting · todai`;
  const main = $("main");
  main.replaceChildren();
  const back = el("a", "inbox-link", "← all boards");
  back.href = "#/";
  main.append(back);
  const busy = views.filter((v) => waiting(v)).sort((a, b) => (a.board.project + a.board.name).localeCompare(b.board.project + b.board.name));
  if (!busy.length) {
    main.append(el("p", "empty", "Nothing is waiting for you on any board."));
    return;
  }
  for (const v of busy) {
    const head = el("div", "project-head");
    const a = el("a", "board-link", `${v.board.project} / ${v.board.name}`);
    a.href = `#/b/${v.board.id}`;
    head.append(a, el("span", "count", String(waiting(v))));
    main.append(head);
    for (const d of v.decisions.filter((d) => d.status === "pending")) main.append(decisionCard(v, d));
    for (const t of v.todos.filter((t) => !t.done)) main.append(todoCard(v, t));
  }
}

function section(title, count, cards, empty) {
  const s = el("section", "section");
  const h = el("div", "section-head");
  h.append(el("h2", null, title), el("span", "count", String(count)));
  s.append(h);
  if (cards.length) s.append(...cards);
  else s.append(el("p", "empty", empty));
  return s;
}

// The id, title and context of a decision or todo.
function itemBody(item, struck) {
  const body = el("div", "item-body");
  const head = el("div", "item-head");
  head.append(el("span", "id", item.id));
  const title = el("div", "title" + (struck ? " struck" : ""));
  title.append(renderMarkdown(item.title));
  head.append(title);
  body.append(head);
  if (item.context) {
    const ctx = el("div", "context md" + (struck ? " struck" : ""));
    ctx.append(renderMarkdown(item.context));
    body.append(ctx);
  }
  return body;
}

const card = (id) => {
  const c = el("article", "card");
  c.dataset.item = id;
  return c;
};

function draftInput(key, multiline, placeholder) {
  const input = el(multiline ? "textarea" : "input");
  if (multiline) input.rows = 1;
  input.placeholder = placeholder;
  input.dataset.key = key;
  input.value = drafts.get(key) || "";
  input.addEventListener("input", () => drafts.set(key, input.value));
  return input;
}

// Runs an action; on failure keeps the draft and says why.
async function send(v, kind, draftKey, btn) {
  if (btn) btn.disabled = true;
  try {
    await act(v.board.id, kind);
    if (draftKey) drafts.delete(draftKey);
    status("sent", "ok");
    await screen?.refresh();
  } catch (e) {
    if (e instanceof AuthError) return showLogin(e.message);
    status(e.message, "error");
    if (btn) btn.disabled = false;
  }
}

function decisionCard(v, d) {
  const c = card(d.id);
  c.append(itemBody(d));
  const key = `resolve:${v.board.id}:${d.id}`;
  const input = draftInput(key, true, "your answer");
  if (d.options.length) {
    const opts = el("div", "options");
    d.options.forEach((o) => {
      const chip = button(o, () => {
        input.value = o;
        drafts.set(key, o);
        opts.querySelectorAll("button").forEach((b) => b.classList.toggle("selected", b === chip));
      }, "chip" + (drafts.get(key) === o ? " selected" : ""));
      opts.append(chip);
    });
    c.append(opts);
  }
  const resolve = button("Resolve", () => {
    const text = input.value.trim();
    if (text) send(v, { type: "decision.resolve", id: d.id, text }, key, resolve);
    else input.focus();
  }, "primary");
  const row = el("div", "row");
  row.append(input, resolve, askButton(v, d, c));
  c.append(row);
  return c;
}

function todoCard(v, t) {
  const c = card(t.id);
  c.append(itemBody(t));
  const key = `done:${v.board.id}:${t.id}`;
  const input = draftInput(key, false, "note for the agent (optional)");
  const done = button("Done", () => {
    const note = input.value.trim();
    send(v, { type: "todo.done", id: t.id, ...(note ? { note } : {}) }, key, done);
  }, "primary");
  const row = el("div", "row");
  row.append(input, done, askButton(v, t, c));
  c.append(row);
  return c;
}

function resolvedCard(v, d) {
  const c = card(d.id);
  c.classList.add("finished");
  c.append(itemBody(d, d.status === "withdrawn"));
  c.append(d.status === "resolved"
    ? el("p", "resolution", `» ${d.resolution}`)
    : el("p", "muted", "withdrawn by the agent"));
  const row = el("div", "row end");
  if (d.status === "resolved") row.append(button("Reopen", (e) => send(v, { type: "decision.reopen", id: d.id }, null, e.target)));
  row.append(trashButton(v, d.id), askButton(v, d, c));
  c.append(row);
  return c;
}

function doneCard(v, t) {
  const c = card(t.id);
  c.classList.add("finished");
  c.append(itemBody(t));
  if (t.note) c.append(el("p", "muted", `note: ${t.note}`));
  const row = el("div", "row end");
  row.append(button("Undo", (e) => send(v, { type: "todo.undone", id: t.id }, null, e.target)), trashButton(v, t.id), askButton(v, t, c));
  c.append(row);
  return c;
}

const trashButton = (v, id) => button("Trash", (e) => {
  if (confirm(`Remove ${id} and its elaborations from the board?`)) send(v, { type: "item.trash", ids: [id] }, null, e.target);
});

// ---- elaborations ---------------------------------------------------------------

// Groups requests into threads (follow `parent` to the root), oldest turn
// first; the most recently active thread first.
function threadsOf(elabs) {
  const byRid = new Map(elabs.map((e) => [e.rid, e]));
  const rootOf = (e) => {
    let r = e;
    const seen = new Set();
    while (r.parent && byRid.has(r.parent) && !seen.has(r.parent)) {
      seen.add(r.rid);
      r = byRid.get(r.parent);
    }
    return r.rid;
  };
  const threads = new Map();
  elabs.forEach((e, i) => {
    const root = rootOf(e);
    if (!threads.has(root)) threads.set(root, { turns: [], last: 0 });
    const t = threads.get(root);
    t.turns.push(e);
    t.last = i;
  });
  return [...threads.values()].sort((a, b) => b.last - a.last).map((t) => t.turns);
}

function threadCard(v, turns, titles) {
  const root = turns[0];
  const newest = turns[turns.length - 1];
  const c = card(root.rid);
  c.classList.add("thread");
  const head = el("div", "item-head");
  head.append(el("span", "id", root.rid), el("span", "id", `on ${root.target}`), el("span", "muted target", titles.get(root.target) || ""));
  c.append(head);
  turns.forEach((r, i) => {
    if (i) c.append(el("hr"));
    const parent = turns.find((t) => t.rid === r.parent);
    if (parent && parent.answer && parent.answer.trim() === r.selection.trim()) c.append(el("p", "quote muted", "follow-up on the answer above"));
    else c.append(el("p", "quote", `“${r.selection}”`));
    if (r.note) c.append(el("p", "note", r.note));
    if (r.answer) {
      const a = el("div", "answer md");
      a.dataset.rid = r.rid;
      a.append(renderMarkdown(r.answer));
      c.append(a);
    } else {
      c.append(el("p", "muted waiting", "waiting for the agent's answer…"));
    }
  });
  const row = el("div", "row end");
  if (newest.answer) {
    row.append(button("Follow up", () => {
      const sel = selectionIn(c);
      // Reply to the turn whose answer holds the selection, else the newest.
      const holder = sel && turns.slice().reverse().find((t) => t.answer && plainText(t.answer).includes(sel));
      const parent = holder || newest;
      openAsk(v, root.target, sel || plainText(parent.answer).trim(), parent.rid);
    }));
  }
  row.append(trashButton(v, root.rid));
  c.append(row);
  return c;
}

// ---- asking for an elaboration ---------------------------------------------------

// Phones clear the selection when a button is tapped, so remember the last
// non-empty one and the card it was in.
let lastSelection = { text: "", card: null };
document.addEventListener("selectionchange", () => {
  const s = document.getSelection();
  const text = s ? s.toString().trim() : "";
  if (!text) return;
  const node = s.anchorNode && (s.anchorNode.nodeType === 1 ? s.anchorNode : s.anchorNode.parentElement);
  lastSelection = { text, card: node?.closest(".card") || null };
});

function selectionIn(cardEl) {
  const s = document.getSelection();
  const now = s ? s.toString().trim() : "";
  if (now && cardEl.contains(s.anchorNode)) return now;
  return lastSelection.card === cardEl ? lastSelection.text : "";
}

function askButton(v, item, cardEl) {
  return button("Ask", () => {
    const sel = selectionIn(cardEl);
    openAsk(v, item.id, sel || plainText([item.title, item.context].filter(Boolean).join("\n\n")).trim(), null);
  });
}

function openAsk(v, target, selection, parent) {
  const dlg = $("ask");
  $("ask-selection").textContent = selection;
  const note = $("ask-note");
  note.value = "";
  dlg.hidden = false;
  note.focus();
  const form = $("ask-form");
  const close = () => {
    dlg.hidden = true;
    form.onsubmit = null;
    lastSelection = { text: "", card: null };
  };
  $("ask-cancel").onclick = close;
  form.onsubmit = (e) => {
    e.preventDefault();
    const n = note.value.trim();
    close();
    send(v, { type: "elab.request", rid: "", target, selection, ...(n ? { note: n } : {}), ...(parent ? { parent } : {}) });
  };
}

// ---- routing ----------------------------------------------------------------------

async function route() {
  takeTokenFromUrl();
  if (!token()) return showLogin();
  runFeed();
  const m = location.hash.match(/^#\/b\/([0-9a-f]{16})$/);
  try {
    if (m) await showBoard(m[1]);
    else if (location.hash === "#/inbox") await showInbox();
    else await showBoards();
  } catch (e) {
    if (e instanceof AuthError) {
      localStorage.removeItem(TOKEN_KEY);
      return showLogin(`${e.message}; pair again.`);
    }
    status(e.message, "error");
  }
}

window.addEventListener("hashchange", route);
// Back from the background on a phone: the long-poll may have been frozen,
// and the screen may be out of date.
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState !== "visible" || !token()) return;
  restartFeed();
  screen?.refresh().catch((e) => status(e.message, "error"));
});

// ---- keyboard ---------------------------------------------------------------------
//
// For a desktop browser or a phone with a keyboard. Documented in
// docs/server.md ("Web app"); the window and the TUI have their own keys.

const KEYS = [
  ["j / k", "next / previous card or board"],
  ["Enter", "answer the card (focus its field) · open the board"],
  ["1–9", "pick that option of the decision"],
  ["Ctrl+Enter", "in a field: send (Resolve / Done)"],
  ["Esc", "leave the field · close this help"],
  ["e", "ask about the card"],
  ["h", "board list"],
  ["a", "everything waiting"],
  ["/", "filter the board list"],
  ["?", "this help"],
];

let current = -1; // index into navigable()

const navigable = () => [...$("main").querySelectorAll("article.card, ul.boards li")];

function setCurrent(i) {
  const items = navigable();
  items.forEach((x) => x.classList.remove("current"));
  if (!items.length) return (current = -1);
  current = Math.max(0, Math.min(i, items.length - 1));
  items[current].classList.add("current");
  items[current].scrollIntoView({ block: "nearest" });
}

function toggleHelp(show) {
  let box = $("keys");
  if (!box) {
    box = el("div", "keys");
    box.id = "keys";
    box.hidden = true;
    const t = el("table");
    for (const [k, what] of KEYS) {
      const tr = el("tr");
      tr.append(el("td", "key", k), el("td", null, what));
      t.append(tr);
    }
    box.append(el("h2", null, "Keys"), t, el("p", "muted", "Esc or ? to close"));
    box.addEventListener("click", () => toggleHelp(false));
    document.body.append(box);
  }
  box.hidden = !(show ?? box.hidden);
}

document.addEventListener("keydown", (e) => {
  const t = e.target;
  const typing = t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement;
  if (typing) {
    if (e.key === "Escape") t.blur();
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      t.closest(".card")?.querySelector("button.primary")?.click();
    }
    return;
  }
  if (e.ctrlKey || e.metaKey || e.altKey || !$("ask").hidden) return;
  const items = navigable();
  const cur = items[current];
  const k = e.key;
  if (k === "Escape") return toggleHelp(false);
  if (k === "?") return toggleHelp();
  if (k === "j") return setCurrent(current + 1);
  if (k === "k") return setCurrent(current < 0 ? 0 : current - 1);
  if (k === "h") return (location.hash = "#/");
  if (k === "a") return (location.hash = "#/inbox");
  if (k === "/") {
    const f = $("main").querySelector("input[type=search]");
    if (f) {
      e.preventDefault();
      f.focus();
    }
    return;
  }
  if (!cur) return;
  if (k === "Enter") {
    e.preventDefault();
    const link = cur.querySelector("a");
    if (cur.tagName === "LI" && link) return link.click();
    cur.querySelector("textarea, input:not([type=checkbox])")?.focus();
    return;
  }
  if (/^[1-9]$/.test(k)) {
    cur.querySelectorAll(".options button")[Number(k) - 1]?.click();
    return;
  }
  if (k === "e") {
    [...cur.querySelectorAll("button")].find((b) => b.textContent === "Ask")?.click();
  }
});

// A new screen starts with nothing marked.
new MutationObserver(() => {
  const items = navigable();
  if (current >= items.length) current = items.length - 1;
  if (current >= 0) items[current]?.classList.add("current");
}).observe($("main"), { childList: true });
window.addEventListener("hashchange", () => (current = -1));

setUpWorker();
setUpNotifications();
route();
