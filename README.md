# Forge

Workspace nativo para macOS (en Rust) para desarrollar con agentes de IA: cada proyecto
tiene sus terminales, procesos, agentes, memoria y un **Guard** que valida los cambios
antes de cada commit, push o pull request.

- **Terminales reales** con divisiones, foco por teclado y sesión que se restaura al abrir.
- **Procesos administrados** (`dev`, `api`…) con reinicio, puertos detectados y health checks.
- **Guard**: reglas deterministas (tamaño, archivos prohibidos, secretos, lint/tests/build),
  hooks de Git, excepciones con motivo, evidencias reutilizables y revisión opcional con IA.
- **Agentes** (Claude Code, Codex, OpenCode…) que se reanudan y comparten la memoria del
  proyecto vía MCP.
- **Comandos peligrosos** (`rm -rf` fuera del proyecto, `git reset --hard`, `git push --force`,
  `git clean -f`, `docker system prune`, `DROP DATABASE`) piden autorización en la ventana,
  también cuando los lanza un agente.

## Mod de Forge para Claude Code

Cualquier `claude` que se ejecute en una terminal de Forge (escrito a mano, `claude --resume`,
`claude -c`, o abierto desde la barra lateral) carga el mod de `claude-plugin/`: Forge pone
`CLAUDE_CODE_PLUGIN_DIRS` en sus terminales. No se instala nada en tu Claude ni se toca el repo;
fuera de Forge, Claude queda como siempre.

- **Subagentes en modelos más baratos**: `forge:explorer` (Haiku) para buscar y leer código,
  `forge:reviewer` (Sonnet) para tests y revisiones, `forge:architect` (Opus) para lo difícil;
  el principal delega en ellos y mantiene su contexto y su caché.
- **Consumo por modelo**: cada turno (subagentes incluidos) llega a Forge; se ve en Ajustes.
- **Banda** encima del cuadro de texto: estado de Guard, ideas pendientes y tokens de la sesión.
- **Comandos sin tokens**: `/ideas`, `/guard`, `/remember`.

Comprobar el mod: `claude plugin validate claude-plugin` y `claude plugin test claude-plugin`.

## Instalar

Requiere Rust estable (`brew install rustup`).

```sh
./scripts/install-app.sh          # compila e instala ~/Applications/Forge.app
```

CLI (opcional): `ln -sf ~/Applications/Forge.app/Contents/MacOS/forge /opt/homebrew/bin/forge`
y luego `forge help`.

## Estructura

```
crates/forge-core/   Lógica sin interfaz: Guard, hooks, evidencias, agentes, router de
                     modelos, memoria, servidor MCP y persistencia (SQLite).
src/                 La app: ventana (egui), terminales, layout, procesos y CLI.
  app/               Ventana principal: barra lateral, paneles de Guard y memoria, tests e2e.
tests/               Integración con el binario real: hooks de Git y servidor MCP.
scripts/             Empaquetado de Forge.app.
```

Configuración por proyecto en `.forge/` (`project.toml`, `rules.toml`, `agents.toml`,
`routing.toml`); la global en `~/.config/forge/config.toml`.

## Desarrollo

```sh
cargo test --workspace                                   # unitarios, integración y e2e de UI
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

- Los tests e2e de la interfaz (`src/app/e2e.rs`) manejan la app real con `egui_kittest`:
  atajos de teclado y clics sobre los controles por su nombre accesible.
- Capturas de la interfaz sin pantalla:
  `FORGE_PREVIEW_DIR=/tmp/forge cargo test --release ui_preview -- --ignored`.
- Guard sobre el propio repositorio: `cargo test --release dogfood -- --ignored --nocapture`.
