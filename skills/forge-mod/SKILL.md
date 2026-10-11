---
name: forge-mod
description: El mod de Claude Code de Forge (`claude-plugin/`).
---

# Mod de Claude Code

Vive en `claude-plugin/` y se incrusta en el binario de Forge; cualquier `claude` abierto en una terminal de Forge lo carga. Fuera de Forge no hace nada (comprueba `FORGE_BIN` / `FORGE_PROJECT`).

## Piezas
- `hooks/register.tsx`: los hooks (`session.start`, `prompt.context`, `turn.step`, `agent.spawn`, `ui.render` para la banda, `command.run`…).
- `hooks/forge.test.ts`: los tests.
- `.claude-plugin/plugin.json`: declara el servidor MCP `forge`.
- Para hablar con Forge usa el ayudante `forge($, [args])`: nunca lanza errores y devuelve `null` fuera de Forge.

## Reglas
- Lo que pueda tardar (git, la CLI de Forge) va en segundo plano (`void …`): nunca retrases el arranque ni un turno.
- La banda solo se dibuja cuando hay algo que mostrar; usa colores de tema con su significado (`error`, `warning`, `success`) solo para estados.
- Recuerda que el mod es solo para Claude Code: lo que tenga que servir a todos los agentes va en el servidor MCP o en la app.

## Comprobaciones
1. `npx -p typescript@5.9.3 tsc --noEmit -p claude-plugin/tsconfig.json` (sin `any`).
2. `claude plugin validate claude-plugin`.
3. `claude plugin test claude-plugin`.
Los tests responden por el motor con `on(...)`: mira los existentes (por ejemplo, `session.measure` para que la banda tenga algo que dibujar, `{ value: … }` en `process.run`).
