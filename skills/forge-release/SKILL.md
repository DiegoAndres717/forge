---
name: forge-release
description: Sacar una versión nueva de Forge.
---

# Versión nueva

Solo cuando la persona responsable lo pida.

1. Rama `chore/release-X.Y.Z` desde `main` actualizado.
2. Sube `version` en el `Cargo.toml` raíz y ejecuta `cargo build` para que se actualice `Cargo.lock`.
3. `cargo fmt --all --check`, commit `chore(release): vX.Y.Z` y PR con un resumen de lo que trae la versión.
4. Cuando el CI pase y se haga el merge: en `main` actualizado, `git tag vX.Y.Z && git push origin vX.Y.Z`.
5. El workflow `release.yml` comprueba que la etiqueta coincide con la versión, genera el DMG universal y su `.sha256`, publica la versión en GitHub y actualiza la receta de Homebrew en `DiegoAndres717/homebrew-tap` (con el secreto `HOMEBREW_TAP_TOKEN`).
6. Comprueba que la versión tiene `Forge-X.Y.Z.dmg` y `Forge-X.Y.Z.dmg.sha256` (`gh release view vX.Y.Z`).

Instalación para quien la use: `curl -fsSL https://raw.githubusercontent.com/DiegoAndres717/forge/main/scripts/install.sh | sh` o `brew install --cask diegoandres717/tap/forge`; la app también avisa de las actualizaciones.
