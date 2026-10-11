---
name: forge-plans
description: Use Forge's phased plans (the `forge` MCP server).
---

# Forge plans

Plans and the backlog live in Forge (not in the repo) and are read and changed over MCP with the `forge` server, which Forge connects to every agent it opens.

## Tools
- `plans_list`: plans in progress with their phases, branches and tasks, plus the backlog. Use it when starting work or when asked what's left.
- `plan_add`: creates a plan. With `phases` (each with `title`, optional `branch` and `tasks`) it's a phased plan; without phases, a backlog idea.
- `plan_update`: changes a plan. `phase` + `phase_status` (`in_progress`, `done`); `phase` + `task` + `task_done` to tick a task; `phase` + `add_task` to add one; `phases` to replace them all.
- `plan_delete`: deletes a plan or an idea.

## Working a phase
1. `plans_list` and read the whole phase.
2. Mark it `in_progress` and create its branch (`forge-branch-pr` skill).
3. Tick each task as soon as it's done, not all at the end.
4. If something unplanned comes up, add it as a task or to the backlog instead of forgetting it.
5. When opening the PR, mark the phase `done` only if all its tasks are done; if something depends on the person (a manual test, for example), leave it unticked and say so.

## When someone describes something they want built
Ask the few questions that matter, then create the plan with `plan_add`: small phases that each end in a PR, with their branch and tasks. Summarize the plan for the person in a few lines.
