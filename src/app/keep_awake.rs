// Mantener el equipo despierto mientras un agente trabaja (plan #348): la app mira si algún
// agente de cualquier proyecto está «trabajando» (las mismas filas de la barra lateral, que
// salen de los hooks) y se lo dice a `forge_core::keep_awake`, que decide tomar o soltar.
use super::*;
use forge_core::keep_awake::{Inhibitor, KeepAwake};

/// El del sistema; en las pruebas, uno que no toca nada de verdad.
pub(super) fn new() -> KeepAwake<Box<dyn Inhibitor>> {
    #[cfg(test)]
    let inhibitor: Box<dyn Inhibitor> = Box::new(Pretend);
    #[cfg(not(test))]
    let inhibitor: Box<dyn Inhibitor> = Box::new(forge_core::keep_awake::System::default());
    KeepAwake::new(inhibitor)
}

#[cfg(test)]
struct Pretend;

#[cfg(test)]
impl Inhibitor for Pretend {
    fn hold(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn release(&mut self) {}
}

impl App {
    /// ¿Hay algún agente trabajando en algún proyecto despierto?
    /// Esperar permiso o una respuesta, terminado o quieto no cuentan (memoria #282: la
    /// salida de la terminal no es trabajo para los agentes con hooks; `live::rows` lo aplica).
    pub(super) fn agents_working(&mut self) -> bool {
        let now = forge_core::store::now();
        for ws in self.workspaces.iter_mut().filter(|w| !w.is_dormant()) {
            let infos: Vec<_> = ws
                .agent_terminals(now)
                .into_iter()
                .map(|(_, t)| t)
                .collect();
            if infos.is_empty() {
                continue;
            }
            let (rows, _) =
                self.agent_rows
                    .rows(self.store.as_ref(), &ws.project.path, &infos, now);
            if rows
                .iter()
                .any(|r| r.state == forge_core::live::State::Working)
            {
                return true;
            }
        }
        false
    }

    /// Toma o suelta según el ajuste y los agentes; vuelve a mirar cada 2 s, también con la
    /// ventana escondida.
    pub(super) fn keep_awake_tick(&mut self, ctx: &egui::Context) {
        if !self.keep_awake_on && !self.keep_awake.held() {
            return;
        }
        // `logic` corre antes de cada fotograma: mirar las filas de todos los proyectos basta
        // una vez por segundo (apagar el ajuste suelta sin esperar).
        if self.keep_awake_on
            && self
                .keep_awake_checked
                .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        self.keep_awake_checked = Some(Instant::now());
        let working = self.keep_awake_on && self.agents_working();
        self.keep_awake
            .update(self.keep_awake_on, working, Instant::now());
        ctx.request_repaint_after(Duration::from_secs(2));
    }
}
