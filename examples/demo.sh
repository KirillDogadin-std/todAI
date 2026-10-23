#!/usr/bin/env bash
# Recreates examples/demo.jsonl with a mix of pending, resolved and elaborated
# items, then opens it. Re-run any time to reset the board.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q --release
T=./target/release/todai
F=examples/demo.jsonl
rm -f "$F"

$T add-decision $F "Default location for board files?" \
  -c "Repo-local keeps boards next to the code and lets git track them; a global dir keeps repos clean but boards get lost when you move a project." \
  -o ".agents/todai/ in repo" -o "~/.local/share/todai" -o "both, repo wins" >/dev/null
$T add-decision $F "Should the operator be able to add their own todos?" >/dev/null
$T add-decision $F "Withdrawn decisions: hide or strike through?" -o hide -o strike >/dev/null
$T add-decision $F "Notify on new decisions?" \
  -c "A desktop notification via notify-send when an agent posts a decision while the window is unfocused." \
  -o yes -o no -o "only when blocked" >/dev/null
$T add-decision $F "Max title length the agent may post?" -o 60 -o 80 -o 120 >/dev/null
$T add-decision $F "Use glow or wgpu backend?" -o glow -o wgpu >/dev/null

$T add-todo $F "Create the GitHub repo and push" -c "Agent has no gh auth on this machine." >/dev/null
$T add-todo $F "Add a Hyprland window rule for class 'todai'" \
  -c "Example: float, size 900x700, or pin it to a special workspace." >/dev/null
$T add-todo $F "Install noto-fonts-emoji" >/dev/null

# Some history so Done and Elaborations aren't empty.
$T resolve $F d6 glow
$T done $F t3 -n "installed, emoji render now"
$T ask $F d1 "lets git track them" -n "would that clutter git diffs?" >/dev/null
$T elaborate $F r1 - <<'MD' >/dev/null
Only if you commit them. Two options:

- add `.agents/todai/` to `.gitignore` and keep boards local
- **commit them** as a lightweight decision record, reviewed with the code

The log is append-only, so diffs are just new lines at the end.
MD
$T ask $F t1 "no gh auth" -n "can't you use a token?" >/dev/null
$T ack $F

echo "Board: $F  (agent side: $T inbox $F / $T wait $F)"
exec $T "$F"
