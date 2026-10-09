// Árbol de paneles: hojas = paneles, nodos = divisiones con proporción ajustable.
use eframe::egui::{self, CursorIcon, Id, Rect, Sense, Vec2};
use serde::{Deserialize, Serialize};

pub type PanelId = u64;

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum Dir {
    /// Paneles lado a lado (división vertical, ⌘D).
    Row,
    /// Paneles uno encima del otro (división horizontal, ⌘⇧D).
    Column,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Toward {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Node {
    Leaf(PanelId),
    Split {
        dir: Dir,
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

const DIVIDER: f32 = 1.0;
const GRAB: f32 = 6.0;

impl Node {
    /// Divide el panel `target` y coloca `new` a la derecha o debajo.
    pub fn split(&mut self, target: PanelId, dir: Dir, new: PanelId) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                *self = Node::Split {
                    dir,
                    ratio: 0.5,
                    first: Box::new(Node::Leaf(target)),
                    second: Box::new(Node::Leaf(new)),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { first, second, .. } => {
                first.split(target, dir, new) || second.split(target, dir, new)
            }
        }
    }

    /// Pone `new` en el hueco de `old` (cambiar la terminal visible de un grupo).
    pub fn replace(&mut self, old: PanelId, new: PanelId) -> bool {
        match self {
            Node::Leaf(id) if *id == old => {
                *id = new;
                true
            }
            Node::Leaf(_) => false,
            Node::Split { first, second, .. } => {
                first.replace(old, new) || second.replace(old, new)
            }
        }
    }

    /// Orientación de la división que contiene directamente a `target` (None: está solo).
    pub fn parent_dir(&self, target: PanelId) -> Option<Dir> {
        match self {
            Node::Leaf(_) => None,
            Node::Split {
                dir, first, second, ..
            } => {
                let is = |n: &Node| matches!(n, Node::Leaf(id) if *id == target);
                if is(first) || is(second) {
                    Some(*dir)
                } else {
                    first
                        .parent_dir(target)
                        .or_else(|| second.parent_dir(target))
                }
            }
        }
    }

    /// Gira la división que contiene a `target`: lado a lado ↔ uno encima del otro.
    pub fn rotate(&mut self, target: PanelId) -> bool {
        match self {
            Node::Leaf(_) => false,
            Node::Split {
                dir, first, second, ..
            } => {
                let is = |n: &Node| matches!(n, Node::Leaf(id) if *id == target);
                if is(first) || is(second) {
                    *dir = match dir {
                        Dir::Row => Dir::Column,
                        Dir::Column => Dir::Row,
                    };
                    true
                } else {
                    first.rotate(target) || second.rotate(target)
                }
            }
        }
    }

    /// Quita un panel; su hermano ocupa el espacio. La raíz-hoja no se quita.
    pub fn remove(&mut self, target: PanelId) -> bool {
        let is = |n: &Node| matches!(n, Node::Leaf(id) if *id == target);
        let keep = match self {
            Node::Leaf(_) => return false,
            Node::Split { first, second, .. } => {
                if is(first) {
                    std::mem::replace(second.as_mut(), Node::Leaf(0))
                } else if is(second) {
                    std::mem::replace(first.as_mut(), Node::Leaf(0))
                } else {
                    return first.remove(target) || second.remove(target);
                }
            }
        };
        *self = keep;
        true
    }

    pub fn ids(&self) -> Vec<PanelId> {
        match self {
            Node::Leaf(id) => vec![*id],
            Node::Split { first, second, .. } => {
                let mut ids = first.ids();
                ids.extend(second.ids());
                ids
            }
        }
    }

    /// Rectángulo de cada panel.
    pub fn rects(&self, rect: Rect, out: &mut Vec<(PanelId, Rect)>) {
        match self {
            Node::Leaf(panel) => out.push((*panel, rect)),
            Node::Split {
                dir,
                ratio,
                first,
                second,
            } => {
                let (a, b, _) = split_rect(rect, *dir, *ratio);
                first.rects(a, out);
                second.rects(b, out);
            }
        }
    }

    /// Pinta los divisores y gestiona su arrastre. Se llama después de los paneles para que
    /// la zona de agarre (más ancha que la línea) quede por encima de ellos.
    pub fn dividers(&mut self, ui: &egui::Ui, rect: Rect, id: Id) {
        let Node::Split {
            dir,
            ratio,
            first,
            second,
        } = self
        else {
            return;
        };
        let (a, b, divider) = split_rect(rect, *dir, *ratio);

        let grab = match dir {
            Dir::Row => divider.expand2(Vec2::new(GRAB / 2.0, 0.0)),
            Dir::Column => divider.expand2(Vec2::new(0.0, GRAB / 2.0)),
        };
        let response = ui.interact(grab, id.with("divider"), Sense::drag());
        let icon = match dir {
            Dir::Row => CursorIcon::ResizeHorizontal,
            Dir::Column => CursorIcon::ResizeVertical,
        };
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(icon);
        }
        if let Some(pos) = response
            .interact_pointer_pos()
            .filter(|_| response.dragged())
        {
            let r = match dir {
                Dir::Row => (pos.x - rect.min.x) / rect.width(),
                Dir::Column => (pos.y - rect.min.y) / rect.height(),
            };
            *ratio = r.clamp(0.1, 0.9);
        }
        let color = if response.dragged() || response.hovered() {
            crate::theme::ACCENT
        } else {
            crate::theme::SEPARATOR
        };
        ui.painter().rect_filled(divider, 0.0, color);

        first.dividers(ui, a, id.with(0));
        second.dividers(ui, b, id.with(1));
    }
}

fn split_rect(rect: Rect, dir: Dir, ratio: f32) -> (Rect, Rect, Rect) {
    match dir {
        Dir::Row => {
            let x = (rect.min.x + rect.width() * ratio).round();
            (
                Rect::from_min_max(rect.min, egui::pos2(x, rect.max.y)),
                Rect::from_min_max(egui::pos2(x + DIVIDER, rect.min.y), rect.max),
                Rect::from_min_max(
                    egui::pos2(x, rect.min.y),
                    egui::pos2(x + DIVIDER, rect.max.y),
                ),
            )
        }
        Dir::Column => {
            let y = (rect.min.y + rect.height() * ratio).round();
            (
                Rect::from_min_max(rect.min, egui::pos2(rect.max.x, y)),
                Rect::from_min_max(egui::pos2(rect.min.x, y + DIVIDER), rect.max),
                Rect::from_min_max(
                    egui::pos2(rect.min.x, y),
                    egui::pos2(rect.max.x, y + DIVIDER),
                ),
            )
        }
    }
}

/// Panel vecino en una dirección: el más cercano que solape en el eje perpendicular.
pub fn neighbor(rects: &[(PanelId, Rect)], from: PanelId, toward: Toward) -> Option<PanelId> {
    let (_, src) = rects.iter().find(|(id, _)| *id == from)?;
    rects
        .iter()
        .filter(|(id, _)| *id != from)
        .filter_map(|(id, r)| {
            let (gap, overlap) = match toward {
                Toward::Left => (src.min.x - r.max.x, r.y_range().intersection(src.y_range())),
                Toward::Right => (r.min.x - src.max.x, r.y_range().intersection(src.y_range())),
                Toward::Up => (src.min.y - r.max.y, r.x_range().intersection(src.x_range())),
                Toward::Down => (r.min.y - src.max.y, r.x_range().intersection(src.x_range())),
            };
            (gap >= -DIVIDER && overlap.span() > 0.0).then_some((id, gap, -overlap.span()))
        })
        .min_by(|a, b| {
            (a.1, a.2)
                .partial_cmp(&(b.1, b.2))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(id, _, _)| *id)
}

/// Dirección de división automática (⌘T): a lo largo del lado más largo.
pub fn auto_dir(rect: Rect) -> Dir {
    if rect.width() >= rect.height() * 1.6 {
        Dir::Row
    } else {
        Dir::Column
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rects(node: &Node) -> Vec<(PanelId, Rect)> {
        let mut out = Vec::new();
        node.rects(
            Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1000.0, 600.0)),
            &mut out,
        );
        out
    }

    #[test]
    fn split_remove_and_neighbors() {
        // [1 | [2 / 3]]
        let mut root = Node::Leaf(1);
        assert!(root.split(1, Dir::Row, 2));
        assert!(root.split(2, Dir::Column, 3));
        assert_eq!(root.ids(), vec![1, 2, 3]);

        let r = rects(&root);
        assert_eq!(neighbor(&r, 1, Toward::Right), Some(2));
        assert_eq!(neighbor(&r, 3, Toward::Left), Some(1));
        assert_eq!(neighbor(&r, 2, Toward::Down), Some(3));
        assert_eq!(neighbor(&r, 3, Toward::Up), Some(2));
        assert_eq!(neighbor(&r, 1, Toward::Left), None);

        // Al cerrar 2, el 3 ocupa toda la columna derecha.
        assert!(root.remove(2));
        assert_eq!(root.ids(), vec![1, 3]);
        assert!(root.remove(1));
        assert_eq!(root, Node::Leaf(3));
        assert!(!root.remove(3));
    }

    #[test]
    fn rotating_flips_only_the_panels_own_split() {
        // (1 | (2 / 3)): girar el 3 gira la división 2/3; girar el 1, la de fuera.
        let mut layout = Node::Leaf(1);
        layout.split(1, Dir::Row, 2);
        layout.split(2, Dir::Column, 3);
        assert_eq!(layout.parent_dir(3), Some(Dir::Column));
        assert!(layout.rotate(3));
        assert_eq!(layout.parent_dir(3), Some(Dir::Row));
        assert_eq!(
            layout.parent_dir(1),
            Some(Dir::Row),
            "la de fuera no cambia"
        );
        assert!(layout.rotate(1));
        assert_eq!(layout.parent_dir(1), Some(Dir::Column));
        assert!(!Node::Leaf(9).rotate(9), "un panel solo no tiene división");
    }
}
