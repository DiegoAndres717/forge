---
name: forge-docs
description: Forge's READMEs, screenshots, GIF and diagrams.
---

# Documentation

## Files
- Two READMEs: `README.md` (English) and `README.es.md` (Spanish). Every change goes in both.
- Few, current screenshots: `docs/screenshots/` come from the UI preview (`forge-ui` skill), 1440 px wide.
- The GIF (`docs/demo.gif`) is built from several preview scenes at 1000 px, ~2 s per scene, 128 colors (Pillow).
- The "How it works" diagrams are made with [Archify](https://github.com/tt-a1i/archify) from the JSON in `docs/diagrams/src/` (English and Spanish); see `docs/diagrams/README.md`. If how Forge Memory or Project Guard works changes, update the JSON and regenerate the PNGs.

## Writing so it's understood fast
- Write for someone who doesn't know Forge: what it does and why first, details after.
- Lead with the answer or the action; context comes after.
- Quick path first (the usual steps and what you should see), then details and edge cases.
- Short sections with clear headings. Prefer tables, checklists and examples over long paragraphs.
- Proper names stay untranslated (Forge Memory, Project Guard).
