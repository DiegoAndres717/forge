// «Lanzar en paralelo»: varias fases de un plan a la vez, cada una con su agente en su propia
// terminal y su worktree (el mismo de `forge worktree new`, registrado para que la limpieza
// lo vea). Lo que se puede lanzar y el mensaje de cada agente salen de `forge_core::parallel`.
use super::*;
use forge_core::ideas::{Idea, PhaseChange};
use forge_core::parallel;
use forge_core::worktrees;
use std::cell::RefCell;
use std::collections::BTreeSet;

/// Lo elegido en «Lanzar en paralelo»: (plan, fases desde 0).
pub(super) type Pick = Option<(i64, BTreeSet<usize>)>;

impl App {
    /// Los agentes instalados y activos del proyecto: (id, nombre, admite un mensaje inicial).
    pub(super) fn launchers(&self, workspace: usize) -> Vec<(String, String, bool)> {
        let Ok(detected) = self.agents.lock().map(|d| d.clone()) else {
            return Vec::new();
        };
        let Some(ws) = self.workspaces.get(workspace) else {
            return Vec::new();
        };
        ws.project
            .agents
            .iter()
            .filter(|a| a.enabled && detected.get(a.program()).is_some_and(|d| d.path.is_some()))
            .map(|a| {
                let prompt = a.with_prompt(&a.command, "x").is_some();
                (a.id.clone(), a.name.clone(), prompt)
            })
            .collect()
    }

    /// Lanza las fases `phases` (desde 0) del plan `id` con el agente `agent`: por cada una,
    /// su worktree (registrado con quién y qué fase), una terminal en él con el mensaje de la
    /// fase, y la fase en curso con su rama. Lo que falle se dice; lo demás sigue.
    pub(super) fn launch_parallel(
        &mut self,
        ctx: &egui::Context,
        id: i64,
        phases: &[usize],
        agent: &str,
        area: Rect,
    ) {
        let Some(i) = self.active else { return };
        let path = self.workspaces[i].project.path.clone();
        let Some(idea) = self.db(|s| s.get_idea(Some(&path), id)) else {
            return; // el error de la base ya se muestra
        };
        let Some(idea) = idea else {
            self.error = Some(tr!("El plan #{id} ya no existe.", id = id));
            return;
        };
        if let Err(why) = parallel::check(&idea, phases) {
            self.error = Some(why);
            return;
        }
        let ws = &self.workspaces[i];
        let Some(spec) = ws.project.agents.iter().find(|a| a.id == agent).cloned() else {
            self.error = Some(tr!(
                "No encuentro el agente {id} en este proyecto.",
                id = agent
            ));
            return;
        };
        // Antes de crear nada: un agente sin mensaje inicial no recibiría la fase.
        if spec.with_prompt(&spec.command, "x").is_none() {
            self.error = Some(crate::workspace::no_prompt(&spec.name));
            return;
        }
        let Some(root) = self.worktrees.root.clone().or_else(worktrees::root) else {
            self.error = Some(tr!("no se encontró $HOME").into());
            return;
        };
        let checks: Vec<String> = Rules::load(&ws.project.root())
            .ok()
            .flatten()
            .map(|r| {
                r.checks
                    .iter()
                    .map(|c| format!("{}: `{}`", c.name.as_deref().unwrap_or(&c.id), c.command))
                    .collect()
            })
            .unwrap_or_default();
        let mut chosen = phases.to_vec();
        chosen.sort_unstable();
        chosen.dedup();
        let mut failed = Vec::new();
        // Las que no llegaron a abrirse quedan elegidas para reintentarlas.
        let mut retry = BTreeSet::new();
        for &n in &chosen {
            let Some(branch) = idea.phases[n].branch.as_deref().map(str::trim) else {
                continue; // `check` ya lo descartó
            };
            let ws = &mut self.workspaces[i];
            let who = worktrees::Record {
                origin: Some(spec.name.clone()),
                // La terminal que se abre ahora: al cerrarla, la limpieza lo propone.
                session: Some(ws.next_session_id()),
                phase: Some(parallel::record_label(&idea, n)),
                ..Default::default()
            };
            let launched = worktrees::create(&path, branch, None, &root, who).and_then(|wt| {
                let brief = parallel::brief(&idea, n, &wt, &chosen, &checks)
                    .ok_or_else(|| tr!("la fase no tiene rama").to_string())?;
                let name = tr!("{agent} · fase {n}", agent = spec.name, n = n + 1);
                ws.launch_agent_in(ctx, &spec.id, &brief, wt, name, area)
            });
            if let Err(e) = launched {
                failed.push(tr!("Fase {n} ({b}): {e}", n = n + 1, b = branch, e = e));
                retry.insert(n);
                continue;
            }
            let change = PhaseChange {
                status: Some("doing"),
                branch: Some(branch),
                ..Default::default()
            };
            // El agente ya trabaja: si no se pudo marcar, se dice (y no se ofrece relanzarla).
            if self
                .db(|s| s.update_phase(Some(&path), id, n, change, &spec.id))
                .is_none()
            {
                let e = self.error.take().unwrap_or_default();
                failed.push(tr!(
                    "Fase {n}: su agente ya trabaja, pero no se pudo marcar en curso: {e}",
                    n = n + 1,
                    e = e
                ));
            }
        }
        let view = &mut self.workspaces[i].ideas;
        view.dirty = true;
        view.parallel = (!retry.is_empty()).then_some((id, retry));
        if failed.is_empty() {
            view.open = false; // que se vean los agentes trabajando
        } else {
            self.error = Some(failed.join("\n"));
        }
    }
}

