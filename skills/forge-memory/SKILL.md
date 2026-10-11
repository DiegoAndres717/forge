---
name: forge-memory
description: Usar Forge Memory (la memoria del proyecto) por MCP.
---

# Forge Memory

La memoria del proyecto se comparte entre agentes por MCP (servidor `forge`). Va ligada al remoto de Git, no a la carpeta, y se guarda fuera del repo.

## Cuándo buscar (`memory_search`)
- Antes de tomar una decisión de diseño o de elegir una librería.
- Al encontrar un error: puede que ya se resolviera antes.
- Al tocar una parte del código que no conoces.
Busca con pocas palabras clave (sin tildes también vale). `memory_list` da las más recientes.

## Cuándo guardar (`memory_save`)
Guarda lo que le ahorraría tiempo a otro agente o a ti mañana, con un título claro y el porqué:
- `decision`: qué se decidió y por qué (y qué se descartó).
- `error`: el síntoma, la causa y la solución.
- `convention`: una regla del proyecto que no se ve en el código.
- `command`: un comando útil que no es obvio.
- `architecture`: cómo encajan las piezas.
No guardes lo que ya está en el código, en el historial de Git o en los planes.

## Sin duplicados
Si `memory_save` responde que ya hay una memoria parecida, no la fuerces: actualiza esa con `memory_update` (su id) para que la memoria no se llene de repetidas. Usa `force=true` solo si de verdad es otra cosa. Si una memoria ya no es cierta, bórrala con `memory_delete`.
