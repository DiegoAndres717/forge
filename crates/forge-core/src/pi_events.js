// forge-events: extensión de Pi que escribe Forge (se reescribe al abrir Pi desde Forge).
// Avisa a Forge cuando Pi termina y cuando espera tu respuesta o tu permiso, con
// `forge agent-event stop|waiting`. Fuera de una terminal de Forge no hace nada.
const FORGE = __FORGE__;

export default function (pi) {
  // Sin estas variables Pi no corre en una terminal de Forge: nada que avisar.
  if (!process.env.FORGE_EVENTS || !process.env.FORGE_PROJECT) return;
  let running = false;
  let summary;

  // Solo la sesión interactiva: los subagentes (Pi en modo rpc, json o print) heredan el
  // entorno y no deben avisar como si fueran la conversación principal.
  const interactive = (ctx) => ctx && ctx.mode === "tui";
  // Sin shell: la ruta y el JSON llegan como argumentos, sin comillas que escapar. Sin
  // esperar: un forge lento no retrasa a Pi, y un error no le llega.
  const send = (kind, message) => {
    const args = ["agent-event", kind];
    if (message) args.push(JSON.stringify({ message }));
    pi.exec(FORGE, args, { timeout: 10000 }).catch(() => {});
  };

  pi.on("agent_start", () => {
    running = true;
    summary = undefined;
  });

  // La primera línea de la última respuesta: el resumen del aviso, como en Claude Code.
  pi.on("agent_end", (event) => {
    const messages = (event && event.messages) || [];
    for (let i = messages.length - 1; i >= 0; i--) {
      const m = messages[i];
      if (!m || m.role !== "assistant" || !Array.isArray(m.content)) continue;
      const text = m.content
        .filter((c) => c && c.type === "text" && typeof c.text === "string")
        .map((c) => c.text)
        .join("\n");
      const line = text.split("\n").map((l) => l.trim()).find((l) => l.length > 0);
      if (line) {
        summary = line.length > 200 ? line.slice(0, 199) + "…" : line;
        break;
      }
    }
  });

  // Un diálogo que bloquea a Pi a mitad de trabajo: una confirmación (una extensión que pide
  // permiso antes de una herramienta) o una pregunta al usuario. Los de un comando que el
  // usuario abrió con Pi parado no avisan: ya está mirando.
  pi.on("ui_prompt_start", (event, ctx) => {
    if (!running || !interactive(ctx)) return;
    const title = event && event.title ? ": " + event.title : "";
    send("waiting", event && event.kind === "confirm" ? "Pi needs your permission" + title : undefined);
  });

  // Pi ya no seguirá solo (sin reintentos ni mensajes en cola). Con Esc lo paró el usuario.
  pi.on("agent_settled", (event, ctx) => {
    if (!running) return;
    running = false;
    if (!interactive(ctx) || (event && event.aborted)) return;
    send("stop", summary);
  });
}
