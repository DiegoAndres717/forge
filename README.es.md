<div align="center">

<img src="assets/icon.png" width="128" alt="Icono de Forge">

# Forge

**Un espacio de trabajo nativo para macOS para programar con agentes de IA.**<br>
Terminales, procesos, agentes, **Forge Memory** y un Guard que revisa cada cambio antes de que salga de tu máquina: una ventana por proyecto.

[![CI](https://github.com/DiegoAndres717/forge/actions/workflows/ci.yml/badge.svg)](https://github.com/DiegoAndres717/forge/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/DiegoAndres717/forge?label=versión&cacheSeconds=300)](https://github.com/DiegoAndres717/forge/releases/latest)
![macOS 13+](https://img.shields.io/badge/macOS-13%2B-black?logo=apple)
![Apple Silicon + Intel](https://img.shields.io/badge/Apple%20Silicon%20%2B%20Intel-universal-black)
![Rust](https://img.shields.io/badge/hecho%20con-Rust-orange?logo=rust)
[![Licencia: MIT](https://img.shields.io/badge/licencia-MIT-blue)](LICENSE)

[Descargar](#instalar) · [Funciones](#funciones) · [Cómo funciona](#cómo-funciona) · [Mod de Claude Code](#mod-de-claude-code) · [Atajos](#atajos-de-teclado) · [Preguntas](#preguntas-frecuentes) · [English](README.md)

<img src="docs/demo.gif" width="900" alt="Demo de Forge">

</div>

---

## Por qué Forge

Cuando trabajas con Claude Code, Codex u OpenCode, el día se reparte entre pestañas de terminal, servidores de desarrollo, notas y "¿pasaron los tests antes del push?". Forge reúne cada proyecto en un solo lugar:

- **Abres una carpeta y recuperas todo**: terminales, divisiones, servidores y agentes tal como los dejaste.
- **Agentes que planifican y recuerdan**: los planes por fases, el backlog y Forge Memory (la memoria del proyecto) se comparten con todos los agentes vía MCP.
- **Nada sale roto**: Project Guard aplica tus reglas (tamaño, secretos, lint, tests, build) antes de cada commit, push y pull request.
- **Nativo y ligero**: escrito en Rust con interfaz dibujada en GPU; sin Electron ni navegador embebido.

## Funciones

<table>
<tr>
<td width="50%"><img src="docs/screenshots/workspace.png" alt="Terminales"></td>
<td width="50%">

### Terminales y procesos
Terminales reales **al estilo VS Code**: varias en un mismo panel (`⌘T`), cambias desde la lista lateral, las reordenas arrastrando y ves cuáles escribieron algo mientras estaban ocultas. Divisiones, búsqueda (`⌘F`), sugerencias de carpetas al escribir e historial que sobrevive a los reinicios. Procesos administrados (`dev`, `api`…) con reinicio, puertos detectados y health checks. Un clic en un agente te lleva a su terminal abierta o reanuda su última sesión.

</td>
</tr>
<tr>
<td>

### Project Guard
Reglas deterministas antes de **commit / push / PR**: tamaño del cambio, archivos prohibidos, detección de secretos y tus propias comprobaciones (`npm test`, `cargo clippy`…), en una copia aislada, más una revisión con IA opcional según el riesgo. Las excepciones piden motivo y quedan registradas. Crea el commit o abre el pull request desde el panel.

</td>
<td><img src="docs/screenshots/guard.png" alt="Project Guard"></td>
</tr>
<tr>
<td><img src="docs/screenshots/plans.png" alt="Planes"></td>
<td>

### Planes y Forge Memory
Cuéntale a la IA lo que quieres hacer y guarda un **plan por fases**, cada una con su rama y tareas que va tachando; lo que surja para después va al **backlog**. **Forge Memory** guarda las decisiones, los errores resueltos y las convenciones. Las dos se comparten por MCP con Claude Code, Codex y OpenCode, y se guardan fuera del repo.

</td>
</tr>
<tr>
<td>

### Ajustes sin editar archivos
Pestañas, un ⓘ en cada opción y formularios para Guard, agentes, revisión con IA y procesos que se guardan solos (conservando los comentarios de tus archivos de `.forge/`). Varias **cuentas de Claude Code y Codex** a la vez, cada una con su uso del plan.

</td>
<td><img src="docs/screenshots/settings.png" alt="Ajustes"></td>
</tr>
</table>

Además: paleta `⌘K` para todo · confirmación antes de comandos peligrosos (`rm -rf` fuera del proyecto, `git push --force`, `DROP DATABASE`…), también si los lanza un agente · panel de Git · barra de menús de macOS con todas las acciones · avisos cuando un agente termina o te necesita · actualizaciones automáticas · inglés y español.

## Cómo funciona

### Forge Memory
Todos los agentes abiertos en Forge leen y escriben la misma memoria del proyecto por MCP. Va ligada al remoto de Git, no guarda duplicados, y Claude Code recibe las decisiones clave y los planes en curso en el primer mensaje de cada conversación.

<img src="docs/diagrams/forge-memory.es.png" alt="Cómo funciona Forge Memory">

### Project Guard
Lo que pasa entre `git commit` (o el push, o el pull request) y que tu cambio salga de la máquina.

<img src="docs/diagrams/project-guard.es.png" alt="Cómo funciona Project Guard">

## Instalar

> **Requisitos:** macOS 13 Ventura o superior, en Apple Silicon o Intel (app universal).

### Con un comando (recomendado)

```sh
curl -fsSL https://raw.githubusercontent.com/DiegoAndres717/forge/main/scripts/install.sh | sh
```

Descarga la última versión, comprueba su SHA-256, instala `Forge.app` en `/Applications` (o `~/Applications`) y enlaza el comando `forge`. Vuelve a ejecutarlo para actualizar. Instalado así, macOS no muestra el aviso de "no se puede abrir".

### Descarga

1. Descarga `Forge-x.y.z.dmg` de la [última versión](https://github.com/DiegoAndres717/forge/releases/latest).
2. Ábrelo y arrastra **Forge** a **Aplicaciones**.
3. La primera vez, **clic derecho en Forge → Abrir → Abrir**. Forge aún no está notarizado por Apple, así que con doble clic macOS dice que "no se puede abrir". Si aun así no abre, ejecuta:

   ```sh
   xattr -dr com.apple.quarantine /Applications/Forge.app
   ```

### Homebrew

```sh
brew install --cask diegoandres717/tap/forge
```

Para actualizar: `brew upgrade --cask forge`.

### Compilar desde el código

```sh
brew install rustup && rustup-init -y      # Rust estable
git clone https://github.com/DiegoAndres717/forge.git && cd forge
./scripts/install-app.sh                   # compila e instala ~/Applications/Forge.app
# FORGE_UNIVERSAL=1 ./scripts/install-app.sh   → binario universal (Apple Silicon + Intel)
```

### Línea de comandos (opcional)

```sh
ln -sf /Applications/Forge.app/Contents/MacOS/forge /opt/homebrew/bin/forge
forge help
```

El idioma se cambia en Ajustes (`⌘,`).

## Primeros pasos

1. Abre Forge y elige una carpeta (o arrástrala a la ventana, o ejecuta `forge .`).
2. Funciona sin configurar nada. Para guardar tu layout y tus procesos, usa **Ajustes → Crear .forge/project.toml** en la barra lateral: Forge lo rellena con los scripts que encuentre en `package.json`.
3. Pulsa **Claude Code**, **Codex** u **OpenCode** en la barra lateral para abrir un agente en el proyecto. Si vuelves a pulsarlo, te lleva a su terminal abierta o reanuda tu última sesión (la fila dice cuándo fue y de qué iba); **+** abre una nueva.
4. Pulsa `⌘G` antes de hacer commit para ver si el cambio está listo.

## Mod de Claude Code

Cualquier `claude` que se ejecute en una terminal de Forge (escrito a mano, `claude --resume`, `claude -c` o abierto desde la barra lateral) carga el mod de Forge automáticamente. No se instala nada en tu configuración de Claude ni se añade nada al repo; fuera de Forge, Claude funciona como siempre.

- **Subagentes más baratos**: `forge:explorer` (Haiku) busca y lee código, `forge:reviewer` (Sonnet) corre tests y revisa, `forge:architect` (Opus) resuelve lo difícil. El modelo principal delega y conserva su contexto y su caché.
- **Banda en vivo** encima del cuadro de texto: estado de Guard, planes en progreso y backlog, el modelo que trabaja en ese momento (● opus / ↳ explorer: … (haiku)), tokens de la sesión (nuevos frente a caché) y barras de uso del plan (5 h y semanal).
- **Consumo por modelo**: cada turno, subagentes incluidos, queda registrado; se ve en Ajustes.
- **Empieza sabiendo del proyecto**: cada conversación (y tras compactar o `/clear`) arranca con las decisiones clave, convenciones, reglas de Guard y planes en curso.
- **Varias cuentas**: cuentas de Claude Code y Codex a la vez sin cerrar sesión (Ajustes → Cuentas). Cada una tiene su login e historial y comparte tus ajustes, `CLAUDE.md` y skills; eliges cuál usar en cada proyecto desde el clic derecho del agente. Puedes pasar una conversación a otra cuenta y seguir donde ibas; Forge te lo ofrece también cuando una cuenta llega a su límite.
- **Comandos sin tokens**: `/forge-plans`, `/forge-guard`, `/forge-remember` (escribe `/forge` para verlos).

## Atajos de teclado

| Atajo | Acción | Atajo | Acción |
|---|---|---|---|
| `⌘K` | Paleta de comandos | `⌘G` | Project Guard |
| `⌘O` | Abrir carpeta | `⌘⇧M` / `⌘⇧I` | Memoria / Planes |
| `⌘1`…`⌘9` | Cambiar de proyecto | `⌘⇧A` | Abrir agente |
| `⌃Tab` | Proyecto reciente | `⌘F` | Buscar en la terminal |
| `⌘T` | Nueva terminal (en el panel) | `⌘B` | Mostrar/ocultar barra lateral |
| `⌘⇧]` `⌘⇧[` | Terminal siguiente / anterior | `⌘]` `⌘[` | Panel siguiente / anterior |
| `⌘D` / `⌘⇧D` | Dividir a la derecha / abajo | `⌘⇧H` | Inicio |
| `⌘⌥←` `⌘⌥→` | Mover el foco | `⌘,` | Ajustes |

## Configuración

Por proyecto, en `.forge/` (súbelo al repo para compartirlo con tu equipo):

| Archivo | Qué contiene |
|---|---|
| `project.toml` | Layout (terminales y divisiones), procesos y comandos |
| `rules.toml` | Reglas y comprobaciones de Guard por etapa |
| `agents.toml` | Agentes y cómo se lanzan |
| `routing.toml` | Enrutado de modelos para revisiones con IA (`forge ai init`) |

La configuración global está en `~/.config/forge/config.toml` (fuentes y más). Planes, memoria e historial se guardan en la base de datos de Forge, nunca en el repo.

La referencia completa de la CLI está en el [README en inglés](README.md#configuration) y en `forge help`.

## Preguntas frecuentes

**¿Forge envía mi código a algún sitio?**
No. Forge funciona en local; su única llamada de red es la comprobación de actualizaciones en GitHub Releases (más la revisión con IA opcional, si la activas en `routing.toml`). Los agentes que lances hablan con sus proveedores como siempre.

**¿Macs Intel? ¿Windows?**
Macs Intel: sí, la app es universal. Windows y Linux: por ahora no.

**¿Necesito Claude Code?**
No. Terminales, procesos, Guard, Git y planes funcionan sin agentes. Forge detecta Claude Code, Codex, OpenCode, Gemini CLI, Qwen Code y Pi si están instalados.

**¿Cómo reporto un error?**
Abre un [issue](https://github.com/DiegoAndres717/forge/issues) con lo que hiciste, lo que esperabas y, si puedes, una captura.

## Licencia

[MIT](LICENSE) © Diego Andres Salas
