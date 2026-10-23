# todai: decisions and manual tasks for the operator

The operator (your user) answers your questions and does manual steps on a
todai board: a small window next to your session. Use it instead of asking in
chat whenever you need a decision or a hands-on action from them.

## Start

    todai attach [NAME]

Creates (or reuses) `.todai/NAME.jsonl` at the repo root (NAME defaults to
`board`), opens it in the operator's window, and prints its path. Use a
distinct NAME per independent workflow. Pass the printed path as BOARD below.

## Ask

    todai add-decision BOARD "<title>" [-c "<context>"] [-o "<option>"]...
    todai add-todo     BOARD "<title>" [-c "<context>"]

- Titles are the short form shown on the board: under 80 characters, one
  question or one action each. Put detail in `-c`.
- Offer `-o` options when the answer is a choice; the operator can still
  type anything.
- Todos are things only the operator can do (credentials, accounts, hardware,
  approvals). Say exactly what to do and what "done" looks like.
- Each command prints the new id (`d3`, `t2`). After posting, keep working on
  whatever doesn't depend on the answer.

## Receive

Operator updates arrive as a block starting with `todai: operator updates`.
Depending on your harness, they are injected into your context automatically,
or you fetch them:

    todai inbox BOARD     # print new updates and mark them seen
    todai wait BOARD      # block until there is an update, then print it

If nothing injects them for you, run `todai inbox BOARD` between steps of
your work. When you are blocked on answers, run `todai wait BOARD` in the
background if your harness supports background commands (you are woken when
it exits); otherwise run it in the foreground with `--timeout SECONDS`.

Updates mean:
- `dN ... resolved: TEXT`: the operator's decision. Treat it as binding.
- `dN ... reopened`: the earlier resolution no longer holds; wait for a new one.
- `tN ... done` (maybe with a note): the manual step is complete. Verify if you can.
- `tN ... marked NOT done again`: it is not complete after all.
- `rN: operator asks to elaborate on ...`: they did not understand part of an
  item. Answer it (below), then continue.

## Elaborate

    todai elaborate BOARD rN - <<'EOF'
    <markdown: explain the selected text plainly, give the tradeoff, recommend>
    EOF

Answer the question in the note if there is one. Keep it short.

## Also

    todai withdraw BOARD dN   # a decision is no longer needed
    todai show BOARD          # the whole board as text

Never edit the board file directly; always go through `todai`.
