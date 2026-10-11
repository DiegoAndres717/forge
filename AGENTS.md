# Reglas de Forge

## Flujo de trabajo

- Una fase se trabaja en su propia rama y con su propio PR; nunca directamente en `main`.
- Los merges los hace la persona responsable del proyecto.
- Usa Conventional Commits.
- Al empezar una fase de un plan, márcala `in_progress`; tacha cada tarea al terminarla y marca la fase `done` al acabar.

## Calidad

- Antes de terminar o hacer commit, ejecuta `npx tsc --noEmit` y no uses `any` en TypeScript.
- Para cambios Rust, ejecuta `cargo test`, `cargo clippy` y, al final, `cargo fmt --check`.
- Todo texto visible debe pasar por `tr!` y tener su traducción en `crates/forge-core/src/i18n_en.rs`.
- El código se escribe en inglés y los comentarios en español.
- Si un cambio afecta la interfaz, revisa las capturas de la vista previa.
- Las funciones deben servir para todos los agentes compatibles, no solo para uno.

## Commits


