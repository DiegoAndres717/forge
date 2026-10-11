# Forge — instrucciones para agentes

Forge es un workspace nativo de macOS (Rust + egui) para programar con agentes de IA. Estas reglas valen para cualquier agente (Claude Code, Codex, OpenCode, Gemini CLI, Qwen Code…).

## Reglas básicas

- Nunca trabajes en `main`: cada fase o cambio va en su rama y su PR. Los merges los hace la persona responsable del proyecto.
- Código e identificadores en inglés; comentarios y textos de la interfaz en español (con traducción al inglés).
- Todo lo que Forge ofrezca a los agentes debe servir para todos los agentes compatibles, no solo para uno.
- Sin navegador embebido ni botones que abran URLs: puertos y direcciones se muestran como texto.
- Antes de dar algo por terminado: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` y, como último paso, `cargo fmt --all --check`.

## Skills

Antes de empezar una tarea, mira la columna «Cuándo» y lee la skill que toque (puede ser más de una). Sigue lo que diga.

| Skill | Cuándo | Archivo |
|---|---|---|
| `forge-branch-pr` | Al crear una rama, un commit o un pull request | [`skills/forge-branch-pr/SKILL.md`](skills/forge-branch-pr/SKILL.md) |
| `forge-plans` | Al empezar o seguir una fase, o cuando te cuenten algo que quieren construir | [`skills/forge-plans/SKILL.md`](skills/forge-plans/SKILL.md) |
| `forge-memory` | Al tomar una decisión, resolver un error, encontrar una convención o empezar a depurar | [`skills/forge-memory/SKILL.md`](skills/forge-memory/SKILL.md) |
| `forge-ui` | Al tocar la interfaz de la app (`src/app/`, `src/theme.rs`, textos visibles) | [`skills/forge-ui/SKILL.md`](skills/forge-ui/SKILL.md) |
| `forge-mod` | Al tocar el mod de Claude Code (`claude-plugin/`) | [`skills/forge-mod/SKILL.md`](skills/forge-mod/SKILL.md) |
| `forge-release` | Al sacar una versión nueva | [`skills/forge-release/SKILL.md`](skills/forge-release/SKILL.md) |
| `forge-docs` | Al cambiar el README, las capturas, el GIF o los diagramas | [`skills/forge-docs/SKILL.md`](skills/forge-docs/SKILL.md) |

## Mapa rápido

- `crates/forge-core/`: lógica sin interfaz (Guard, memoria, planes, cuentas, MCP, SQLite, traducciones).
- `src/`: la app (ventana egui, terminales, layout, procesos) y la CLI (`src/cli.rs`).
- `claude-plugin/`: el mod de Claude Code (hooks en TypeScript, se incrusta en el binario).
- `tests/`: integración contra el binario real (hooks de Git y servidor MCP).
- `scripts/`: instalar la app, generar el DMG, el instalador `curl | sh` y la receta de Homebrew.
