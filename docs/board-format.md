# Board format

A board is an append-only [JSON Lines](https://jsonlines.org) file: one event
per line, never rewritten. The current board is what you get by applying the
events in order. Always write through `todai`; it validates every event.

```json
{"seq":1,"ts":1792750000000,"by":"agent","type":"decision.add","id":"d1","title":"Use egui or iced?","options":["egui","iced"]}
{"seq":2,"ts":1792750004000,"by":"operator","type":"decision.resolve","id":"d1","text":"egui"}
{"seq":3,"ts":1792750009000,"by":"agent","type":"ack","upto_seq":2}
```

## Common fields

| Field | |
|---|---|
| `seq` | 1, 2, 3, … strictly increasing, assigned by the writer under the file lock |
| `ts` | Unix time in milliseconds |
| `by` | `agent` or `operator`; each event type may only come from one of them |
| `type` | one of the types below |

## Events

| `type` | By | Fields | Meaning |
|---|---|---|---|
| `decision.add` | agent | `id` (`dN`), `title`, `context`?, `options`? | ask the operator to decide |
| `decision.resolve` | operator | `id`, `text` | the answer (may resolve again after a reopen) |
| `decision.reopen` | operator | `id` | the previous answer no longer holds |
| `decision.withdraw` | agent | `id` | the question is no longer needed |
| `todo.add` | agent | `id` (`tN`), `title`, `context`? | a manual step for the operator |
| `todo.done` | operator | `id`, `note`? | the step is done |
| `todo.undone` | operator | `id` | it isn't after all |
| `elab.request` | operator | `rid` (`rN`), `target` (item id), `selection`, `note`? | "explain this part" |
| `elab.add` | agent | `rid`, `text_md` | the explanation, as markdown |
| `item.trash` | operator | `ids` | remove finished items (resolved or withdrawn decisions, done todos) with their elaborations, and/or elaborations (`rN`) on their own |
| `ack` | agent | `upto_seq` | the agent has seen operator events up to this seq |

Ids are assigned by the writer: `d`, `t` and `r` followed by a counter.
Trashed ids are never reused.

## Rules

- **Validation.** An event is rejected if its actor is wrong or it refers to
  something missing: resolving an unknown or withdrawn decision, reopening one
  that isn't resolved, answering an elaboration that wasn't requested, acking
  past the last seq. Rejected events are never written.
- **Concurrency.** Every writer takes an exclusive `flock` on the file, re-reads
  it, validates, assigns `seq` and ids, and appends one line. The agent's CLI
  calls, hooks and the window can write at the same time safely.
- **Crashes.** A torn last line (no trailing newline) is ignored on read.
- **Trash.** Only finished items and elaborations can be trashed; trashing an
  unanswered elaboration cancels it. Operator actions on a
  trashed item are rejected; the agent's late `decision.withdraw` or
  `elab.add` for one is accepted and ignored, so agents never fail because
  the operator tidied up.
- **Delivery.** Operator events with `seq` above the highest `ack` are
  *unprocessed* (except `item.trash`, which agents are never told about). `todai inbox`, `todai wait` and the agent adapters print them
  and then ack exactly the events they printed, so each is delivered once.
