// Notificaciones de macOS para los avisos de actividad cuando Forge no está a la vista
// (o el aviso es de otro proyecto). Un clic en la notificación abre ese proyecto.
use super::*;
use crate::workspace::Attention;

impl App {
    pub(super) fn notify_tick(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        for (i, ws) in self.workspaces.iter_mut().enumerate() {
            let attention = ws.attention();
            if attention == ws.notified {
                continue;
            }
            ws.notified = attention.clone();
            let Some(attention) = attention else { continue };
            if focused && self.active == Some(i) || !self.notifications {
                continue;
            }
            let text = match &attention {
                Attention::Agent(name) => tr!("{name} terminó o espera respuesta", name = name),
                Attention::Failed(name) => tr!("El proceso {name} falló", name = name),
                Attention::Blocked => tr!("Guard bloqueó la validación").to_string(),
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
        // Clic en una notificación: Forge al frente y en ese proyecto.
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
