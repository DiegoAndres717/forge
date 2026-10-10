// Notificaciones de macOS para los avisos de actividad, solo con Forge en segundo plano
// (dentro de la app basta el punto de la barra lateral). Una por aviso hasta que se entra
// al proyecto: un agente que espera y redibuja su pantalla no vuelve a notificar. Un clic
// en la notificación abre ese proyecto.
use super::*;
use crate::workspace::Attention;

impl App {
    pub(super) fn notify_tick(&mut self, ctx: &egui::Context) {
        self.notification_clicked(ctx);
        // Eventos de los agentes (hooks), cada medio segundo.
        if self
            .events_checked
            .is_none_or(|t| t.elapsed() > Duration::from_millis(500))
            && let Some(dir) = crate::terminal::SHELL_ENV
                .get()
                .and_then(|e| e.get("FORGE_EVENTS"))
        {
            self.events_checked = Some(Instant::now());
            for event in forge_core::events::drain(Path::new(dir)) {
                if let Some(ws) = self
                    .workspaces
                    .iter_mut()
                    .find(|w| w.project.path == event.project)
                {
                    if event.kind == "limit"
                        && let Some(panel) = event.panel
                    {
                        ws.limit_reached(panel);
                    }
                    ws.agent_event(event.text);
                }
            }
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if focused || !(self.notifications && self.notifications_enabled) {
            return;
        }
        for ws in &mut self.workspaces {
            let Some(attention) = ws.attention() else {
                continue;
            };
            if ws.notified.contains(&attention) {
                continue;
            }
            ws.notified.push(attention.clone());
            let text = match &attention {
                Attention::Agent(name) => tr!("{name} terminó o espera respuesta", name = name),
                Attention::Failed(name) => tr!("El proceso {name} falló", name = name),
                Attention::Blocked => tr!("Guard bloqueó la validación").to_string(),
                Attention::Event(text) => text.clone(),
            };
            let (project, path) = (ws.project.name(), ws.project.path.clone());
            let (clicked, ctx) = (self.notification_click.clone(), ctx.clone());
            std::thread::spawn(move || {
                let _ = mac_notification_sys::set_application("dev.forge.app");
                let response = mac_notification_sys::Notification::new()
                    .title("Forge")
                    .subtitle(&project)
                    .message(&text)
                    .sound("Glass")
                    .wait_for_click(true)
                    .send();
                if matches!(
                    response,
                    Ok(mac_notification_sys::NotificationResponse::Click)
                ) {
                    *clicked.lock().unwrap() = Some(path);
                    ctx.request_repaint();
                }
            });
        }
    }

    /// Clic en una notificación: Forge al frente y en ese proyecto.
    fn notification_clicked(&mut self, ctx: &egui::Context) {
        if let Some(path) = self
            .notification_click
            .lock()
            .ok()
            .and_then(|mut c| c.take())
            && let Some(i) = self.workspaces.iter().position(|w| w.project.path == path)
        {
            self.active = Some(i);
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }
}
