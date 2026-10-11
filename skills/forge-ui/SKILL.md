---
name: forge-ui
description: Changes to Forge's app UI (egui).
---

# Forge UI

## Text and translations
- Every visible string goes through `tr!("texto en español")` (or `tr!("… {p0}", p0 = …)`).
- Every new string needs its translation in `crates/forge-core/src/i18n_en.rs`: `("texto en español", "English text"),`. Proper names (Forge Memory, Project Guard) are not translated.
- To find untranslated strings in the files you touched, extract the `tr!(` literals and compare them with the keys in `i18n_en.rs`.

## Make it understandable
- Every Settings option gets an ⓘ (`theme::help`) explaining what it does and when to use it.
- Forms save themselves (see `Autosave` in `src/app/guard_form.rs`) and show "Guardando…" / "✓ Guardado".
- No unexplained technical text: the file name can go in gray next to it, but what it does comes first.

## Fit any width
- No fixed widths that don't fit: `horizontal_wrapped`, label above the control when it doesn't fit (see `row` in `src/app/settings.rs`).
- Always check wide (1440 px) and narrow (560 px).

## Screenshots (required when the UI changes)
`FORGE_PREVIEW_DIR=/tmp/forge-shots cargo test --release --bin forge ui_preview -- --ignored`
Renders the scenes in `src/app/preview.rs` as PNGs. If you add a screen or a new state, add its scene. Look at the screenshots before calling the change good.

## Tests
- The e2e tests (`src/app/e2e.rs`) drive the real app with `egui_kittest`, finding controls by their accessible name. Add one for each new behavior.
- `get_by_*` fails on zero or several matches: use `query_*` to count or to check something is absent.
