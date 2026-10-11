---
name: forge-branch-pr
description: Ramas, commits y pull requests en Forge.
---

# Ramas, commits y PR

## Rama
- Una rama por fase o por cambio, desde `main` actualizado: `git checkout main && git pull && git checkout -b tipo/descripcion`.
- Tipos: `feat/`, `fix/`, `docs/`, `chore/`, `refactor/`, `test/`. Si la fase del plan trae rama, usa esa.
- Nunca hagas commit en `main`: está protegida y exige el check `check` del CI.

## Commits
- Conventional Commits en inglés: `feat(settings): …`, `fix(mod): …`, `docs: …`.
- Primera línea corta; debajo, una lista breve de lo que cambia y por qué.

## Antes de cada commit
1. `cargo test --workspace` (incluye los e2e de la interfaz, sin GPU).
2. `cargo clippy --workspace --all-targets -- -D warnings`.
3. Si tocaste `claude-plugin/`: las comprobaciones de la skill `forge-mod`.
4. Si tocaste la interfaz: revisa las capturas (skill `forge-ui`).
5. **Último paso, siempre**: `cargo fmt --all` y `cargo fmt --all --check`. Un cambio de última hora sin `fmt` rompe el CI.

## Pull request
- `gh pr create` con un título en Conventional Commits y una descripción en español que diga: qué cambia, cómo se usa, qué se probó y lo que quede pendiente o tenga límites.
- Si viene de un plan: «Plan #N · fase M».
- No hagas merge tú: lo hace la persona responsable del proyecto. No actives la fusión automática.
- Si `main` avanzó y hay conflictos: `git merge origin/main` en tu rama, resuélvelos conservando ambos lados y vuelve a pasar todas las comprobaciones.
