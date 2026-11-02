// A small markdown renderer for board text. It builds DOM nodes and only
// ever sets text through textContent, so nothing an agent writes is parsed
// as HTML. Like the desktop cards, line breaks are kept as written.
//
// Supports: fenced code blocks, headings, bullet and numbered lists, block
// quotes, paragraphs, `code`, **bold**, *italic*, and http(s) links.

const el = (tag, cls) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  return e;
};

const INLINE = /(`+)([\s\S]*?)\1|\*\*([^*]+)\*\*|__([^_]+)__|\*([^*\s][^*]*)\*|\b_([^_\s][^_]*)_\b|\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g;

function inline(parent, text) {
  let last = 0;
  for (const m of text.matchAll(INLINE)) {
    if (m.index > last) parent.append(text.slice(last, m.index));
    last = m.index + m[0].length;
    if (m[1]) {
      const c = el("code");
      c.textContent = m[2];
      parent.append(c);
    } else if (m[3] || m[4]) {
      const b = el("strong");
      inline(b, m[3] || m[4]);
      parent.append(b);
    } else if (m[5] || m[6]) {
      const i = el("em");
      inline(i, m[5] || m[6]);
      parent.append(i);
    } else {
      const a = el("a");
      a.href = m[8];
      a.rel = "noreferrer noopener";
      a.target = "_blank";
      inline(a, m[7]);
      parent.append(a);
    }
  }
  if (last < text.length) parent.append(text.slice(last));
}

// Appends `lines` to `parent`, one <br> between them.
function lines(parent, ls) {
  ls.forEach((l, i) => {
    if (i) parent.append(el("br"));
    inline(parent, l);
  });
}

const BULLET = /^(\s*)[-*+]\s+(.*)$/;
const NUMBER = /^(\s*)(\d+)[.)]\s+(.*)$/;
const FENCE = /^\s*(```|~~~)/;

export function renderMarkdown(src) {
  const out = document.createDocumentFragment();
  const all = (src || "").replace(/\r\n?/g, "\n").split("\n");
  let i = 0;
  while (i < all.length) {
    const line = all[i];
    if (!line.trim()) {
      i++;
      continue;
    }
    const fence = line.match(FENCE);
    if (fence) {
      const code = [];
      i++;
      while (i < all.length && !all[i].trim().startsWith(fence[1])) code.push(all[i++]);
      i++; // closing fence
      const pre = el("pre");
      const c = el("code");
      c.textContent = code.join("\n");
      pre.append(c);
      out.append(pre);
      continue;
    }
    const h = line.match(/^(#{1,6})\s+(.*)$/);
    if (h) {
      const e = el("h" + Math.min(6, h[1].length + 2));
      inline(e, h[2]);
      out.append(e);
      i++;
      continue;
    }
    if (BULLET.test(line) || NUMBER.test(line)) {
      const numbered = NUMBER.test(line);
      const list = el(numbered ? "ol" : "ul");
      if (numbered) list.start = Number(line.match(NUMBER)[2]);
      let item = null;
      while (i < all.length && all[i].trim() && !FENCE.test(all[i])) {
        const m = all[i].match(numbered ? NUMBER : BULLET);
        if (m && (!m[1] || !item)) {
          item = el("li");
          inline(item, numbered ? m[3] : m[2]);
          list.append(item);
        } else {
          // A continuation line, or a nested item: kept on its own line.
          item.append(el("br"));
          inline(item, all[i].trim());
        }
        i++;
      }
      out.append(list);
      continue;
    }
    if (line.startsWith(">")) {
      const q = [];
      while (i < all.length && all[i].startsWith(">")) q.push(all[i++].replace(/^>\s?/, ""));
      const b = el("blockquote");
      lines(b, q);
      out.append(b);
      continue;
    }
    const para = [];
    while (i < all.length && all[i].trim() && !FENCE.test(all[i]) && !/^#{1,6}\s/.test(all[i])
      && !BULLET.test(all[i]) && !NUMBER.test(all[i]) && !all[i].startsWith(">")) {
      para.push(all[i++]);
    }
    const p = el("p");
    lines(p, para);
    out.append(p);
  }
  return out;
}

// The text a reader sees, for "ask about the whole item".
export function plainText(src) {
  const div = document.createElement("div");
  div.append(renderMarkdown(src));
  return div.innerText || div.textContent;
}
