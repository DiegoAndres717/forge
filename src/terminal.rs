// Panel de terminal: PTY real + emulación VT (alacritty_terminal) + render e input en egui.
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode, viewport_to_point};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Processor, Rgb};
use eframe::egui::{
    self, Color32, CursorIcon, FontFamily, FontId, Key, Modifiers, PointerButton, Pos2, Rect,
    Sense, Shape, Stroke, StrokeKind, Vec2,
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

/// Eventos del terminal que la UI debe atender en su hilo.
enum UiEvent {
    Title(String),
    ResetTitle,
    Clipboard(String),
}

struct Listener {
    writer: Writer,
    tx: Sender<UiEvent>,
    ctx: egui::Context,
}

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let ui_event = match event {
            TermEvent::PtyWrite(text) => {
                let _ = self.writer.lock().unwrap().write_all(text.as_bytes());
                return;
            }
            // OSC 10/11/4: apps como Claude Code preguntan el fondo para elegir tema.
            TermEvent::ColorRequest(index, reply) => {
                let text = reply(rgb_for_index(index));
                let _ = self.writer.lock().unwrap().write_all(text.as_bytes());
                return;
            }
            TermEvent::Title(title) => UiEvent::Title(title),
            TermEvent::ResetTitle => UiEvent::ResetTitle,
            TermEvent::ClipboardStore(_, text) => UiEvent::Clipboard(text),
            _ => return,
        };
        let _ = self.tx.send(ui_event);
        self.ctx.request_repaint();
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Métricas de fuente compartidas por todos los paneles.
pub struct Metrics {
    pub font_size: f32,
    pub cell: Vec2,
    pub option_as_meta: bool,
}

impl Metrics {
    fn font(&self, flags: Flags) -> FontId {
        let family = match (flags.contains(Flags::BOLD), flags.contains(Flags::ITALIC)) {
            (false, false) => return FontId::monospace(self.font_size),
            (true, false) => "bold",
            (false, true) => "italic",
            (true, true) => "bold_italic",
        };
        FontId::new(self.font_size, FontFamily::Name(family.into()))
    }
}

pub struct Terminal {
    term: Arc<Mutex<Term<Listener>>>,
    writer: Writer,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    exited: Arc<AtomicBool>,
    events: Receiver<UiEvent>,
    size: Size,
    scroll_acc: f32,
    had_focus: bool,
    mouse_down: Option<u8>,
    pub title: Option<String>,
    /// URLs locales vistas en la salida (p. ej. "http://localhost:5173/" de Vite).
    urls: Arc<Mutex<Vec<String>>>,
    exit_code: Option<u32>,
}

type SpawnResult = Result<Terminal, Box<dyn std::error::Error + Send + Sync>>;

impl Terminal {
    /// Abre el shell de login en `cwd`; si hay `command`, lo escribe en el shell al arrancar
    /// (así queda en el historial y, al terminar o con Ctrl+C, se vuelve al prompt).
    pub fn spawn(
        ctx: &egui::Context,
        cwd: &Path,
        command: Option<&str>,
        env: &HashMap<String, String>,
    ) -> SpawnResult {
        Self::start(ctx, cwd, env, CommandBuilder::new_default_prog(), command)
    }

    /// Ejecuta `command` en un shell de login interactivo (carga PATH, nvm, etc.) que termina
    /// con él: el código de salida es el del comando. Para procesos administrados.
    pub fn exec(
        ctx: &egui::Context,
        cwd: &Path,
        command: &str,
        env: &HashMap<String, String>,
    ) -> SpawnResult {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let mut cmd = CommandBuilder::new(shell);
        cmd.args(["-l", "-i", "-c", command]);
        Self::start(ctx, cwd, env, cmd, None)
    }

    fn start(
        ctx: &egui::Context,
        cwd: &Path,
        env: &HashMap<String, String>,
        mut cmd: CommandBuilder,
        input: Option<&str>,
    ) -> SpawnResult {
        let size = Size { cols: 80, rows: 24 };
        let pair = native_pty_system().openpty(pty_size(size))?;

        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Forge");
        for (key, value) in env {
            cmd.env(key, value);
        }
        cmd.cwd(cwd);
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let writer: Writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        let (tx, events) = channel();
        let listener = Listener {
            writer: writer.clone(),
            tx,
            ctx: ctx.clone(),
        };
        let term = Arc::new(Mutex::new(Term::new(Config::default(), &size, listener)));
        let exited = Arc::new(AtomicBool::new(false));

        // ponytail: un hilo bloqueante por PTY; tokio si los paneles se cuentan por cientos.
        let mut reader = pair.master.try_clone_reader()?;
        let urls = Arc::new(Mutex::new(Vec::new()));
        let (t, e, c, u) = (term.clone(), exited.clone(), ctx.clone(), urls.clone());
        std::thread::spawn(move || {
            let mut parser: Processor = Processor::new();
            let mut buf = [0u8; 64 * 1024];
            let mut scanner = UrlScanner::default();
            while let Ok(n @ 1..) = reader.read(&mut buf) {
                parser.advance(&mut *t.lock().unwrap(), &buf[..n]);
                for url in scanner.feed(&buf[..n]) {
                    let mut urls = u.lock().unwrap();
                    if !urls.contains(&url) && urls.len() < 8 {
                        urls.push(url);
                    }
                }
                c.request_repaint();
            }
            e.store(true, Ordering::Relaxed);
            c.request_repaint();
        });

        if let Some(input) = input {
            let _ = writer
                .lock()
                .unwrap()
                .write_all(format!("{input}\r").as_bytes());
        }

        Ok(Self {
            term,
            writer,
            master: pair.master,
            child,
            exited,
            events,
            size,
            scroll_acc: 0.0,
            had_focus: true,
            mouse_down: None,
            title: None,
            urls,
            exit_code: None,
        })
    }

    pub fn exited(&self) -> bool {
        self.exited.load(Ordering::Relaxed)
    }

    /// Código de salida cuando el proceso ya terminó.
    pub fn exit_code(&mut self) -> Option<u32> {
        if self.exit_code.is_none()
            && let Ok(Some(status)) = self.child.try_wait()
        {
            self.exit_code = Some(status.exit_code());
        }
        self.exit_code
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    pub fn urls(&self) -> Vec<String> {
        self.urls.lock().unwrap().clone()
    }

    /// Ctrl+C: el driver de la tty envía SIGINT al grupo en primer plano (parada limpia).
    pub fn interrupt(&self) {
        self.send(b"\x03");
    }

    /// Mata el grupo de procesos entero (npm → node → esbuild...).
    pub fn kill(&mut self, signal: libc::c_int) {
        if let Some(pid) = self.pid() {
            // SAFETY: killpg solo envía una señal; pid es el líder de la sesión creada con setsid.
            unsafe { libc::killpg(pid as libc::pid_t, signal) };
        }
        let _ = self.child.kill();
    }

    /// Directorio actual del shell (para abrir terminales nuevas en el mismo sitio).
    pub fn cwd(&self) -> Option<PathBuf> {
        process_cwd(self.child.process_id()?)
    }

    fn send(&self, bytes: &[u8]) {
        let _ = self.writer.lock().unwrap().write_all(bytes);
    }

    /// Dibuja el terminal en `rect` y, si tiene el foco, procesa el teclado.
    /// El ratón se atiende siempre; la respuesta permite al llamador cambiar el foco.
    pub fn ui(
        &mut self,
        ui: &egui::Ui,
        rect: Rect,
        id: egui::Id,
        focused: bool,
        m: &Metrics,
    ) -> egui::Response {
        let ctx = ui.ctx().clone();

        while let Ok(event) = self.events.try_recv() {
            match event {
                UiEvent::Title(t) => self.title = Some(t),
                UiEvent::ResetTitle => self.title = None,
                UiEvent::Clipboard(text) => ctx.copy_text(text),
            }
        }

        let size = Size {
            cols: ((rect.width() / m.cell.x) as usize).max(2),
            rows: ((rect.height() / m.cell.y) as usize).max(1),
        };
        if size != self.size {
            self.size = size;
            self.term.lock().unwrap().resize(size);
            let _ = self.master.resize(pty_size(size));
        }

        let response = ui.interact(rect, id, Sense::click_and_drag());
        if focused {
            self.keyboard(&ctx, m);
        }
        self.mouse(&ctx, &response, rect, m);
        let painter = ui.painter_at(rect);
        paint(&self.term.lock().unwrap(), &painter, rect, focused, m);
        response
    }

    fn keyboard(&mut self, ctx: &egui::Context, m: &Metrics) {
        let mode = *self.term.lock().unwrap().mode();
        let (events, window_focused, alt) =
            ctx.input(|i| (i.events.clone(), i.focused, i.modifiers.alt));

        // Reporta foco solo si la app lo pidió (modo 1004).
        if window_focused != self.had_focus {
            self.had_focus = window_focused;
            if mode.contains(TermMode::FOCUS_IN_OUT) {
                self.send(if window_focused { b"\x1b[I" } else { b"\x1b[O" });
            }
        }

        for event in events {
            match event {
                // Con Option como Meta, el texto "∂" lo reemplaza ESC+d desde key_bytes.
                egui::Event::Text(text) if !(alt && m.option_as_meta) => {
                    self.scroll_to_bottom();
                    self.send(text.as_bytes());
                }
                egui::Event::Paste(text) => {
                    self.scroll_to_bottom();
                    // Normaliza saltos de línea como Terminal.app/iTerm.
                    let text = text.replace("\r\n", "\r").replace('\n', "\r");
                    if mode.contains(TermMode::BRACKETED_PASTE) {
                        let text = text.replace('\x1b', "");
                        self.send(format!("\x1b[200~{text}\x1b[201~").as_bytes());
                    } else {
                        self.send(text.as_bytes());
                    }
                }
                egui::Event::Copy => {
                    if let Some(text) = self.term.lock().unwrap().selection_to_string() {
                        ctx.copy_text(text);
                    }
                }
                egui::Event::Key {
                    key: Key::K,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.mac_cmd => {
                    // ⌘K: limpia pantalla y scrollback, como Terminal.app.
                    self.term.lock().unwrap().grid_mut().clear_history();
                    self.send(b"\x0c");
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if let Some(bytes) = key_bytes(key, modifiers, mode, m.option_as_meta) {
                        self.scroll_to_bottom();
                        self.term.lock().unwrap().selection = None;
                        self.send(&bytes);
                    }
                }
                _ => {}
            }
        }
    }

    fn scroll_to_bottom(&self) {
        self.term.lock().unwrap().scroll_display(Scroll::Bottom);
    }

    /// Celda del viewport bajo `pos` (línea, columna, lado).
    fn cell_at(&self, pos: Pos2, rect: Rect, m: &Metrics) -> (usize, usize, Side) {
        let x = ((pos.x - rect.min.x) / m.cell.x).max(0.0);
        let y = ((pos.y - rect.min.y) / m.cell.y).max(0.0);
        let col = (x as usize).min(self.size.cols - 1);
        let line = (y as usize).min(self.size.rows - 1);
        let side = if x.fract() > 0.5 {
            Side::Right
        } else {
            Side::Left
        };
        (line, col, side)
    }

    fn mouse(&mut self, ctx: &egui::Context, response: &egui::Response, rect: Rect, m: &Metrics) {
        let (events, scroll, hover, mods) = ctx.input(|i| {
            (
                i.events.clone(),
                i.smooth_scroll_delta.y,
                i.pointer.hover_pos(),
                i.modifiers,
            )
        });
        let mut term = self.term.lock().unwrap();
        let mode = *term.mode();
        let offset = term.grid().display_offset();
        // Shift fuerza la selección local aunque la app capture el ratón (como iTerm).
        let report = mode.intersects(TermMode::MOUSE_MODE) && !mods.shift;

        // Scroll: a la app si captura ratón; flechas en pantalla alternativa; si no, scrollback.
        if hover.is_some_and(|p| rect.contains(p)) && scroll != 0.0 {
            self.scroll_acc += scroll;
            let lines = (self.scroll_acc / m.cell.y).trunc();
            if lines != 0.0 {
                self.scroll_acc -= lines * m.cell.y;
                let (line, col, _) = self.cell_at(hover.unwrap_or(rect.min), rect, m);
                let count = lines.abs() as usize;
                if report {
                    let button = if lines > 0.0 { 64 } else { 65 };
                    let seq = mouse_report(button, col, line, true, mode);
                    drop(term);
                    for _ in 0..count {
                        self.send(&seq);
                    }
                    return;
                } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
                    let arrow: &[u8] = if lines > 0.0 { b"\x1bOA" } else { b"\x1bOB" };
                    drop(term);
                    self.send(&arrow.repeat(count));
                    return;
                }
                term.scroll_display(Scroll::Delta(lines as i32));
            }
        }

        let mut outgoing = Vec::new();
        for event in &events {
            match *event {
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers,
                } if rect.contains(pos) || !pressed => {
                    let (line, col, side) = self.cell_at(pos, rect, m);
                    let code = match button {
                        PointerButton::Primary => 0,
                        PointerButton::Middle => 1,
                        PointerButton::Secondary => 2,
                        _ => continue,
                    };
                    if report {
                        if pressed || self.mouse_down.is_some() {
                            outgoing.extend(mouse_report(code, col, line, pressed, mode));
                        }
                        self.mouse_down = pressed.then_some(code);
                        continue;
                    }
                    if button != PointerButton::Primary || !pressed {
                        continue;
                    }
                    let point = viewport_to_point(offset, Point::new(line, Column(col)));
                    if modifiers.mac_cmd {
                        // ⌘+clic abre la URL bajo el puntero.
                        if let Some(url) = url_at_point(&term, point) {
                            ctx.open_url(egui::OpenUrl::new_tab(url));
                        }
                    } else {
                        term.selection = Some(Selection::new(SelectionType::Simple, point, side));
                    }
                }
                egui::Event::PointerMoved(pos) => {
                    let (line, col, side) = self.cell_at(pos, rect, m);
                    if report {
                        if let Some(code) = self.mouse_down
                            && mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
                        {
                            outgoing.extend(mouse_report(code + 32, col, line, true, mode));
                        }
                    } else if response.dragged_by(PointerButton::Primary) {
                        let point = viewport_to_point(offset, Point::new(line, Column(col)));
                        if let Some(selection) = term.selection.as_mut() {
                            selection.update(point, side);
                        }
                    }
                }
                _ => {}
            }
        }

        // Doble clic selecciona palabra; triple clic, la línea.
        if !report {
            let kind = if response.triple_clicked() {
                Some(SelectionType::Lines)
            } else if response.double_clicked() {
                Some(SelectionType::Semantic)
            } else {
                None
            };
            if let (Some(kind), Some(pos)) = (kind, response.interact_pointer_pos()) {
                let (line, col, side) = self.cell_at(pos, rect, m);
                let point = viewport_to_point(offset, Point::new(line, Column(col)));
                term.selection = Some(Selection::new(kind, point, side));
            } else if response.clicked() {
                // Un clic simple sin arrastre no deja selección vacía.
                term.selection = None;
            }
        }

        // ⌘ sobre una URL: cursor de mano (el subrayado se pinta en paint()).
        if mods.mac_cmd && hover.is_some_and(|p| rect.contains(p)) {
            let (line, col, _) = self.cell_at(hover.unwrap_or(rect.min), rect, m);
            let point = viewport_to_point(offset, Point::new(line, Column(col)));
            if url_at_point(&term, point).is_some() {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
            }
        } else if hover.is_some_and(|p| rect.contains(p)) && !report {
            ctx.set_cursor_icon(CursorIcon::Text);
        }

        drop(term);
        if !outgoing.is_empty() {
            self.send(&outgoing);
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Cerrar el panel cuelga la sesión entera, no solo el shell: así un `npm run dev`
        // no queda huérfano ocupando el puerto.
        if !self.exited() {
            self.kill(libc::SIGHUP);
        }
    }
}

/// Busca URLs locales en la salida cruda, ignorando secuencias de escape y tolerando
/// que una URL llegue partida entre dos lecturas.
#[derive(Default)]
struct UrlScanner {
    carry: String,
}

impl UrlScanner {
    fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut text = std::mem::take(&mut self.carry);
        text.push_str(&strip_ansi(&String::from_utf8_lossy(bytes)));
        // La última línea puede estar incompleta: se guarda para la próxima lectura.
        let split = text.rfind(['\n', '\r']).map_or(0, |i| i + 1);
        let tail = text.split_off(split);
        if tail.len() <= 4096 {
            self.carry = tail;
        }
        text.lines().flat_map(local_urls).collect()
    }
}

pub(crate) fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: ESC [ ... byte final en @..~
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... BEL o ESC \
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// URLs http(s) a localhost/127.0.0.1/0.0.0.0/[::1] (0.0.0.0 se reescribe a localhost).
fn local_urls(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = line;
    while let Some(start) = ["http://", "https://"]
        .iter()
        .filter_map(|s| rest.find(s))
        .min()
    {
        let candidate = &rest[start..];
        let end = candidate
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`'))
            .unwrap_or(candidate.len());
        let url = candidate[..end].trim_end_matches(['.', ',', ';', ')', ']']);
        let host = url.split("://").nth(1).unwrap_or("");
        let local = ["localhost", "127.0.0.1", "0.0.0.0", "[::1]", "[::]"]
            .iter()
            .any(|h| host.starts_with(h));
        if local {
            found.push(
                url.replacen("0.0.0.0", "localhost", 1)
                    .replacen("[::]", "localhost", 1),
            );
        }
        rest = &candidate[end..];
    }
    found
}

fn paint<T: EventListener>(
    term: &Term<T>,
    painter: &egui::Painter,
    rect: Rect,
    focused: bool,
    m: &Metrics,
) {
    let default_bg = color(Color::Named(NamedColor::Background));
    painter.rect_filled(rect, 0.0, default_bg);

    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let cursor = content.cursor;
    let selection = content.selection;
    let cell_rect = |line: i32, col: usize, width: usize| {
        let pos = rect.min + Vec2::new(col as f32 * m.cell.x, (line + offset) as f32 * m.cell.y);
        Rect::from_min_size(pos, Vec2::new(m.cell.x * width as f32, m.cell.y))
    };
    let block_cursor = focused && cursor.shape == CursorShape::Block;

    // Fondo en tramos contiguos del mismo color para reducir formas.
    let mut run: Option<(i32, usize, usize, Color32)> = None;
    let flush = |run: &mut Option<(i32, usize, usize, Color32)>| {
        if let Some((line, start, end, c)) = run.take() {
            painter.rect_filled(cell_rect(line, start, end - start), 0.0, c);
        }
    };

    // ponytail: una forma de texto por celda (egui cachea glifos en un atlas y pinta en GPU
    // vía wgpu); ver bench_render: renderer de glifos propio solo si el perfilado lo pide.
    let mut foreground = Vec::new();
    for cell in content.display_iter {
        let flags = cell.flags;
        if flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let point = cell.point;
        let width = if flags.contains(Flags::WIDE_CHAR) {
            2
        } else {
            1
        };
        let (mut fg, mut bg) = (cell_fg(cell.fg, flags), color(cell.bg));
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if selection.is_some_and(|s| s.contains(point)) {
            bg = Color32::from_rgb(0x1f, 0x3d, 0x66);
        }
        if block_cursor && point == cursor.point {
            (fg, bg) = (default_bg, color(Color::Named(NamedColor::Cursor)));
        }

        let col = point.column.0;
        match &mut run {
            Some((l, _, end, c)) if *l == point.line.0 && *end == col && *c == bg => *end += width,
            _ => {
                flush(&mut run);
                if bg != default_bg {
                    run = Some((point.line.0, col, col + width, bg));
                }
            }
        }

        let r = cell_rect(point.line.0, col, width);
        if cell.c != ' ' && cell.c != '\t' && !flags.contains(Flags::HIDDEN) {
            let mut text = String::from(cell.c);
            if let Some(extra) = cell.zerowidth() {
                text.extend(extra);
            }
            let galley = painter.layout_no_wrap(text, m.font(flags), fg);
            foreground.push(Shape::galley(r.min, galley, fg));
        }
        if flags.intersects(Flags::ALL_UNDERLINES) {
            foreground.push(Shape::hline(
                r.x_range(),
                r.bottom() - 1.5,
                Stroke::new(1.0, fg),
            ));
        }
        if flags.contains(Flags::STRIKEOUT) {
            foreground.push(Shape::hline(
                r.x_range(),
                r.center().y,
                Stroke::new(1.0, fg),
            ));
        }
    }
    flush(&mut run);
    // Texto encima de todos los fondos.
    painter.extend(foreground);

    // Cursor no-bloque (o bloque sin foco: hueco).
    let r = cell_rect(cursor.point.line.0, cursor.point.column.0, 1);
    let cursor_color = color(Color::Named(NamedColor::Cursor));
    match cursor.shape {
        CursorShape::Hidden => {}
        CursorShape::Block if focused => {}
        CursorShape::Beam => {
            painter.rect_filled(
                Rect::from_min_size(r.min, Vec2::new(2.0, r.height())),
                0.0,
                cursor_color,
            );
        }
        CursorShape::Underline => {
            painter.hline(
                r.x_range(),
                r.bottom() - 1.0,
                Stroke::new(2.0, cursor_color),
            );
        }
        _ => {
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, cursor_color), StrokeKind::Inside);
        }
    }
}

#[cfg(target_os = "macos")]
fn process_cwd(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: proc_pidinfo escribe como máximo `size` bytes en `info`, que es POD.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let written = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&mut info as *mut libc::proc_vnodepathinfo).cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    // vip_path es un [c_char; MAXPATHLEN] partido en 32x32 por compatibilidad de libc.
    let path = info.pvi_cdir.vip_path.as_flattened();
    // SAFETY: c_char e u8 tienen el mismo tamaño y alineación.
    let bytes = unsafe { std::slice::from_raw_parts(path.as_ptr().cast::<u8>(), path.len()) };
    let path = std::ffi::CStr::from_bytes_until_nul(bytes).ok()?;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes())))
}

#[cfg(not(target_os = "macos"))]
fn process_cwd(pid: u32) -> Option<PathBuf> {
    // ponytail: Linux; Windows (Fase 12) necesitará otra vía.
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

fn pty_size(size: Size) -> PtySize {
    PtySize {
        rows: size.rows as u16,
        cols: size.cols as u16,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Secuencia de reporte de ratón (SGR si la app lo pidió; si no, X10 clásico).
fn mouse_report(button: u8, col: usize, line: usize, pressed: bool, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if pressed { 'M' } else { 'm' };
        format!("\x1b[<{button};{};{}{end}", col + 1, line + 1).into_bytes()
    } else {
        let b = if pressed { button } else { 3 };
        let clamp = |v: usize| (32 + 1 + v).min(255) as u8;
        vec![0x1b, b'[', b'M', 32 + b, clamp(col), clamp(line)]
    }
}

/// Texto de la fila en `point` y la URL que cubre su columna.
fn url_at_point<T>(term: &Term<T>, point: Point) -> Option<String> {
    let row = &term.grid()[point.line];
    let chars: Vec<char> = (0..term.columns()).map(|c| row[Column(c)].c).collect();
    url_at(&chars, point.column.0)
}

// ponytail: URLs de una sola fila; las que el shell parte en dos líneas no se detectan.
fn url_at(chars: &[char], col: usize) -> Option<String> {
    let stop = |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`' | '│');
    if col >= chars.len() || stop(chars[col]) {
        return None;
    }
    let start = (0..col)
        .rev()
        .take_while(|&i| !stop(chars[i]))
        .last()
        .unwrap_or(col);
    let end = (col..chars.len()).take_while(|&i| !stop(chars[i])).last()? + 1;
    let token: String = chars[start..end].iter().collect();

    let scheme = ["https://", "http://", "file://"]
        .iter()
        .filter_map(|s| token.find(s))
        .min()?;
    // La URL debe empezar antes del clic.
    if start + token[..scheme].chars().count() > col {
        return None;
    }
    let url = token[scheme..].trim_end_matches(['.', ',', ';', ':', ')', ']', '}', '!', '?']);
    (url.len() > "https://".len()).then(|| url.to_string())
}

/// Traduce teclas especiales y combinaciones a secuencias de terminal.
pub fn key_bytes(key: Key, m: Modifiers, mode: TermMode, option_as_meta: bool) -> Option<Vec<u8>> {
    let ss3_or_csi = |c: char| {
        let prefix = if mode.contains(TermMode::APP_CURSOR) {
            "\x1bO"
        } else {
            "\x1b["
        };
        format!("{prefix}{c}").into_bytes()
    };
    // Modificadores estilo xterm: 1 + shift(1) + alt(2) + ctrl(4).
    let xterm_mod = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8;
    let cursor_key = |c: char| {
        if xterm_mod > 1 {
            format!("\x1b[1;{xterm_mod}{c}").into_bytes()
        } else {
            ss3_or_csi(c)
        }
    };
    let tilde = |n: u8| {
        if xterm_mod > 1 {
            format!("\x1b[{n};{xterm_mod}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };

    // Atajos de edición de macOS (como Terminal.app/iTerm).
    if m.mac_cmd {
        return match key {
            Key::ArrowLeft => Some(vec![0x01]),  // inicio de línea
            Key::ArrowRight => Some(vec![0x05]), // fin de línea
            Key::Backspace => Some(vec![0x15]),  // borrar línea
            _ => None,
        };
    }
    if m.alt && !m.ctrl && !m.shift {
        match key {
            Key::ArrowLeft => return Some(b"\x1bb".to_vec()),
            Key::ArrowRight => return Some(b"\x1bf".to_vec()),
            Key::Backspace => return Some(b"\x1b\x7f".to_vec()),
            _ => {}
        }
    }

    let name = key.name();
    let single = match name.as_bytes() {
        [b] if b.is_ascii_alphanumeric() => Some(*b),
        _ => None,
    };

    // Ctrl+letra: Ctrl+C = 0x03, Ctrl+D = 0x04...
    if m.ctrl {
        if let Some(b @ b'A'..=b'Z') = single {
            return Some(vec![b - b'A' + 1]);
        }
        match key {
            Key::Space => return Some(vec![0]),
            Key::OpenBracket => return Some(vec![0x1b]),
            Key::Backslash => return Some(vec![0x1c]),
            Key::CloseBracket => return Some(vec![0x1d]),
            _ => {}
        }
    }

    // Option como Meta: ESC + tecla.
    if m.alt
        && option_as_meta
        && let Some(b) = single
    {
        let b = if m.shift { b } else { b.to_ascii_lowercase() };
        return Some(vec![0x1b, b]);
    }

    match key {
        // Shift/Option+Enter = nueva línea en Claude Code y la mayoría de TUIs.
        Key::Enter if m.shift || m.alt => Some(b"\x1b\r".to_vec()),
        Key::Enter => Some(b"\r".to_vec()),
        Key::Backspace if m.ctrl => Some(vec![0x08]),
        Key::Backspace => Some(b"\x7f".to_vec()),
        Key::Tab if m.shift => Some(b"\x1b[Z".to_vec()),
        Key::Tab => Some(b"\t".to_vec()),
        Key::Escape => Some(b"\x1b".to_vec()),
        Key::ArrowUp => Some(cursor_key('A')),
        Key::ArrowDown => Some(cursor_key('B')),
        Key::ArrowRight => Some(cursor_key('C')),
        Key::ArrowLeft => Some(cursor_key('D')),
        Key::Home => Some(cursor_key('H')),
        Key::End => Some(cursor_key('F')),
        Key::Insert => Some(tilde(2)),
        Key::Delete => Some(tilde(3)),
        Key::PageUp => Some(tilde(5)),
        Key::PageDown => Some(tilde(6)),
        Key::F1 => Some(b"\x1bOP".to_vec()),
        Key::F2 => Some(b"\x1bOQ".to_vec()),
        Key::F3 => Some(b"\x1bOR".to_vec()),
        Key::F4 => Some(b"\x1bOS".to_vec()),
        Key::F5 => Some(tilde(15)),
        Key::F6 => Some(tilde(17)),
        Key::F7 => Some(tilde(18)),
        Key::F8 => Some(tilde(19)),
        Key::F9 => Some(tilde(20)),
        Key::F10 => Some(tilde(21)),
        Key::F11 => Some(tilde(23)),
        Key::F12 => Some(tilde(24)),
        _ => None,
    }
}

// ponytail: paleta fija (Tomorrow Night); los temas llegan con la configuración.
// Colores del sistema de macOS (modo oscuro), como en Terminal.app con un tema moderno.
const PALETTE: [(u8, u8, u8); 16] = [
    (0x32, 0x32, 0x34), // negro
    (0xff, 0x45, 0x3a), // rojo
    (0x32, 0xd7, 0x4b), // verde
    (0xff, 0xd6, 0x0a), // amarillo
    (0x0a, 0x84, 0xff), // azul
    (0xbf, 0x5a, 0xf2), // magenta
    (0x64, 0xd2, 0xff), // cian
    (0xd1, 0xd1, 0xd6), // blanco
    (0x63, 0x63, 0x66), // negro brillante
    (0xff, 0x69, 0x61), // rojo brillante
    (0x5b, 0xe3, 0x7a), // verde brillante
    (0xff, 0xe4, 0x5e), // amarillo brillante
    (0x40, 0x9c, 0xff), // azul brillante
    (0xda, 0x8f, 0xff), // magenta brillante
    (0x8e, 0xdc, 0xff), // cian brillante
    (0xf2, 0xf2, 0xf7), // blanco brillante
];
const FOREGROUND: (u8, u8, u8) = (0xe5, 0xe5, 0xea);
const BACKGROUND: (u8, u8, u8) = (0x1c, 0x1c, 0x1e);
const CURSOR: (u8, u8, u8) = (0xf2, 0xf2, 0xf7);

/// Color de primer plano: negrita aclara los 8 colores base; DIM los atenúa.
fn cell_fg(c: Color, flags: Flags) -> Color32 {
    let c = match c {
        Color::Named(n) if flags.contains(Flags::BOLD) && (n as usize) < 8 => {
            Color::Indexed(n as u8 + 8)
        }
        other => other,
    };
    let fg = color(c);
    if flags.contains(Flags::DIM) {
        fg.gamma_multiply(0.66)
    } else {
        fg
    }
}

fn rgb_for_index(index: usize) -> Rgb {
    let (r, g, b) = match index {
        0..256 => indexed(index as u8),
        256 => FOREGROUND,
        258 => CURSOR,
        _ => BACKGROUND,
    };
    Rgb { r, g, b }
}

pub fn color(c: Color) -> Color32 {
    let rgb = |(r, g, b): (u8, u8, u8)| Color32::from_rgb(r, g, b);
    match c {
        Color::Spec(Rgb { r, g, b }) => Color32::from_rgb(r, g, b),
        Color::Indexed(i) => rgb(indexed(i)),
        Color::Named(NamedColor::Foreground | NamedColor::BrightForeground) => rgb(FOREGROUND),
        Color::Named(NamedColor::DimForeground) => rgb(FOREGROUND).gamma_multiply(0.66),
        Color::Named(NamedColor::Background) => rgb(BACKGROUND),
        Color::Named(NamedColor::Cursor) => rgb(CURSOR),
        Color::Named(n) => {
            let i = n as usize;
            if i < 16 {
                rgb(PALETTE[i])
            } else {
                // Dim*: el color base atenuado.
                rgb(PALETTE[(i - NamedColor::DimBlack as usize) % 8]).gamma_multiply(0.66)
            }
        }
    }
}

fn indexed(i: u8) -> (u8, u8, u8) {
    match i {
        0..16 => PALETTE[i as usize],
        16..232 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(i / 36), level(i / 6 % 6), level(i % 6))
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            (v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(key: Key, m: Modifiers) -> Option<Vec<u8>> {
        key_bytes(key, m, TermMode::empty(), true)
    }

    #[test]
    fn control_and_meta_keys() {
        assert_eq!(keys(Key::C, Modifiers::CTRL), Some(vec![0x03]));
        assert_eq!(keys(Key::B, Modifiers::ALT), Some(b"\x1bb".to_vec()));
        assert_eq!(keys(Key::Enter, Modifiers::SHIFT), Some(b"\x1b\r".to_vec()));
        assert_eq!(keys(Key::ArrowLeft, Modifiers::MAC_CMD), Some(vec![0x01]));
        assert_eq!(keys(Key::A, Modifiers::NONE), None); // lo envía el evento Text
    }

    #[test]
    fn cursor_keys_follow_mode_and_modifiers() {
        let none = Modifiers::NONE;
        assert_eq!(
            key_bytes(Key::ArrowUp, none, TermMode::empty(), true),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            key_bytes(Key::ArrowUp, none, TermMode::APP_CURSOR, true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            keys(Key::ArrowUp, Modifiers::SHIFT),
            Some(b"\x1b[1;2A".to_vec())
        );
        assert_eq!(
            keys(Key::Delete, Modifiers::NONE),
            Some(b"\x1b[3~".to_vec())
        );
    }

    #[test]
    fn color_cube_and_gray() {
        assert_eq!(indexed(16), (0, 0, 0));
        assert_eq!(indexed(231), (255, 255, 255));
        assert_eq!(indexed(232), (8, 8, 8));
    }

    #[test]
    fn url_detection() {
        let row: Vec<char> = "ver (https://github.com/a/b). y http://x.io"
            .chars()
            .collect();
        assert_eq!(url_at(&row, 10).as_deref(), Some("https://github.com/a/b"));
        assert_eq!(url_at(&row, 2), None);
        assert_eq!(url_at(&row, 36).as_deref(), Some("http://x.io"));
        assert_eq!(url_at(&row, 31), None);
    }

    /// Coste de CPU por frame de una pantalla 200x60 llena de texto coloreado.
    /// `cargo test --release bench_render -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_render() {
        use alacritty_terminal::event::VoidListener;
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx, &crate::Settings::default()).unwrap();
        let size = Size {
            cols: 200,
            rows: 60,
        };
        let mut term = Term::new(Config::default(), &size, VoidListener);
        let mut parser: Processor = Processor::new();
        for i in 0..size.rows * 2 {
            let line: String = (0..size.cols)
                .map(|c| char::from(b'!' + ((c + i) % 90) as u8))
                .collect();
            parser.advance(
                &mut term,
                format!("\x1b[3{}m{line}\x1b[0m", i % 8).as_bytes(),
            );
        }
        // Las fuentes existen tras el primer frame.
        ctx.begin_pass(egui::RawInput::default());
        let _ = ctx.end_pass();
        let m = crate::metrics(&ctx, &crate::Settings::default());
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(m.cell.x * 200.0, m.cell.y * 60.0));

        let frames = 100;
        let start = std::time::Instant::now();
        for _ in 0..frames {
            ctx.begin_pass(egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            });
            let painter = egui::Painter::new(ctx.clone(), egui::LayerId::background(), rect);
            paint(&term, &painter, rect, true, &m);
            let out = ctx.end_pass();
            let meshes = ctx.tessellate(out.shapes, out.pixels_per_point);
            assert!(!meshes.is_empty());
        }
        let per_frame = start.elapsed() / frames;
        println!("render 200x60: {per_frame:?}/frame");
        assert!(
            per_frame.as_millis() < 16,
            "más lento que 60 fps: {per_frame:?}"
        );
    }

    /// zsh real en un PTY: el comando inicial se ejecuta y Forge ve el nuevo directorio.
    #[test]
    fn shell_cwd_follows_cd() {
        let ctx = egui::Context::default();
        let term =
            Terminal::spawn(&ctx, Path::new("/tmp"), Some("cd /usr"), &HashMap::new()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while term.cwd().as_deref() != Some(Path::new("/usr")) {
            assert!(
                std::time::Instant::now() < deadline,
                "cwd = {:?}",
                term.cwd()
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[test]
    fn detects_local_urls_across_chunks() {
        let mut s = UrlScanner::default();
        // Vite: color ANSI en medio y la URL partida entre dos lecturas.
        assert!(
            s.feed(b"  \x1b[32m\xE2\x9E\x9C\x1b[39m  Local:   \x1b[36mhttp://local")
                .is_empty()
        );
        assert_eq!(
            s.feed(b"host:\x1b[1m5173\x1b[22m/\x1b[39m\n"),
            vec!["http://localhost:5173/"]
        );
        assert_eq!(
            s.feed(b"listening on http://0.0.0.0:3000.\r\n"),
            vec!["http://localhost:3000"]
        );
        assert!(s.feed(b"docs: https://vitejs.dev/guide\n").is_empty());
        assert_eq!(
            s.feed(b"\x1b]8;;http://x\x07link\x1b]8;;\x07 http://127.0.0.1:8080\n"),
            vec!["http://127.0.0.1:8080"]
        );
    }

    /// Proceso administrado: el código de salida es el del comando.
    #[test]
    fn exec_reports_exit_code() {
        let ctx = egui::Context::default();
        let mut term = Terminal::exec(&ctx, Path::new("/tmp"), "exit 3", &HashMap::new()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while term.exit_code().is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "el proceso no terminó"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(term.exit_code(), Some(3));
    }

    #[test]
    fn cwd_of_own_process() {
        assert_eq!(
            process_cwd(std::process::id()),
            std::env::current_dir().ok()
        );
    }

    #[test]
    fn mouse_reports() {
        assert_eq!(
            mouse_report(0, 4, 9, true, TermMode::SGR_MOUSE),
            b"\x1b[<0;5;10M".to_vec()
        );
        assert_eq!(
            mouse_report(0, 4, 9, false, TermMode::SGR_MOUSE),
            b"\x1b[<0;5;10m".to_vec()
        );
        assert_eq!(
            mouse_report(0, 0, 0, true, TermMode::empty()),
            vec![0x1b, b'[', b'M', 32, 33, 33]
        );
    }
}
