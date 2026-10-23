// todai adapter for pi, installed by `todai setup`. Inert until the session
// runs `todai attach`; then it keeps a `todai wait` running on that board and
// delivers each operator update into the conversation: steering the agent if
// it is working, starting a turn if it is idle. Remove with `todai setup --remove`.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { spawn, type ChildProcess } from "node:child_process";

const TODAI = "__TODAI_BIN__";
const MARKER = "todai: attached ";

export default function (pi: ExtensionAPI) {
  let board: string | undefined;
  let waiter: ChildProcess | undefined;
  let stopped = false;

  const watch = () => {
    if (!board || stopped) return;
    // `todai wait` exits once it has printed (and acked) new updates.
    const child = spawn(TODAI, ["wait", board], { stdio: ["ignore", "pipe", "ignore"] });
    waiter = child;
    let out = "";
    child.stdout?.on("data", (d) => (out += d));
    child.on("exit", (code) => {
      const current = waiter === child;
      if (current) waiter = undefined;
      if (stopped || !current) return;
      if (code === 0 && out.trim()) {
        pi.sendMessage(
          { customType: "todai", content: out.trim(), display: true },
          { triggerTurn: true, deliverAs: "steer" },
        );
      }
      // Back off if todai itself is failing instead of spinning.
      setTimeout(watch, code === 0 ? 0 : 5000);
    });
  };

  pi.on("tool_result", (event) => {
    if (event.toolName !== "bash") return;
    const text = event.content.map((c) => (c.type === "text" ? c.text : "")).join("\n");
    const line = text.split("\n").reverse().find((l) => l.startsWith(MARKER));
    const next = line?.slice(MARKER.length).trim();
    if (!next || next === board) return;
    board = next;
    waiter?.kill();
    watch();
  });

  pi.on("session_shutdown", () => {
    stopped = true;
    waiter?.kill();
  });
}
