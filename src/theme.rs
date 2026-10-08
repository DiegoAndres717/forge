// Sistema de diseño: colores del modo oscuro de macOS, tipografía (SF Pro / SF Mono),
// espaciados y componentes con aspecto nativo (filas de barra lateral, botones, píldoras).
use eframe::egui::{
    self, Color32, CornerRadius, CursorIcon, FontId, Margin, Rect, Response, RichText, Sense,
    Stroke, TextStyle, Ui, Vec2,
};

pub use egui_phosphor::regular as icon;

// Superficies (de más oscura a más clara).
pub const BG: Color32 = Color32::from_rgb(0x1c, 0x1c, 0x1e);
pub const SIDEBAR: Color32 = Color32::from_rgb(0x24, 0x24, 0x26);
pub const TOOLBAR: Color32 = Color32::from_rgb(0x22, 0x22, 0x24);
pub const SURFACE: Color32 = Color32::from_rgb(0x2c, 0x2c, 0x2e);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3c);
pub const SELECTED: Color32 = Color32::from_rgba_premultiplied(0x2e, 0x2e, 0x30, 0x2e);
pub const SEPARATOR: Color32 = Color32::from_rgb(0x38, 0x38, 0x3a);
pub const FIELD: Color32 = Color32::from_rgb(0x15, 0x15, 0x17);

// Texto (etiquetas primaria a cuaternaria de macOS).
pub const TEXT: Color32 = Color32::from_rgb(0xf2, 0xf2, 0xf7);
pub const TEXT_2: Color32 = Color32::from_rgb(0xc7, 0xc7, 0xcc);
pub const TEXT_3: Color32 = Color32::from_rgb(0x8e, 0x8e, 0x93);
pub const TEXT_4: Color32 = Color32::from_rgb(0x63, 0x63, 0x66);

// Colores del sistema (modo oscuro).
pub const ACCENT: Color32 = Color32::from_rgb(0x0a, 0x84, 0xff);
pub const GREEN: Color32 = Color32::from_rgb(0x30, 0xd1, 0x58);
pub const YELLOW: Color32 = Color32::from_rgb(0xff, 0xd6, 0x0a);
pub const ORANGE: Color32 = Color32::from_rgb(0xff, 0x9f, 0x0a);
pub const RED: Color32 = Color32::from_rgb(0xff, 0x45, 0x3a);
pub const PURPLE: Color32 = Color32::from_rgb(0xbf, 0x5a, 0xf2);

/// Alto de la barra de herramientas unificada (los semáforos de la ventana van en ella).
pub const TOOLBAR_HEIGHT: f32 = 40.0;
pub const STATUS_HEIGHT: f32 = 24.0;

pub fn apply(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        use egui::FontFamily::{Monospace, Proportional};
        style.text_styles = [
            (TextStyle::Small, FontId::new(11.0, Proportional)),
            (TextStyle::Body, FontId::new(13.0, Proportional)),
            (TextStyle::Button, FontId::new(13.0, Proportional)),
            (TextStyle::Heading, FontId::new(22.0, Proportional)),
            (TextStyle::Monospace, FontId::new(12.0, Monospace)),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = Vec2::new(8.0, 6.0);
        s.button_padding = Vec2::new(10.0, 4.0);
        s.interact_size.y = 24.0;
        s.window_margin = Margin::same(12);
        s.menu_margin = Margin::same(6);
        s.indent = 14.0;

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.window_stroke = Stroke::new(1.0, SEPARATOR);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        v.extreme_bg_color = FIELD;
        v.faint_bg_color = SIDEBAR;
        v.code_bg_color = FIELD;
        v.hyperlink_color = ACCENT;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.override_text_color = None;
        let radius = CornerRadius::same(6);
        let w = &mut v.widgets;
        w.noninteractive.bg_stroke = Stroke::new(1.0, SEPARATOR);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_2);
        w.noninteractive.corner_radius = radius;
        for (state, fill, text) in [
            (&mut w.inactive, SURFACE, TEXT),
            (&mut w.hovered, SURFACE_HOVER, TEXT),
            (&mut w.active, Color32::from_rgb(0x48, 0x48, 0x4a), TEXT),
            (&mut w.open, SURFACE_HOVER, TEXT),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::NONE;
            state.fg_stroke = Stroke::new(1.2, text);
            state.corner_radius = radius;
            state.expansion = 0.0;
        }
        w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x4a, 0x4a, 0x4c));
    });
}

