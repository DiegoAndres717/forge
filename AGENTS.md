# Reglas de Forge

## Flujo de trabajo

- Una fase se trabaja en su propia rama y con su propio PR; nunca directamente en `main`.
- Los merges los hace la persona responsable del proyecto.
- Usa Conventional Commits.
- Al empezar una fase de un plan, márcala `in_progress`; tacha cada tarea al terminarla y marca la fase `done` al acabar.

## Calidad

- Para cambios Rust, ejecuta `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` y, como último paso, `cargo fmt --all --check` (el CI falla si no).
- Para cambios en el mod de Claude Code (`claude-plugin/`): `npx -p typescript@5.9.3 tsc --noEmit -p claude-plugin/tsconfig.json`, `claude plugin validate claude-plugin` y `claude plugin test claude-plugin`. No uses `any` en TypeScript.
- Todo texto visible debe pasar por `tr!` y tener su traducción en `crates/forge-core/src/i18n_en.rs`.
- El código se escribe en inglés y los comentarios en español.
- Si un cambio afecta la interfaz, revisa las capturas de la vista previa.
- Las funciones deben servir para todos los agentes compatibles, no solo para uno.
- Sin navegador embebido ni botones que abran URLs: puertos y direcciones se muestran como texto.

