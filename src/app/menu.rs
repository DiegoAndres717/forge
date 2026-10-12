// Menús de Forge: las acciones con sus atajos. En macOS van a la barra de menús nativa (la de
// arriba, junto a la manzana); en Linux y Windows, que no la tienen, salen del botón ☰ junto
// a la barra lateral. Los atajos siguen en `shortcut()` en todos los casos.
//
// ponytail: sin Copiar/Pegar en el menú: con ⌘C/⌘V como atajos de menú macOS mandaría
// `copy:`/`paste:` a la vista de winit, que no los atiende, y la terminal dejaría de copiar.
use super::*;

/// Lo que hace una opción del menú.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MenuCmd {
    Action(Action),
    CheckUpdates,
    Welcome,
}

/// Una opción: texto, atajo (formato de muda: `CmdOrCtrl+Shift+E`) y qué hace; None separa.
type Entry = Option<(String, Option<&'static str>, MenuCmd)>;

/// Los menús comunes, en orden (los propios de macOS los añade la barra nativa).
fn sections() -> Vec<(String, Vec<Entry>)> {
    let item = |text: &str, accel: Option<&'static str>, cmd: MenuCmd| {
        Some((text.to_string(), accel, cmd))
    };
    let act = MenuCmd::Action;
    let ws = |a: WsAction| MenuCmd::Action(Action::Ws(a));
    vec![
        (
            tr!("Archivo").to_string(),
            vec![
                item(
                    tr!("Abrir carpeta…"),
                    Some("CmdOrCtrl+O"),
                    act(Action::OpenFolder),
                ),
                item(
                    tr!("Abrir en el editor"),
                    Some("CmdOrCtrl+Shift+E"),
                    act(Action::OpenInEditor),
                ),
                item(tr!("Inicio"), Some("CmdOrCtrl+Shift+H"), act(Action::Home)),
                None,
                item(
                    tr!("Cerrar proyecto"),
                    Some("CmdOrCtrl+Shift+W"),
                    act(Action::CloseProject),
                ),
            ],
        ),
        (
            tr!("Ver").to_string(),
            vec![
                item(
                    tr!("Paleta de comandos"),
                    Some("CmdOrCtrl+K"),
                    act(Action::Palette),
                ),
                item(
                    tr!("Barra lateral"),
                    Some("CmdOrCtrl+B"),
                    act(Action::ToggleSidebar),
                ),
                None,
                item(
                    tr!("Aumentar la letra"),
                    Some("CmdOrCtrl+Equal"),
                    act(Action::FontBigger),
                ),
                item(
                    tr!("Reducir la letra"),
                    Some("CmdOrCtrl+Minus"),
                    act(Action::FontSmaller),
                ),
                item(
                    tr!("Tamaño normal"),
                    Some("CmdOrCtrl+Digit0"),
                    act(Action::FontReset),
                ),
            ],
        ),
        (
            tr!("Terminal").to_string(),
            vec![
                item(
                    tr!("Nueva terminal"),
                    Some("CmdOrCtrl+T"),
                    ws(WsAction::NewTerminal),
                ),
                item(
                    tr!("Dividir a la derecha"),
                    Some("CmdOrCtrl+D"),
                    ws(WsAction::Split(Dir::Row)),
                ),
                item(
                    tr!("Dividir abajo"),
                    Some("CmdOrCtrl+Shift+D"),
                    ws(WsAction::Split(Dir::Column)),
                ),
                item(tr!("Girar la división"), None, ws(WsAction::Rotate)),
                None,
                item(
                    tr!("Terminal siguiente"),
                    Some("CmdOrCtrl+Shift+BracketRight"),
                    ws(WsAction::CycleTab(1)),
                ),
                item(
                    tr!("Terminal anterior"),
                    Some("CmdOrCtrl+Shift+BracketLeft"),
                    ws(WsAction::CycleTab(-1)),
                ),
                item(
                    tr!("Panel siguiente"),
                    Some("CmdOrCtrl+BracketRight"),
                    ws(WsAction::Cycle(1)),
                ),
                item(
                    tr!("Panel anterior"),
                    Some("CmdOrCtrl+BracketLeft"),
                    ws(WsAction::Cycle(-1)),
                ),
                item(
                    tr!("Maximizar o restaurar panel"),
                    Some("CmdOrCtrl+Enter"),
                    ws(WsAction::ToggleMaximize),
                ),
                None,
                item(
                    tr!("Buscar en la terminal"),
                    Some("CmdOrCtrl+F"),
                    ws(WsAction::Find),
                ),
                None,
                item(
                    tr!("Cerrar terminal"),
                    Some("CmdOrCtrl+W"),
                    ws(WsAction::Close),
                ),
            ],
        ),
        (
            tr!("Proyecto").to_string(),
            vec![
                item(
                    "Project Guard",
                    Some("CmdOrCtrl+G"),
                    act(Action::ToggleGuard),
                ),
                item("Git", Some("CmdOrCtrl+Shift+G"), act(Action::ToggleGit)),
                item(
                    tr!("Planes"),
                    Some("CmdOrCtrl+Shift+I"),
                    act(Action::ToggleIdeas),
                ),
                item(
                    tr!("Memoria"),
                    Some("CmdOrCtrl+Shift+M"),
                    act(Action::ToggleMemory),
                ),
                None,
                item(
                    tr!("Abrir agente"),
                    Some("CmdOrCtrl+Shift+A"),
                    act(Action::OpenDefaultAgent),
                ),
            ],
        ),
        (
            tr!("Ayuda").to_string(),
            vec![
                item(tr!("Bienvenida a Forge"), None, MenuCmd::Welcome),
                item(tr!("Buscar actualizaciones…"), None, MenuCmd::CheckUpdates),
            ],
        ),
    ]
}

