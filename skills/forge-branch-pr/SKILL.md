---
name: forge-branch-pr
description: Branches, commits, pull requests and review comments in Forge.
---

# Branches, commits and PRs

## Branch
- One branch per phase or change, from an up-to-date `main`: `git checkout main && git pull && git checkout -b type/short-description`.
- Types: `feat/`, `fix/`, `docs/`, `chore/`, `refactor/`, `perf/`, `test/`, `ci/`. Lowercase, hyphens. If the plan phase names a branch, use it.
- Never commit to `main`: it is protected and requires the CI `check`.

## Commits: one work unit each
- Conventional Commits in English: `feat(settings): …`, `fix(mod): …`, `docs: …`. Add `!` for a breaking change.
- A commit is one deliverable unit (a behavior, a fix, a migration), not a file type. Never "add models", then "add tests".
- Tests and docs go in the same commit as the behavior they cover.
- The repo must build and pass tests after each commit, and reverting it must not undo unrelated work.
- The message says the outcome and why, not the list of files.

## Before each commit
1. `cargo test --workspace` (includes the UI e2e tests, no GPU needed).
2. `cargo clippy --workspace --all-targets -- -D warnings`.
3. If you touched `claude-plugin/`: the checks in the `forge-mod` skill.
4. If you touched the UI: look at the screenshots (`forge-ui` skill).
5. **Last step, always**: `cargo fmt --all` and then `cargo fmt --all --check`. A late edit without `fmt` breaks CI.

## Size
- Aim for PRs under ~400 changed lines (additions + deletions), reviewable in under an hour. Generated files and screenshots don't count.
- If a phase will clearly exceed that, split it into chained PRs, each one a work unit that lands on its own, in order. Each PR says what it depends on and what comes next.

## Pull request
- Title in Conventional Commits, in English.
- Description in English, answer first:
  1. What changes for the user, in one or two lines. If it comes from a plan: "Plan #N, phase M".
  2. What to review first, and what is intentionally out of scope.
  3. What was tested: the commands you ran and their real result. Leave out what you didn't verify, or say it wasn't verified.
  4. Limits or pending items.
- Don't merge it yourself and don't enable auto-merge: the project owner merges.
- If `main` moved and there are conflicts: `git merge origin/main` on your branch, resolve keeping both sides, and run every check again.

## Review comments
- Lead with the actionable point; give the reason only if it isn't obvious; end with the concrete next step.
- One to three short paragraphs. Comment on what matters most, not every preference.
- Reply in the language of the thread.
