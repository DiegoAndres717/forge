---
name: forge-memory
description: Use Forge Memory (the project memory) over MCP.
---

# Forge Memory

The project memory is shared between agents over MCP (`forge` server). It's tied to the Git remote, not the folder, and stored outside the repo.

## When to search (`memory_search`)
- Before a design decision or choosing a library.
- When you hit an error: it may have been solved already.
- When touching a part of the code you don't know.
Search with a few keywords (accents optional). `memory_list` returns the most recent ones; `memory_get` gives the full text of one.

## When to save (`memory_save`)
Save what would save another agent (or you, tomorrow) time, with a clear title and the why:
- `decision`: what was decided and why (and what was discarded).
- `error`: the symptom, the cause and the fix.
- `convention`: a project rule you can't see in the code.
- `command`: a useful, non-obvious command.
- `architecture`: how the pieces fit together.
Don't save what's already in the code, the Git history or the plans.
Exception: when you fix a bug that a review or the user caught, save it as `error` if the same mistake could show up elsewhere (a pattern, not a one-off typo), even though the fix is in Git: the commit doesn't warn the next agent.

## No duplicates
If `memory_save` answers that a similar memory exists, don't force it: update that one with `memory_update` (its id) so memory doesn't fill up with repeats. Use `force=true` only if it really is something else. If a memory is no longer true, delete it with `memory_delete`.
