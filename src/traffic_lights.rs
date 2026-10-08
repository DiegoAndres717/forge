// Semáforos de macOS (cerrar, minimizar, zoom) centrados en la barra superior de Forge,
// como Warp: AppKit los deja arriba del todo en una barra de título de 28 pt. Se
// recolocan en cada fotograma porque macOS los repone al redimensionar o salir de
// pantalla completa.
use objc2_app_kit::{NSView, NSWindowButton};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Centra los semáforos en una barra de `bar_height` puntos y devuelve dónde acaban (x).
pub fn center(frame: &eframe::Frame, bar_height: f64) -> Option<f32> {
    let handle = frame.window_handle().ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    // SAFETY: eframe da la NSView de su ventana, viva mientras dura el fotograma, y
    // estamos en el hilo principal (donde corre la interfaz).
    let view: &NSView = unsafe { appkit.ns_view.cast().as_ref() };
    let window = view.window()?;
    let close = window.standardWindowButton(NSWindowButton::CloseButton)?;
    let zoom = window.standardWindowButton(NSWindowButton::ZoomButton)?;
    // El contenedor de la barra de título: se alarga para que los botones queden centrados.
    // SAFETY: vistas de AppKit en el hilo principal; solo se leen sus padres.
    let container = unsafe { close.superview()?.superview()? };
    // Los botones guardan su distancia al borde inferior del contenedor (origin.y, AppKit
    // cuenta desde abajo): para centrarlos a `bar_height / 2` desde arriba, el contenedor
    // debe medir ese centro + media altura de botón + esa distancia.
    let button = close.frame();
    let height = bar_height / 2.0 + button.size.height / 2.0 + button.origin.y;
    let mut rect = container.frame();
    if (rect.size.height - height).abs() > 0.5 {
        rect.size.height = height;
        rect.origin.y = window.frame().size.height - height;
        container.setFrame(rect);
    }
    let z = zoom.frame();
    Some((z.origin.x + z.size.width) as f32)
}
