# Roadmap público de Forge

Este documento describe lo que ya está disponible, lo que se encuentra en desarrollo activo y lo planeado para futuras versiones de Forge.

Forge se construye en público. Si tienes ideas, comentarios o quieres solicitar una funcionalidad, [abre un issue](https://github.com/DiegoAndres717/forge/issues) o participa en las discusiones.

---

## 🟢 Disponible

### v0.10.0 (Última versión)
- **Comandos de cero tokens y prompts MCP**: `/forge-plans`, `/forge-memory`, `/forge-remember`, `/forge-guard` disponibles de forma nativa en Claude Code, OpenCode, Gemini CLI, Qwen Code y Codex.
- **Delegación de coste a subagentes**: Recomendación automática para delegar en modelos más económicos (`forge:explorer` en Haiku, `forge:reviewer` en Sonnet) la búsqueda de código, lectura y verificación de tests.
- **Gestión de múltiples cuentas**: Varias cuentas de Claude Code y Codex a la vez con seguimiento en vivo de tokens y límites.
- **Soporte multiplataforma (preview)**: Scripts de instalación nativos y soporte para macOS (universal), Linux y Windows.
- **Hooks de Project Guard**: Hooks deterministas de pre-commit y pre-push ejecutados en un worktree aislado de git.
- **Forge Memory**: Memoria de proyecto persistente entre sesiones y agentes compartida vía MCP, con deduplicación semántica.
- **Planes por fases y backlog**: Hojas de ruta estructuradas guardadas localmente fuera del repositorio de git.

---

## 🟡 En desarrollo (Corto plazo)

- **Paridad de plataforma y firma oficial**:
  - Firma con Apple Developer ID y notarización para macOS (eliminando el aviso inicial de Gatekeeper).
  - Firma Authenticode en Windows y distribución en winget.
  - Paquetes nativos para Linux (.deb, .rpm, AUR).
- **Visor visual de diffs en Project Guard**:
  - Visor interactivo integrado en el panel de Guard para revisar cambios antes de commitear.
  - Excepciones línea a línea y filtrado de archivos antes de abrir PRs o hacer commit.
- **Pestañas de espacio de trabajo y soporte multiventana**:
  - Arrastrar y soltar divisiones de terminales entre ventanas.
  - Gestión de espacios de trabajo en múltiples monitores.
- **Integración avanzada de subagentes**:
  - Orquestación de subagentes para Antigravity, Pi y ejecutores locales de LLMs.

---

## 🔮 Planificado (Medio y largo plazo)

- **Espacios de trabajo remotos y contenedores**:
  - Conectar espacios de Forge a servidores remotos vía SSH o contenedores Docker manteniendo la interfaz nativa por GPU.
- **Sincronización en equipo cifrada para Forge Memory**:
  - Sincronización opcional cifrada punto a punto o autoalojada para convenciones y memoria de proyecto entre miembros del equipo.
- **Reglas personalizadas de Guard y motor de extensiones**:
  - Plugins de Guard en WebAssembly o scripts para definir políticas de cumplimiento antes de cada commit.
- **Panel de telemetría y rendimiento en vivo**:
  - Métricas visuales de consumo de tokens por agente, tasa de acierto de caché y ahorro estimado en el tiempo.

---

## 🤝 Contribuir

¿Quieres contribuir? ¡Revisa nuestros issues etiquetados como [**good first issue**](https://github.com/DiegoAndres717/forge/labels/good%20first%20issue) para empezar!