/// El atajo como se lee fuera de macOS: `CmdOrCtrl+Shift+BracketRight` → `Ctrl+Shift+]`.
fn accel_label(accel: &str) -> String {
    accel
        .split('+')
        .map(|key| match key {
            "CmdOrCtrl" => "Ctrl",
            "Equal" => "=",
            "Minus" => "-",
            "Comma" => ",",
            "BracketRight" => "]",
            "BracketLeft" => "[",
            other => other.strip_prefix("Digit").unwrap_or(other),
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Botón ☰ con los menús (fuera de macOS).
pub(super) fn menu_button(ui: &mut egui::Ui) -> Option<MenuCmd> {
    let response = theme::icon_toggle(ui, icon::LIST, tr!("Menú"), false);
    let mut chosen = None;
    egui::Popup::menu(&response).show(|ui| {
        let mut entry = |ui: &mut egui::Ui, (text, accel, cmd): (String, Option<&str>, MenuCmd)| {
            let button =
                egui::Button::new(text).shortcut_text(accel.map(accel_label).unwrap_or_default());
            if ui.add(button).clicked() {
                chosen = Some(cmd);
                ui.close();
            }
        };
        for (title, entries) in sections() {
            ui.menu_button(title, |ui| {
                for e in entries {
                    match e {
                        Some(item) => entry(ui, item),
                        None => {
                            ui.separator();
                        }
                    }
                }
            });
        }
        ui.separator();
        entry(
            ui,
            (
                tr!("Ajustes…").to_string(),
                Some("CmdOrCtrl+Comma"),
                MenuCmd::Action(Action::Settings),
            ),
        );
        if ui.button(tr!("Salir de Forge")).clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    });
    chosen
}

#[cfg(target_os = "macos")]
pub(super) use native::MenuBar;

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use muda::accelerator::Accelerator;
    use muda::{AboutMetadata, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};

    pub(in crate::app) struct MenuBar {
        /// Hay que conservar el menú vivo mientras esté instalado.
        _menu: Menu,
        commands: HashMap<MenuId, MenuCmd>,
        events: Receiver<MenuEvent>,
    }

    impl MenuBar {
        /// Instala la barra de menús en el idioma actual (se vuelve a llamar al cambiarlo).
        pub(in crate::app) fn install(ctx: &egui::Context) -> Option<Self> {
            // macOS solo deja crear menús en el hilo principal (no en tests ni capturas).
            objc2::MainThreadMarker::new()?;
            let (tx, events) = channel();
            let repaint = ctx.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let _ = tx.send(event);
                repaint.request_repaint();
            }));
            let mut commands = HashMap::new();
            let mut item = |text: &str, accel: Option<&str>, cmd: MenuCmd| {
                let accel = accel.and_then(|a| a.parse::<Accelerator>().ok());
                let item = MenuItem::new(text, true, accel);
                commands.insert(item.id().clone(), cmd);
                item
            };
            let sep = PredefinedMenuItem::separator;

            let about = AboutMetadata {
                name: Some("Forge".into()),
                version: Some(env!("CARGO_PKG_VERSION").into()),
                ..Default::default()
            };
            let forge = Submenu::with_items(
                "Forge",
                true,
                &[
                    &PredefinedMenuItem::about(Some(tr!("Acerca de Forge")), Some(about)),
                    &item(tr!("Buscar actualizaciones…"), None, MenuCmd::CheckUpdates),
                    &sep(),
                    &item(
                        tr!("Ajustes…"),
                        Some("CmdOrCtrl+Comma"),
                        MenuCmd::Action(Action::Settings),
                    ),
                    &sep(),
                    &PredefinedMenuItem::hide(Some(tr!("Ocultar Forge"))),
                    &PredefinedMenuItem::hide_others(Some(tr!("Ocultar otros"))),
                    &PredefinedMenuItem::show_all(Some(tr!("Mostrar todo"))),
                    &sep(),
                    &PredefinedMenuItem::quit(Some(tr!("Salir de Forge"))),
                ],
            )
            .ok()?;
            let menu = Menu::with_items(&[&forge]).ok()?;
            let mut help = None;
            for (title, entries) in sections() {
                let submenu = Submenu::new(&title, true);
                for e in entries {
                    let _ = match e {
                        // En macOS «Buscar actualizaciones…» ya está en el menú Forge.
                        Some((_, _, MenuCmd::CheckUpdates)) => continue,
                        Some((text, accel, cmd)) => submenu.append(&item(&text, accel, cmd)),
                        None => submenu.append(&sep()),
                    };
                }
                if title == tr!("Ver") {
                    let _ = submenu.append_items(&[
                        &sep(),
                        &PredefinedMenuItem::fullscreen(Some(tr!("Pantalla completa"))),
                    ]);
                }
                if title == tr!("Ayuda") {
                    let _ = submenu.append(&item(
                        tr!("Todas las acciones (⌘K)"),
                        None,
                        MenuCmd::Action(Action::Palette),
                    ));
                    // Ventana va antes de Ayuda, como en cualquier app de macOS.
                    let window = Submenu::with_items(
                        tr!("Ventana"),
                        true,
                        &[
                            &PredefinedMenuItem::minimize(Some(tr!("Minimizar"))),
                            &PredefinedMenuItem::maximize(Some(tr!("Zoom"))),
                            &sep(),
                            &PredefinedMenuItem::bring_all_to_front(Some(tr!(
                                "Traer todo al frente"
                            ))),
                        ],
                    )
                    .ok()?;
                    menu.append(&window).ok()?;
                    window.set_as_windows_menu_for_nsapp();
                    help = Some(submenu.clone());
                }
                menu.append(&submenu).ok()?;
            }
            menu.init_for_nsapp();
            if let Some(help) = help {
                help.set_as_help_menu_for_nsapp();
            }
            Some(MenuBar {
                _menu: menu,
                commands,
                events,
            })
        }

        /// Opciones elegidas desde el último fotograma.
        pub(in crate::app) fn poll(&self) -> Vec<MenuCmd> {
            self.events
                .try_iter()
                .filter_map(|e| self.commands.get(e.id()).copied())
                .collect()
        }
    }
}

/// Sin barra de menús nativa: sus opciones salen del botón ☰ (`menu_button`).
#[cfg(not(target_os = "macos"))]
pub(super) struct MenuBar;

#[cfg(not(target_os = "macos"))]
impl MenuBar {
    pub(super) fn install(_ctx: &egui::Context) -> Option<Self> {
        None
    }

    pub(super) fn poll(&self) -> Vec<MenuCmd> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shortcuts_read_naturally_outside_macos() {
        assert_eq!(
            super::accel_label("CmdOrCtrl+Shift+BracketRight"),
            "Ctrl+Shift+]"
        );
        assert_eq!(super::accel_label("CmdOrCtrl+Digit0"), "Ctrl+0");
        assert_eq!(super::accel_label("CmdOrCtrl+Comma"), "Ctrl+,");
    }
}
