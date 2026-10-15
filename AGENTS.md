# Forge — instructions for agents

Forge is a native macOS workspace (Rust + egui) for coding with AI agents. These rules apply to every agent (Claude Code, Codex, OpenCode, Gemini CLI, Qwen Code…).

## Ground rules

- Never work on `main`: every phase or change gets its own branch and PR. The project owner does the merges.
- Code and identifiers in English. Code comments in Spanish. UI text in Spanish through `tr!` with its English translation.
- Commits, PR titles and descriptions, and anything written for agents (skills, instructions, MCP prompts) in English.
- Plans and backlog ideas (Forge plans) in the language of the owner's Forge interface: the owner reads them there.
- Whatever Forge offers to agents must work for every supported agent, not just one.
- No embedded browser and no buttons that open URLs: ports and addresses are shown as text.
- Before calling something done: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and, as the very last step, `cargo fmt --all --check`.

## Skills

Before starting a task, check the "When" column and read the matching skill (it can be more than one). Follow it.

| Skill | When | File |
|---|---|---|
| `forge-branch-pr` | Creating a branch, a commit, a pull request or a review comment | [`skills/forge-branch-pr/SKILL.md`](skills/forge-branch-pr/SKILL.md) |
| `forge-debug` | Fixing a bug, a failing test or a user report | [`skills/forge-debug/SKILL.md`](skills/forge-debug/SKILL.md) |
| `forge-plans` | Starting or continuing a phase, or when someone describes something they want built | [`skills/forge-plans/SKILL.md`](skills/forge-plans/SKILL.md) |
| `forge-memory` | Making a decision, solving an error, finding a convention or starting to debug | [`skills/forge-memory/SKILL.md`](skills/forge-memory/SKILL.md) |
| `forge-ui` | Touching the app UI (`src/app/`, `src/theme.rs`, visible text) | [`skills/forge-ui/SKILL.md`](skills/forge-ui/SKILL.md) |
| `forge-mod` | Touching the Claude Code mod (`claude-plugin/`) | [`skills/forge-mod/SKILL.md`](skills/forge-mod/SKILL.md) |
| `forge-release` | Shipping a new version | [`skills/forge-release/SKILL.md`](skills/forge-release/SKILL.md) |
| `forge-docs` | Changing the READMEs, screenshots, GIF or diagrams | [`skills/forge-docs/SKILL.md`](skills/forge-docs/SKILL.md) |

## Quick map

- `crates/forge-core/`: logic without UI (Guard, memory, plans, accounts, MCP, SQLite, translations).
- `src/`: the app (egui window, terminals, layout, processes) and the CLI (`src/cli.rs`).
- `claude-plugin/`: the Claude Code mod (TypeScript hooks, embedded in the binary).
- `tests/`: integration tests against the real binary (Git hooks, MCP server).
- `scripts/`: app install, DMG build, the `curl | sh` installer and the Homebrew recipe.