/// Encabezado de sección de barra lateral ("PROYECTOS"), como en Finder.
pub fn section(ui: &mut Ui, text: &str) {
    if !ui.layout().is_horizontal() {
        ui.add_space(6.0);
    }
    ui.label(
        RichText::new(text.to_uppercase())
            .size(10.5)
            .color(TEXT_3)
            .strong(),
    );
}

/// Fila de lista (barra lateral): icono, texto, detalle a la derecha; resaltado al pasar
/// el ratón y cuando está seleccionada.
pub fn row(
    ui: &mut Ui,
    icon: &str,
    icon_color: Color32,
    label: &str,
    detail: &str,
    selected: bool,
) -> Response {
    let height = 26.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(
            rect,
            6.0,
            Color32::from_rgba_premultiplied(0x3a, 0x3a, 0x3e, 0xc0),
        );
    } else if response.hovered() {
        painter.rect_filled(rect, 6.0, SELECTED);
    }
    let mid = rect.center().y;
    if icon == "●" {
        // Punto de estado (procesos): más discreto que el glifo.
        painter.circle_filled(egui::pos2(rect.min.x + 17.0, mid), 4.0, icon_color);
    } else {
        painter.text(
            egui::pos2(rect.min.x + 10.0, mid),
            egui::Align2::LEFT_CENTER,
            icon,
            FontId::proportional(14.0),
            icon_color,
        );
    }
    let detail_width = if detail.is_empty() {
        0.0
    } else {
        let galley = painter.layout_no_wrap(detail.to_string(), FontId::proportional(11.0), TEXT_3);
        let w = galley.size().x;
        painter.galley(
            egui::pos2(rect.max.x - 8.0 - w, mid - galley.size().y / 2.0),
            galley,
            TEXT_3,
        );
        w + 12.0
    };
    let text_rect = Rect::from_min_max(
        egui::pos2(rect.min.x + 32.0, rect.min.y),
        egui::pos2(rect.max.x - 8.0 - detail_width, rect.max.y),
    );
    let galley = painter.layout(
        label.to_string(),
        FontId::proportional(13.0),
        TEXT,
        f32::INFINITY,
    );
    painter.with_clip_rect(text_rect).galley(
        egui::pos2(text_rect.min.x, mid - galley.size().y / 2.0),
        galley,
        TEXT,
    );
    // Nombre accesible (VoiceOver y tests de interfaz).
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label)
    });
    response.on_hover_cursor(CursorIcon::Default)
}

/// Botón de icono plano (barra de herramientas y cabeceras), con fondo al pasar el ratón.
pub fn icon_button(ui: &mut Ui, glyph: &str, tooltip: &str) -> Response {
    icon_button_sized(ui, glyph, tooltip, 26.0, TEXT_2)
}

pub fn icon_button_sized(
    ui: &mut Ui,
    glyph: &str,
    tooltip: &str,
    size: f32,
    color: Color32,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    paint_icon_button(ui, rect, glyph, &response, color);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tooltip));
    response.on_hover_text(tooltip)
}

/// Pinta un botón de icono en un rectángulo ya reservado (para cabeceras dibujadas a mano).
pub fn paint_icon_button(ui: &Ui, rect: Rect, glyph: &str, response: &Response, color: Color32) {
    if response.hovered() {
        ui.painter()
            .rect_filled(rect.shrink(1.0), 6.0, SURFACE_HOVER);
    }
    let color = if response.hovered() { TEXT } else { color };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(rect.height() * 0.58),
        color,
    );
}

/// Botón principal (relleno con el color de acento).
pub fn primary(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text.into()).color(Color32::WHITE).strong())
            .fill(ACCENT)
            .corner_radius(6.0)
            .min_size(Vec2::new(0.0, 26.0)),
    )
}

/// Botón secundario (relleno neutro).
pub fn secondary(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text.into()).color(TEXT))
            .fill(SURFACE_HOVER)
            .corner_radius(6.0)
            .min_size(Vec2::new(0.0, 26.0)),
    )
}

/// Contenedor agrupado con esquinas redondeadas (como las listas de Ajustes del Sistema).
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, Color32::from_rgb(0x34, 0x34, 0x36)))
        .corner_radius(10.0)
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Línea separadora fina.
pub fn hairline(ui: &Ui, rect: Rect, vertical: bool) {
    if vertical {
        ui.painter().vline(
            rect.max.x - 0.5,
            rect.y_range(),
            Stroke::new(1.0, SEPARATOR),
        );
    } else {
        ui.painter().hline(
            rect.x_range(),
            rect.max.y - 0.5,
            Stroke::new(1.0, SEPARATOR),
        );
    }
}
