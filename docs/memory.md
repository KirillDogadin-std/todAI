# Decision memory

*Early: cards, keys and finding them by similar wording work; links between
cards, axiom confirmation and the agent protocol are still to come. Agents
don't use it on their own yet: `todai guide` doesn't mention it.*

Long projects pile up decisions, and the reasoning behind them sits in
resolved board items that nobody reads again. Agents re-ask settled
questions and start every session blank. The memory keeps each decision as
a **card**, found again by short **keys**.

## Where it lives

One SQLite database per project:
`$XDG_STATE_HOME/todai/memory/NAME-HASH.sqlite` (else under
`~/.local/state`). NAME is the project folder (the git repository root, as
for boards), HASH tells same-named folders apart. Moving the project starts
an empty memory. Nothing is written into the repository.

Every change is an event appended to a log that is never edited; cards are
derived from it. The store is the `todai-memory` crate, which depends on
nothing else in todai.

## Cards

| Field | |
|---|---|
| id | `m` + 8 hex digits (`m167f8289`); any unique prefix of 4+ characters works |
| class | `axiom` (must always hold), `context` (holds in situations like this; default), `one_off` |
| gist | the decision in one line, at most 120 characters |
| context | the situation it was decided in (required) |
| why | the reasons (required) |
| applies / not when | when it holds and when it doesn't |
| rejected | alternatives, and why not |
| keys | 1-5 short phrases (at most 5 words, 40 characters) it is found by |
| from | the board item it was decided in |

Keys are lowercased with single spaces and no punctuation at the ends
(`GPU  Backend.` → `gpu backend`), and not stemmed. The vocabulary is
kept small on purpose, so the same idea is always filed under the same key:

- a new key that is an existing one in other words (`memory usages`,
  `Memory Usage`) is **replaced** by it, and `add` says so;
- a new key that may be a **synonym** (`ram usage` next to `memory usage`)
  is **refused**, naming the close keys: reuse one, or keep yours with
  `--new-key "ram usage"`;
- anything else is a new key.

An axiom recorded by an agent is **proposed** until the operator confirms
it; the operator's own (`--operator`) are active at once.

## Commands

```sh
todai memory add --gist "egui over iced for the window: idle memory" \
  --key "gui toolkit,memory usage" --from d3 <<'MD'
## Context
Picking the toolkit for a board window that stays open all day.
## Why
egui idles around 40 MB, iced around 90 MB.
## Rejected
iced: memory. gtk4: system build deps.
## Not when
A one-shot tool that exits quickly.
MD
# prints the new id: m167f8289

todai memory find "RAM usage" "graphics backend" # cards whose keys are near any phrase, best first
todai memory show m167f                          # one card in full
todai memory keys                                # the vocabulary, with card counts
```

`--from d3` finds the project board where `d3` changed most recently (item
ids restart on every board); write `BOARD/d3` to name it. Sections on stdin
are `## Context`, `## Why`, `## Applies`, `## Not when`, `## Rejected`.

`find` takes short phrases (2-6 words) in any wording. Each lands on the
nearest existing keys, and those keys select cards exactly. Cards are ranked
by how many phrases reached them, then how close the key was, then axioms
before context before one-offs, then newest. Each card prints as a short block
(about 100 tokens: gist, the start of each field, keys, origin) plus `via:`,
the phrase → key that found it. "No key near these phrases" is an answer
too: nothing has been decided about this yet. `show` prints everything.
`--json` on `show`, `find` and `keys` is for scripts.

## How similar wording is found

A built-in model, [potion-base-8M](https://huggingface.co/minishlab/potion-base-8M)
(MIT), turns a phrase into a vector: the average of its word pieces'
vectors. It runs locally in about 10 ms, needs no network or GPU, and adds
8 MB to the binary. It matches **shared words** well and true synonyms
less so:

| pair | similarity |
|---|---|
| `gpu backend` / `gpu backends` | 0.997 |
| `memory usage` / `memory use` | 0.887 |
| `memory usage` / `ram usage` | 0.74 |
| `memory usage` / `cpu usage` | 0.69 |
| `gui toolkit` / `ui framework` | 0.42 |
| `memory usage` / `release workflow` | 0.07 |

Thresholds: replace a key at **0.88** or above, ask between **0.65** and
0.88, and find keys at **0.5** or above. Synonyms that share no word
(`ui framework`) can be missed, which is why it pays to try two or three
phrasings, and `todai memory keys` shows the vocabulary.

## Changing the embedder

The model is meant to be swappable. Everything goes through the `Embedder`
trait (`crates/todai-memory/src/embed/mod.rs`), and **vectors are only a
cache**: keys are stored as text, vectors per embedder id next to them.
A different embedder computes its own vectors the first time it is used,
with no migration and nothing lost, and switching back is free. To swap,
implement `Embedder`, return it from `embed::default()`, and retune its
three thresholds on real keys. Similarity scales differ between models.
