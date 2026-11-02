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
  if (!res.ok) throw new Error(body?.error || `HTTP ${res.status}`);
  return body;
}

const act = (board, kind) => api(`/boards/${board}/actions`, { method: "POST", body: JSON.stringify(kind) });

function status(text, kind = "") {
  const s = $("status");
  s.textContent = text;
  s.className = kind;
}

// ---- pairing screen ---------------------------------------------------------

function showLogin(reason) {
  stopFollowing();
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

async function showBoards() {
  stopFollowing();
  $("where").textContent = "";
  const boards = await api("/boards");
  const views = await Promise.all(boards.map((b) => api(`/boards/${b.id}`).catch(() => null)));
  const main = $("main");
  main.replaceChildren();
  if (!boards.length) {
    main.append(el("p", "empty", "No boards yet. Boards show up here once an agent runs `todai attach` while the server runs."));
  }
  const byProject = new Map();
  boards.forEach((b, i) => {
    const list = byProject.get(b.project) || [];
    list.push([b, views[i]]);
    byProject.set(b.project, list);
  });
  for (const [project, list] of [...byProject].sort((a, b) => a[0].localeCompare(b[0]))) {
    main.append(el("h2", "project", project));
    const ul = el("ul", "boards");
    for (const [b, v] of list.sort((x, y) => x[0].name.localeCompare(y[0].name))) {
      const li = el("li");
      const a = el("a", null, b.name);
      a.href = `#/b/${b.id}`;
      li.append(a);
      if (v) {
        const n = waiting(v);
        li.append(el("span", n ? "count" : "count zero", n ? `${n} waiting` : "nothing waiting"));
      }
      ul.append(li);
    }
    main.append(ul);
  }
  status("");
}

// Items that wait for the operator: pending decisions and open todos.
const waiting = (v) => v.decisions.filter((d) => d.status === "pending").length + v.todos.filter((t) => !t.done).length;

// ---- one board ----------------------------------------------------------------

let following = null; // { id, stop }
const drafts = new Map(); // input key -> text, kept across re-renders

function stopFollowing() {
  if (following) following.stop = true;
  following = null;
}

async function showBoard(id) {
  stopFollowing();
  const me = { id, stop: false };
  following = me;
  let view = await api(`/boards/${id}`);
  render(view);
  // Long-poll for changes; re-fetch the folded board when anything happens.
  let after = view.last_seq;
  while (!me.stop) {
    try {
      const ev = await api(`/boards/${id}/events?after=${after}&wait=${LONG_POLL}`);
      if (me.stop) return;
      status("live", "ok");
      if (ev.events.length || ev.reset) {
        view = await api(`/boards/${id}`);
        after = view.last_seq;
        if (!me.stop) render(view);
      }
    } catch (e) {
      if (e instanceof AuthError) return showLogin(e.message);
      status("offline, retrying…", "error");
      await new Promise((r) => setTimeout(r, 3000));
    }
  }
}

function render(v) {
  $("where").textContent = `${v.board.project} / ${v.board.name}`;
  document.title = `${waiting(v) ? `(${waiting(v)}) ` : ""}${v.board.name} · todai`;
  const focused = document.activeElement?.dataset?.key;
  const caret = focused ? [document.activeElement.selectionStart, document.activeElement.selectionEnd] : null;
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

  if (focused) {
    const input = main.querySelector(`[data-key="${CSS.escape(focused)}"]`);
    if (input) {
      input.focus({ preventScroll: true });
      if (caret && input.setSelectionRange) input.setSelectionRange(...caret);
    }
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
    render(await api(`/boards/${v.board.id}`));
  } catch (e) {
    if (e instanceof AuthError) return showLogin(e.message);
    status(e.message, "error");
    if (btn) btn.disabled = false;
  }
}

function decisionCard(v, d) {
  const c = card(d.id);
  c.append(itemBody(d));
  const key = `resolve:${d.id}`;
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
  const key = `done:${t.id}`;
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
  const m = location.hash.match(/^#\/b\/([0-9a-f]{16})$/);
  try {
    if (m) await showBoard(m[1]);
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
// Back from the background on a phone: the long-poll may have been frozen.
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible" && $("ask").hidden && !location.hash.startsWith("#/b/")) route();
});
route();
