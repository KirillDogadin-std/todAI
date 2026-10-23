#!/usr/bin/env bash
# Recreates examples/demo.jsonl, a board that exercises every feature, then
# opens it. Re-run any time to reset the board.
#
#   Pending       decisions with options and context, todos
#   Done          resolved, withdrawn and finished items (select, shift-click, delete)
#   Elaborations  a long answer to zoom (K / Zoom / double-click), an old one
#                 that starts collapsed, and one still waiting for the agent
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q --release
T=./target/release/todai
F=examples/demo.jsonl
rm -f "$F"
q() { "$@" >/dev/null; }

# Pending
q $T add-decision $F "Which database for the sync service?" \
  -c "Writes are append-heavy; reads are mostly by user id. Ops team already runs Postgres for two other services." \
  -o "Postgres" -o "SQLite per tenant" -o "FoundationDB"
q $T add-decision $F "Retry failed webhooks how many times?" -o 3 -o 5 -o "until 24h old"
q $T add-decision $F "Ship the beta behind a feature flag?" -o yes -o no
q $T add-todo $F "Create a Stripe test account and paste the key into .env" \
  -c "STRIPE_SECRET_KEY=sk_test_..., the agent can't sign up for you."
q $T add-todo $F "Approve the DNS change for api.example.dev"

# Done: resolved, withdrawn and finished, for selecting and deleting
q $T add-decision $F "Use glow or wgpu for rendering?" -o glow -o wgpu
q $T add-decision $F "Name of the CLI binary?" -o todai -o board
q $T add-decision $F "Keep the legacy REST endpoints?" -o yes -o "deprecate in v2"
q $T add-decision $F "Vendor the protobuf schemas?"
q $T add-todo $F "Install noto-fonts-emoji"
q $T add-todo $F "Rotate the staging database password"
$T resolve $F d4 glow
$T resolve $F d5 todai
$T resolve $F d6 "deprecate in v2"
$T withdraw $F d7
$T done $F t3 -n "installed, emoji render now"
$T done $F t4

# Elaborations, oldest first. An old answer on a resolved item: starts collapsed.
q $T ask $F d4 "glow" -n "what do we lose versus wgpu?"
q $T elaborate $F r1 - <<'MD'
Mostly **nothing we use**. wgpu brings Vulkan/Metal backends and compute
shaders; todai draws a few hundred rectangles and text runs per frame.

- glow: one GL context, ~40 MB less memory, faster startup
- wgpu: needed only for custom shaders or WebGPU builds

Recommendation: **glow**, revisit if we ever need shaders.
MD

# Still waiting for the agent.
q $T ask $F t1 "paste the key into .env" -n "can't you read it from 1Password?"

# The newest answer, on a pending decision: long, made for zooming.
q $T ask $F d1 "append-heavy" -n "compare the three properly, I need to defend this"
q $T elaborate $F r3 - <<'MD'
## Short version

**Postgres.** It fits the access pattern, the ops team already runs it, and
nothing about the workload needs the other two.

## Workload

- ~2k writes/s at peak, almost all inserts into an events table
- reads are `WHERE user_id = $1 ORDER BY ts DESC LIMIT 50`
- 90-day retention, then archive to object storage

## Options

| | Postgres | SQLite per tenant | FoundationDB |
|---|---|---|---|
| Fits append-heavy writes | yes, with partitioning | yes | yes |
| Ops cost for us | already run it | many files to back up | new system to learn |
| Cross-tenant queries | easy | painful | custom layer |
| Failover | streaming replicas | per-file, manual | built in |

### Postgres

Partition `events` by month and index `(user_id, ts DESC)`. Inserts stay
cheap, the hot partition stays in memory, and dropping old months is a
metadata operation instead of a big `DELETE`.

```sql
CREATE TABLE events (
  user_id bigint NOT NULL,
  ts      timestamptz NOT NULL,
  body    jsonb NOT NULL
) PARTITION BY RANGE (ts);
CREATE INDEX ON events (user_id, ts DESC);
```

### SQLite per tenant

Great isolation and trivially fast for a single tenant, but backups,
migrations and any cross-tenant report become fan-out jobs over thousands of
files. Worth it only if tenants must be physically separated.

### FoundationDB

Excellent at exactly this, with strict serializability and automatic
sharding. We would be the only team running it, and the record layer adds a
second abstraction to learn. The scale doesn't justify that yet.

## Risks with Postgres

1. Partition management must be automated (`pg_partman` or a cron job).
2. Very hot users could make one index page contended; shard by `user_id`
   later if that shows up in metrics.
3. JSONB bodies grow; compress or move large payloads to object storage.

> If you expect more than ~20k writes/s within a year, revisit FoundationDB.
MD

q $T ack $F
echo "Board: $F  (agent side: $T inbox $F / $T wait $F)"
[ "${1:-}" = "--no-open" ] || exec $T "$F"