/// «Lanzar fases en paralelo» en un plan con al menos dos fases sin empezar y, abierto, la
/// elección de fases, los avisos y un botón por agente.
pub(super) fn parallel_box(
    ui: &mut egui::Ui,
    idea: &Idea,
    pick: &RefCell<Pick>,
    launchers: &[(String, String, bool)],
    cmds: &mut Vec<UiCmd>,
) {
    let pending = idea.phases.iter().filter(|p| p.status == "pending").count();
    if pending < 2 {
        return;
    }
    let mut pick = pick.borrow_mut();
    let open = pick.as_ref().is_some_and(|(plan, _)| *plan == idea.id);
    let label = format!("{}  {}", icon::GIT_FORK, tr!("Lanzar fases en paralelo"));
    let toggle = ui
        .add(egui::Button::new(RichText::new(label).size(12.0).color(theme::ACCENT)).frame(false))
        .on_hover_text(tr!(
            "Varios agentes a la vez: una terminal por fase, cada una en su propio worktree y su rama"
        ));
    if toggle.clicked() {
        *pick = if open {
            None
        } else {
            // De entrada, las que se pueden lanzar.
            let ready = idea
                .phases
                .iter()
                .enumerate()
                .filter(|(n, p)| parallel::refusal(p, *n).is_none())
                .map(|(n, _)| n)
                .collect();
            Some((idea.id, ready))
        };
    }
    let Some((_, chosen)) = pick.as_mut().filter(|(plan, _)| *plan == idea.id) else {
        return;
    };
    let small = |ui: &mut egui::Ui, text: &str, color| {
        ui.add(egui::Label::new(RichText::new(text).size(11.5).color(color)).wrap());
    };
    small(
        ui,
        tr!(
            "Elige las fases: cada una se abre con su agente en una terminal propia, dentro de su worktree."
        ),
        theme::TEXT_3,
    );
    for (n, phase) in idea.phases.iter().enumerate() {
        if phase.status == "done" {
            continue;
        }
        let refusal = parallel::refusal(phase, n);
        let mut on = chosen.contains(&n) && refusal.is_none();
        let text = match &phase.branch {
            Some(b) => format!("{}. {} · {b}", n + 1, phase.title),
            None => format!("{}. {}", n + 1, phase.title),
        };
        let check = ui.add_enabled(refusal.is_none(), egui::Checkbox::new(&mut on, text));
        if check.changed() {
            if on {
                chosen.insert(n);
            } else {
                chosen.remove(&n);
            }
        }
        if let Some(why) = refusal {
            small(ui, &why, theme::TEXT_4);
        }
    }
    let phases: Vec<usize> = chosen.iter().copied().collect();
    let ready = match parallel::check(idea, &phases) {
        Ok(warnings) => {
            for w in warnings {
                small(
                    ui,
                    &format!("{} {}", icon::WARNING, w.text()),
                    theme::YELLOW,
                );
            }
            true
        }
        Err(why) => {
            small(ui, &why, theme::TEXT_3);
            false
        }
    };
    let (can, cannot): (Vec<_>, Vec<_>) = launchers.iter().partition(|(_, _, prompt)| *prompt);
    let mut cancel = false;
    ui.horizontal_wrapped(|ui| {
        for (id, name, _) in &can {
            let label = format!(
                "{}  {}",
                icon::PLAY,
                tr!("Lanzar con {agent}", agent = name)
            );
            if ui.add_enabled(ready, egui::Button::new(label)).clicked() {
                cmds.push(UiCmd::LaunchParallel(idea.id, phases.clone(), id.clone()));
            }
        }
        if ui.button(tr!("Cancelar")).clicked() {
            cancel = true;
        }
    });
    if can.is_empty() {
        small(
            ui,
            tr!(
                "No encuentro ningún agente instalado y activo que pueda empezar con un mensaje (Claude Code, Codex, OpenCode o Antigravity)."
            ),
            theme::TEXT_3,
        );
    }
    for (_, name, _) in cannot {
        small(ui, &crate::workspace::no_prompt(name), theme::TEXT_4);
    }
    if cancel {
        *pick = None;
    }
}
