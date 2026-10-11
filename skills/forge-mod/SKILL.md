---
name: forge-mod
description: Forge's Claude Code mod (`claude-plugin/`).
---

# Claude Code mod

It lives in `claude-plugin/` and is embedded in the Forge binary; any `claude` opened in a Forge terminal loads it. Outside Forge it does nothing (it checks `FORGE_BIN` / `FORGE_PROJECT`).

## Pieces
- `hooks/register.tsx`: the hooks (`session.start`, `prompt.context`, `turn.step`, `agent.spawn`, `ui.render` for the band, `command.run`…).
- `hooks/forge.test.ts`: the tests.
- `.claude-plugin/plugin.json`: declares the `forge` MCP server.
- To talk to Forge use the `forge($, [args])` helper: it never throws and returns `null` outside Forge.

## Rules
- Anything that can take time (git, the Forge CLI) runs in the background (`void …`): never delay startup or a turn.
- The band is only drawn when there's something to show; use theme colors with meaning (`error`, `warning`, `success`) only for states.
- The mod is only for Claude Code: whatever must serve every agent goes in the MCP server or the app.

## Checks
1. `npx -p typescript@5.9.3 tsc --noEmit -p claude-plugin/tsconfig.json` (no `any`).
2. `claude plugin validate claude-plugin`.
3. `claude plugin test claude-plugin`.
Tests answer through the engine with `on(...)`: look at the existing ones (for example `session.measure` so the band has something to draw, `{ value: … }` in `process.run`).
