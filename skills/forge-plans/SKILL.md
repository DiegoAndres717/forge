---
name: forge-plans
description: Usar los planes por fases de Forge (servidor MCP `forge`).
---

# Planes de Forge

Los planes y el backlog viven en Forge (no en el repo) y se leen y cambian por MCP con el servidor `forge`, que Forge conecta a todos los agentes que abre.

## Herramientas
- `plans_list`: planes en curso con sus fases, ramas y tareas, y el backlog. Úsala al empezar a trabajar o cuando pregunten qué falta.
- `plan_add`: crea un plan. Con `phases` (cada una con `title`, `branch` opcional y `tasks`) es un plan por fases; sin fases, una idea para el backlog.
- `plan_update`: cambia un plan. `phase` + `phase_status` (`in_progress`, `done`), `phase` + `task` + `task_done` para tachar, `phase` + `add_task` para añadir una tarea, `phases` para reemplazarlas todas.
- `plan_delete`: borra un plan o una idea.

## Cómo trabajar una fase
1. `plans_list` y lee la fase entera.
2. Márcala `in_progress` y crea su rama (skill `forge-branch-pr`).
3. Tacha cada tarea en cuanto la termines, no todas al final.
4. Si aparece algo que no estaba previsto, añádelo como tarea o al backlog en vez de olvidarlo.
5. Al abrir el PR, marca la fase `done` solo si todas sus tareas están hechas; si algo depende de la persona (una prueba manual, por ejemplo), déjalo sin tachar y dilo.

## Cuando te cuenten algo que quieren construir
Haz las preguntas que hagan falta (pocas), y luego crea el plan con `plan_add`: fases pequeñas que acaben cada una en un PR, con su rama y sus tareas. Cuéntale a la persona el plan en pocas líneas.
