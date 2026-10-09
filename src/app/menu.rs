// Barra de menús de macOS (la de arriba, junto a la manzana): las acciones de Forge con sus
// atajos, como en cualquier app nativa. Los atajos los atiende el menú; los mismos siguen en
// `shortcut()` para los tests y por si el menú no está.
//
// ponytail: sin Copiar/Pegar en el menú: con ⌘C/⌘V como atajos de menú macOS mandaría
// `copy:`/`paste:` a la vista de winit, que no los atiende, y la terminal dejaría de copiar.
use super::*;
use muda::accelerator::Accelerator;
use muda::{AboutMetadata, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};

/// Lo que hace una opción del menú.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MenuCmd {
    Action(Action),
    CheckUpdates,
    Welcome,
}

pub(super) struct MenuBar {
    /// Hay que conservar el menú vivo mientras esté instalado.
    _menu: Menu,
    commands: HashMap<MenuId, MenuCmd>,
    events: Receiver<MenuEvent>,
}

impl MenuBar {
    /// Instala la barra de menús en el idioma actual (se vuelve a llamar al cambiarlo).
    pub(super) fn install(ctx: &egui::Context) -> Option<Self> {
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
        let act = MenuCmd::Action;
        let ws = |a: WsAction| MenuCmd::Action(Action::Ws(a));
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
                    act(Action::Settings),
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
        let file = Submenu::with_items(
            tr!("Archivo"),
            true,
            &[
                &item(
                    tr!("Abrir carpeta…"),
                    Some("CmdOrCtrl+O"),
                    act(Action::OpenFolder),
                ),
                &item(tr!("Inicio"), Some("CmdOrCtrl+Shift+H"), act(Action::Home)),
                &sep(),
                &item(
                    tr!("Cerrar proyecto"),
                    Some("CmdOrCtrl+Shift+W"),
                    act(Action::CloseProject),
                ),
            ],
        )
        .ok()?;
        let view = Submenu::with_items(
            tr!("Ver"),
            true,
            &[
                &item(
                    tr!("Paleta de comandos"),
                    Some("CmdOrCtrl+K"),
                    act(Action::Palette),
                ),
                &item(
                    tr!("Barra lateral"),
                    Some("CmdOrCtrl+B"),
                    act(Action::ToggleSidebar),
                ),
                &sep(),
                &item(
                    tr!("Aumentar la letra"),
                    Some("CmdOrCtrl+Equal"),
                    act(Action::FontBigger),
                ),
                &item(
                    tr!("Reducir la letra"),
                    Some("CmdOrCtrl+Minus"),
                    act(Action::FontSmaller),
                ),
                &item(
                    tr!("Tamaño normal"),
                    Some("CmdOrCtrl+Digit0"),
                    act(Action::FontReset),
                ),
                &sep(),
                &PredefinedMenuItem::fullscreen(Some(tr!("Pantalla completa"))),
            ],
        )
        .ok()?;
        let terminal = Submenu::with_items(
            tr!("Terminal"),
            true,
            &[
                &item(
                    tr!("Nueva terminal"),
                    Some("CmdOrCtrl+T"),
                    ws(WsAction::NewTerminal),
                ),
                &item(
                    tr!("Dividir a la derecha"),
                    Some("CmdOrCtrl+D"),
                    ws(WsAction::Split(Dir::Row)),
                ),
                &item(
                    tr!("Dividir abajo"),
                    Some("CmdOrCtrl+Shift+D"),
                    ws(WsAction::Split(Dir::Column)),
                ),
                &item(tr!("Girar la división"), None, ws(WsAction::Rotate)),
                &sep(),
                &item(
                    tr!("Terminal siguiente"),
                    Some("CmdOrCtrl+Shift+BracketRight"),
                    ws(WsAction::CycleTab(1)),
                ),
                &item(
                    tr!("Terminal anterior"),
                    Some("CmdOrCtrl+Shift+BracketLeft"),
                    ws(WsAction::CycleTab(-1)),
                ),
                &item(
                    tr!("Panel siguiente"),
                    Some("CmdOrCtrl+BracketRight"),
                    ws(WsAction::Cycle(1)),
                ),
                &item(
                    tr!("Panel anterior"),
                    Some("CmdOrCtrl+BracketLeft"),
                    ws(WsAction::Cycle(-1)),
                ),
                &item(
                    tr!("Maximizar o restaurar panel"),
                    Some("CmdOrCtrl+Enter"),
                    ws(WsAction::ToggleMaximize),
                ),
                &sep(),
                &item(
                    tr!("Buscar en la terminal"),
                    Some("CmdOrCtrl+F"),
                    ws(WsAction::Find),
                ),
                &sep(),
                &item(
                    tr!("Cerrar terminal"),
                    Some("CmdOrCtrl+W"),
                    ws(WsAction::Close),
                ),
            ],
        )
        .ok()?;
        let project = Submenu::with_items(
            tr!("Proyecto"),
            true,
            &[
                &item(
                    "Project Guard",
                    Some("CmdOrCtrl+G"),
                    act(Action::ToggleGuard),
                ),
                &item("Git", Some("CmdOrCtrl+Shift+G"), act(Action::ToggleGit)),
                &item(
                    tr!("Planes"),
                    Some("CmdOrCtrl+Shift+I"),
                    act(Action::ToggleIdeas),
                ),
                &item(
                    tr!("Memoria"),
                    Some("CmdOrCtrl+Shift+M"),
                    act(Action::ToggleMemory),
                ),
                &sep(),
                &item(
                    tr!("Abrir agente"),
                    Some("CmdOrCtrl+Shift+A"),
                    act(Action::OpenDefaultAgent),
                ),
            ],
        )
        .ok()?;
        let window = Submenu::with_items(
            tr!("Ventana"),
            true,
            &[
                &PredefinedMenuItem::minimize(Some(tr!("Minimizar"))),
                &PredefinedMenuItem::maximize(Some(tr!("Zoom"))),
                &sep(),
                &PredefinedMenuItem::bring_all_to_front(Some(tr!("Traer todo al frente"))),
            ],
        )
        .ok()?;
        let help = Submenu::with_items(
            tr!("Ayuda"),
            true,
            &[
                &item(tr!("Bienvenida a Forge"), None, MenuCmd::Welcome),
                &item(tr!("Todas las acciones (⌘K)"), None, act(Action::Palette)),
            ],
        )
        .ok()?;
        let menu =
            Menu::with_items(&[&forge, &file, &view, &terminal, &project, &window, &help]).ok()?;
        menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();
        help.set_as_help_menu_for_nsapp();
        Some(MenuBar {
            _menu: menu,
            commands,
            events,
        })
    }

    /// Opciones elegidas desde el último fotograma.
    pub(super) fn poll(&self) -> Vec<MenuCmd> {
        self.events
            .try_iter()
            .filter_map(|e| self.commands.get(e.id()).copied())
            .collect()
    }
}
