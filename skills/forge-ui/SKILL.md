---
name: forge-ui
description: Cambios en la interfaz de la app (egui) de Forge.
---

# Interfaz de Forge

## Textos y traducciones
- Todo texto visible pasa por `tr!("texto en español")` (o `tr!("… {p0}", p0 = …)`).
- Cada texto nuevo necesita su traducción en `crates/forge-core/src/i18n_en.rs`: `("texto en español", "English text"),`. Los nombres propios (Forge Memory, Project Guard) no se traducen.
- Para encontrar textos sin traducir en los archivos que tocaste, extrae los literales de `tr!(` y compáralos con las claves de `i18n_en.rs`.

## Que se entienda
- Cada opción de Ajustes lleva un ⓘ (`theme::help`) que explique qué hace y cuándo usarla.
- Los formularios se guardan solos (ver `Autosave` en `src/app/guard_form.rs`) y muestran «Guardando…» / «✓ Guardado».
- Nada de texto técnico sin explicar: el nombre del archivo puede ir en gris al lado, pero primero lo que hace.

## Que se adapte al ancho
- Nada de anchos fijos que no quepan: `horizontal_wrapped`, etiqueta encima del control cuando no cabe (ver `row` en `src/app/settings.rs`).
- Revisa siempre ancho (1440 px) y estrecho (560 px).

## Capturas (obligatorio si cambia la interfaz)
`FORGE_PREVIEW_DIR=/tmp/forge-shots cargo test --release --bin forge ui_preview -- --ignored`
Genera las escenas de `src/app/preview.rs` como PNG. Si añades una pantalla o un estado nuevo, añade su escena. Mira las capturas antes de dar el cambio por bueno.

## Tests
- Los e2e (`src/app/e2e.rs`) manejan la app real con `egui_kittest`, buscando controles por su nombre accesible. Añade uno por cada comportamiento nuevo.
- `get_by_*` falla si hay cero o varias coincidencias: usa `query_*` para contar o comprobar que no existe.
