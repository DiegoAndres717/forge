// Tests de extremo a extremo de la interfaz: la app real (shells, Git, SQLite en memoria)
// manejada como lo haría el usuario, con atajos de teclado y clics sobre los controles
// por su nombre accesible. Sin GPU: corren en CI.
use super::*;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

/// Los tests e2e abren shells de login reales (con el perfil del usuario): de uno en uno,
/// para que la suite no dependa de cuánto aguanta la máquina con todos a la vez.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ⌘ y ⌘⇧ en macOS; Ctrl+Shift y Ctrl+Alt en Linux y Windows (ver `command_mods`).
#[cfg(target_os = "macos")]
pub(super) const CMD: Modifiers = Modifiers {
    alt: false,
    ctrl: false,
    shift: false,
    mac_cmd: true,
    command: true,
};
#[cfg(target_os = "macos")]
const CMD_SHIFT: Modifiers = Modifiers { shift: true, ..CMD };
#[cfg(not(target_os = "macos"))]
pub(super) const CMD: Modifiers = Modifiers {
    alt: false,
    ctrl: true,
    shift: true,
    mac_cmd: false,
    command: true,
};
#[cfg(not(target_os = "macos"))]
const CMD_SHIFT: Modifiers = Modifiers {
    alt: true,
    shift: false,
    ..CMD
};

/// Repositorio Git temporal con un archivo preparado y un check que pasa.
fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("forge-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".forge")).unwrap();
    let dir = forge_core::canonical(&dir);
    std::fs::write(
        dir.join(".forge/rules.toml"),
        "[commit]\nrequire_tests = true\n[[checks]]\nid = \"tests\"\nname = \"Tests\"\ncommand = \"true\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("main.ts"), "export const x = 1;\n").unwrap();
    let ok = std::process::Command::new("sh")
        .args([
            "-c",
            "git init -q -b main && git config user.email t@t && git config user.name t && git add -A",
        ])
        .current_dir(&dir)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    dir
}

fn app(project: PathBuf) -> Harness<'static, App> {
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_eframe(move |cc| {
            crate::install_fonts(&cc.egui_ctx, &Settings::default()).unwrap();
            App::with_store(
                &cc.egui_ctx,
                Settings::default(),
                None,
                Some(project),
                Store::in_memory(),
            )
        });
    harness.run_steps(3);
    harness
}

/// Avanza fotogramas hasta que se cumpla la condición (los shells y checks son reales).
fn wait_until(harness: &mut Harness<'_, App>, what: &str, done: impl Fn(&App) -> bool) {
    let start = Instant::now();
    while !done(harness.state()) {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "tiempo agotado: {what}"
        );
        harness.run_steps(2);
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn ws(app: &App) -> &Workspace {
    &app.workspaces[0]
}

#[test]
fn split_and_close_panels_with_shortcuts() {
    let _serial = serial();
    let mut h = app(project("split"));
    assert_eq!(ws(h.state()).panel_count(), 1);

    h.key_press_modifiers(CMD, Key::D);
    h.run_steps(2);
    assert_eq!(ws(h.state()).panel_count(), 2, "⌘D divide a la derecha");

    h.key_press_modifiers(CMD_SHIFT, Key::D);
    h.run_steps(2);
    assert_eq!(ws(h.state()).panel_count(), 3, "⌘⇧D divide hacia abajo");

    h.key_press_modifiers(CMD, Key::W);
    h.run_steps(2);
    assert_eq!(
        ws(h.state()).panel_count(),
        2,
        "⌘W cierra el panel enfocado"
    );
}

/// Plan #42, fase 4: «Revisar cambios» (panel de Git y paleta) lanza Guard con la revisión
/// con IA aunque rules.toml no la pida.
#[test]
fn review_changes_runs_guard_with_the_ai_review() {
    let _serial = serial();
    let dir = project("review-changes");
    // Un proveedor que falla al momento: la prueba nunca llama (ni paga) a un modelo real.
    let fake = "[routing]\nprefer_local = false\n[routing.providers.nada]\ncommand = \"exit 1\"\n";
    let tasks = ["classification", "code_review", "architecture", "security"]
        .map(|t| format!("[routing.tasks.{t}]\nprovider = \"nada\"\nmodel = \"m\"\n"))
        .concat();
    std::fs::write(dir.join(".forge/routing.toml"), format!("{fake}{tasks}")).unwrap();
    let mut h = app(dir);
    // Los ítems llegan en segundo plano, al leer el diff.
    fn has_ai_in(a: &App) -> bool {
        ws(a)
            .guard
            .run
            .as_ref()
            .is_some_and(|r| r.snapshot(|r| r.items.iter().any(|i| i.id == "ai-review")))
    }
    palette(&mut h, "revisar cambios");
    h.run_steps(2);
    assert!(ws(h.state()).guard.open, "lo muestra en el panel de Guard");
    wait_until(&mut h, "revisión con IA", has_ai_in);
    assert_eq!(ws(h.state()).guard.stage, guard::Stage::Commit);
    // El botón del panel de Git hace lo mismo.
    if let Some(run) = &ws(h.state()).guard.run {
        run.cancel();
    }
    h.state_mut().workspaces[0].guard.run = None;
    // Guard se dibuja en el lugar del panel de Git: se cierra. project() deja cambios preparados.
    h.state_mut().workspaces[0].guard.open = false;
    h.key_press_modifiers(CMD_SHIFT, Key::G);
    h.run_steps(2);
    wait_until(&mut h, "estado de git", |a| ws(a).git.data.is_some());
    h.run_steps(2);
    h.get_by_label_contains("Revisar cambios").click();
    h.run_steps(2);
    wait_until(&mut h, "revisión con IA", has_ai_in);
    if let Some(run) = &ws(h.state()).guard.run {
        run.cancel();
    }
}

#[test]
fn guard_validates_the_commit_from_the_panel() {
    let _serial = serial();
    let mut h = app(project("guard"));
    h.key_press_modifiers(CMD, Key::G);
    h.run_steps(2);
    assert!(ws(h.state()).guard.open, "⌘G abre Guard");

    h.get_by_label_contains("Ejecutar").click();
    h.run_steps(2);
    wait_until(&mut h, "Guard termina", |a| {
        ws(a)
            .guard
            .run
            .as_ref()
            .is_some_and(|r| r.snapshot(|r| r.finished()))
    });
    h.run_steps(2);
    let verdict = ws(h.state())
        .guard
        .run
        .as_ref()
        .unwrap()
        .snapshot(|r| r.verdict());
    assert_eq!(verdict, guard::Verdict::Ready);
    h.get_by_label_contains("Listo para commit");
    h.get_by_label_contains("Crear commit");
}

#[test]
fn memory_note_created_from_the_panel_is_searchable() {
    let _serial = serial();
    let dir = project("memory");
    let mut h = app(dir.clone());
    h.key_press_modifiers(CMD_SHIFT, Key::M);
    h.run_steps(2);
    assert!(ws(h.state()).memory.open, "⌘⇧M abre la memoria");

    h.get_by_label_contains("Nueva nota").click();
    h.run_steps(2);
    // Campo de texto por su texto de ayuda (placeholder).
    let title = h.get_by(|n| n.placeholder() == Some("Título"));
    title.focus();
    title.type_text("Vacunación por lotes");
    let tags = h.get_by(|n| n.placeholder() == Some("Etiquetas, separadas por comas"));
    tags.focus();
    tags.type_text("vacunas");
    h.run_steps(2);
    h.get_by_label("Guardar").click();
    h.run_steps(3);

    let found = h
        .state()
        .store
        .as_ref()
        .unwrap()
        .search_memories(&dir, "vacunacion", None, 10)
        .unwrap();
    assert_eq!(found.len(), 1, "la búsqueda ignora tildes");
    assert_eq!(found[0].title, "Vacunación por lotes");
    h.get_by_label_contains("Vacunación por lotes");

    // Las etiquetas existentes salen como botones: un clic filtra, otro lo quita.
    h.get_by_label_contains("#vacunas").click();
    h.run_steps(3);
    assert_eq!(
        ws(h.state()).memory.results.len(),
        1,
        "el filtro de etiqueta se combina con la búsqueda"
    );
}

#[test]
fn guard_blocks_a_commit_with_a_secret() {
    let _serial = serial();
    let dir = project("secret");
    std::fs::write(
        dir.join("config.ts"),
        "export const key = \"AKIAIOSFODNN7EXAMPLE\";\n",
    )
    .unwrap();
    std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(&dir)
        .status()
        .unwrap();
    let mut h = app(dir);
    h.key_press_modifiers(CMD, Key::G);
    h.run_steps(2);
    h.get_by_label_contains("Ejecutar").click();
    wait_until(&mut h, "Guard termina", |a| {
        ws(a)
            .guard
            .run
            .as_ref()
            .is_some_and(|r| r.snapshot(|r| r.finished()))
    });
    h.run_steps(2);
    let verdict = ws(h.state())
        .guard
        .run
        .as_ref()
        .unwrap()
        .snapshot(|r| r.verdict());
    assert_eq!(verdict, guard::Verdict::Blocked);
    // Se ofrece omitir con motivo y no se ofrece crear el commit.
    h.get_by_label_contains("Omitir con motivo");
    assert!(h.query_by_label_contains("Crear commit").is_none());
}

/// Abre la paleta con ⌘K, escribe la consulta y elige el primer resultado con Enter.
fn palette(h: &mut Harness<'_, App>, query: &str) {
    h.key_press_modifiers(CMD, Key::K);
    h.run_steps(2);
    assert!(h.state().palette.is_some(), "⌘K abre la paleta");
    h.get_by(|n| {
        n.placeholder()
            .is_some_and(|p| p.starts_with("Buscar acciones"))
    })
    .type_text(query);
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(
        h.state().palette.is_none(),
        "Enter ejecuta y cierra la paleta"
    );
}

#[test]
fn command_palette_runs_actions_by_fuzzy_name() {
    let _serial = serial();
    let mut h = app(project("palette"));
    // "divder" → "Dividir a la derecha".
    palette(&mut h, "divder");
    assert_eq!(ws(h.state()).panel_count(), 2);

    palette(&mut h, "nueva nota");
    let ws = ws(h.state());
    assert!(
        ws.memory.open && ws.memory.form.is_some(),
        "abre la memoria con el formulario"
    );

    // Escape cierra sin ejecutar nada.
    h.key_press_modifiers(CMD, Key::K);
    h.run_steps(2);
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().palette.is_none());
}

#[test]
fn pull_request_draft_after_guard_passes() {
    let _serial = serial();
    let dir = project("pr");
    // Rama con un commit nuevo frente a main (repositorio local, sin remoto: nada se publica).
    let ok = std::process::Command::new("sh")
        .args([
            "-c",
            "git commit -qm inicio && git switch -qc feat/vacunas \
             && echo 'export const y = 2;' > y.ts && git add -A && git commit -qm 'Registrar vacunas por lote'",
        ])
        .current_dir(&dir)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let mut h = app(dir.clone());
    palette(&mut h, "validar pull request");
    wait_until(&mut h, "Guard termina", |a| {
        ws(a)
            .guard
            .run
            .as_ref()
            .is_some_and(|r| r.snapshot(|r| r.finished()))
    });
    h.run_steps(2);
    assert_eq!(ws(h.state()).guard.stage, Stage::PullRequest);

    h.get_by_label_contains("Crear PR").click();
    h.run_steps(3);
    let pr = ws(h.state()).guard.pr.clone().expect("formulario del PR");
    assert_eq!(pr.base, "main");
    assert_eq!(pr.title, "Registrar vacunas por lote");
    assert!(pr.body.contains("## Validación"));

    let panels = ws(h.state()).panel_count();
    h.get_by_label("Crear en GitHub").click();
    h.run_steps(3);
    assert!(ws(h.state()).guard.pr.is_none());
    assert_eq!(
        ws(h.state()).panel_count(),
        panels + 1,
        "gh corre en un panel nuevo"
    );
    assert!(dir.join(".git/FORGE_PR_BODY.md").exists());
}

#[test]
fn dangerous_command_dialog_answers_the_terminal() {
    let _serial = serial();
    let dir = project("approval");
    let approvals = dir.join(".approvals");
    std::fs::create_dir_all(&approvals).unwrap();
    let mut h = app(dir.clone());
    let ctx = h.ctx.clone();
    h.state_mut().watch_approvals(&ctx, approvals.clone());
    // Lo que deja un `git push --force` lanzado por un agente desde una terminal de Forge.
    let request = forge_core::danger::Request {
        id: "1-1".into(),
        command: "git push --force".into(),
        reason: "reescribe la historia del remoto (git push --force)".into(),
        cwd: dir.clone(),
        project: Some(dir.clone()),
        origin: Some("Claude Code".into()),
        pid: std::process::id(),
    };
    std::fs::write(
        approvals.join("1-1.json"),
        serde_json::to_vec(&request).unwrap(),
    )
    .unwrap();
    wait_until(&mut h, "llega la solicitud", |a| {
        a.approvals.lock().is_ok_and(|p| !p.is_empty())
    });
    h.run_steps(3);
    h.get_by_label_contains("Claude Code quiere ejecutar");
    h.get_by_label("git push --force");
    h.get_by_label("Denegar").click();
    h.run_steps(3);
    assert!(
        approvals.join("1-1.deny").exists(),
        "la terminal recibe el rechazo"
    );
    assert!(
        h.query_by_label("Denegar").is_none(),
        "el diálogo se cierra"
    );
}

#[test]
fn plans_backlog_phases_and_tasks_from_the_panel() {
    let _serial = serial();
    let dir = project("ideas");
    let mut h = app(dir.clone());
    h.key_press_modifiers(CMD_SHIFT, Key::I);
    h.run_steps(2);
    assert!(ws(h.state()).ideas.open, "⌘⇧I abre Planes");

    // Una idea al backlog desde el campo rápido.
    let input = h.get_by(|n| n.placeholder().is_some_and(|p| p.starts_with("Nueva idea")));
    input.focus();
    input.type_text("Exportar vacunas a Excel");
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(3);
    let store = |h: &Harness<'_, App>, p: Option<&Path>| {
        h.state()
            .store
            .as_ref()
            .unwrap()
            .list_ideas(p, true)
            .unwrap()
    };
    let ideas = store(&h, Some(&dir));
    assert_eq!(ideas[0].title, "Exportar vacunas a Excel");
    assert_eq!(ideas[0].source, "usuario");

    // Un agente crea un plan por MCP: aparece solo, con su avance.
    let mut model = forge_core::ideas::Phase::new("Modelo");
    model.branch = Some("feat/lotes".into());
    model.tasks = vec![forge_core::ideas::Task {
        text: "Tabla lotes".into(),
        done: false,
    }];
    let plan = h
        .state()
        .store
        .as_ref()
        .unwrap()
        .add_plan(
            Some(&dir),
            "Vacunas por lote",
            "",
            &[model, forge_core::ideas::Phase::new("Pantalla")],
            "claude-code",
        )
        .unwrap();
    wait_until(&mut h, "el plan del agente aparece", |a| {
        ws(a).ideas.items.len() == 2
    });
    h.run_steps(2);
    h.get_by_label_contains("Fase 1 de 2 · Modelo");
    let get = |h: &Harness<'_, App>| {
        store(h, Some(&dir))
            .into_iter()
            .find(|i| i.id == plan)
            .unwrap()
    };

    // Un plan que crea un agente se abre desplegado y resaltado: se empieza la fase 1 y
    // se tacha la tarea.
    assert_eq!(ws(h.state()).ideas.highlighted, Some(plan));
    h.get_by_label_contains("pasar a En progreso — Modelo")
        .click();
    h.run_steps(3);
    assert_eq!(
        get(&h).status,
        "doing",
        "empezar una fase pone el plan en progreso"
    );
    h.get_by_label("Tachar: Tabla lotes").click();
    h.run_steps(3);
    assert!(get(&h).phases[0].tasks[0].done);
    h.get_by_label_contains("feat/lotes"); // chip de la rama

    // Añadir una fase a mano.
    h.get_by_label_contains("Añadir fase").click();
    h.run_steps(2);
    let field = h.get_by(|n| n.placeholder().is_some_and(|p| p.starts_with("Nueva fase")));
    field.type_text("Tests");
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(get(&h).phases.len(), 3);

    // Completar la idea del backlog: pasa a la sección Completado.
    h.get_by_label("Marcar como completada: Exportar vacunas a Excel")
        .click();
    h.run_steps(3);
    assert!(
        store(&h, Some(&dir))
            .iter()
            .any(|i| i.title == "Exportar vacunas a Excel" && i.done())
    );
    h.get_by_label_contains("COMPLETADO");

    // Lista general en Inicio.
    h.key_press_modifiers(CMD_SHIFT, Key::H);
    h.run_steps(3);
    let input = h.get_by(|n| n.placeholder().is_some_and(|p| p.starts_with("Nueva idea")));
    input.focus();
    input.type_text("App para talleres");
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(store(&h, None)[0].title, "App para talleres");
}

#[test]
fn mcp_plan_opens_and_highlights_plans() {
    let _serial = serial();
    let dir = project("mcp-plan");
    let mut h = app(dir.clone());
    let plan = h
        .state()
        .store
        .as_ref()
        .unwrap()
        .add_plan(
            Some(&dir),
            "Exportación fiable",
            "",
            &[forge_core::ideas::Phase::new("Modelo")],
            "claude-code",
        )
        .unwrap();
    h.state_mut().workspaces[0].ideas.dirty = true;
    h.run_steps(3);
    let ideas = &ws(h.state()).ideas;
    assert!(ideas.open, "un plan del MCP abre Planes");
    assert_eq!(ideas.highlighted, Some(plan));
    assert_eq!(ideas.toggled.get(&plan), Some(&true));
}

fn app_with(store: Store, open: Option<PathBuf>) -> Harness<'static, App> {
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_eframe(move |cc| {
            crate::install_fonts(&cc.egui_ctx, &Settings::default()).unwrap();
            App::with_store(&cc.egui_ctx, Settings::default(), None, open, Ok(store))
        });
    harness.run_steps(3);
    harness
}

#[test]
fn only_the_active_project_starts_and_the_rest_sleep() {
    let _serial = serial();
    let (a, b) = (project("sleep-a"), project("sleep-b"));
    // Dos proyectos abiertos al cerrar Forge la última vez; el activo era A.
    let store = Store::in_memory().unwrap();
    for p in [&a, &b] {
        store
            .touch(p, p.file_name().unwrap().to_str().unwrap())
            .unwrap();
    }
    store.set_setting("active", &a.to_string_lossy()).unwrap();
    let mut h = app_with(store, None);
    h.run_steps(3);
    let state = h.state();
    assert_eq!(state.workspaces.len(), 2);
    assert!(!state.workspaces[0].is_dormant() && state.workspaces[0].panel_count() == 1);
    assert!(
        state.workspaces[1].is_dormant(),
        "B no arranca hasta entrar"
    );
    assert_eq!(state.workspaces[1].panel_count(), 0);

    // ⌘2 entra en B y lo despierta.
    h.key_press_modifiers(CMD, Key::Num2);
    h.run_steps(3);
    assert_eq!(h.state().active, Some(1));
    assert!(!h.state().workspaces[1].is_dormant());
    assert_eq!(h.state().workspaces[1].panel_count(), 1);

    // ⌃Tab vuelve al anterior.
    h.key_press_modifiers(Modifiers::CTRL, Key::Tab);
    h.run_steps(3);
    assert_eq!(h.state().active, Some(0));
    // Repetido (soltando Ctrl entre medias) alterna siempre, sin pulsaciones perdidas.
    for expected in [1, 0, 1, 0] {
        h.key_press_modifiers(Modifiers::CTRL, Key::Tab);
        h.run_steps(2);
        assert_eq!(h.state().active, Some(expected));
    }

    // Dormir A desde la paleta: cierra sus terminales y conserva el layout.
    palette(&mut h, "dormir proyecto");
    assert!(h.state().workspaces[0].is_dormant());
    assert_eq!(h.state().active, None);
    assert!(h.state().workspaces[0].saved_state().is_some());
}

#[test]
fn background_process_failure_lights_the_project() {
    let _serial = serial();
    let dir = project("attention");
    std::fs::write(
        dir.join(".forge/project.toml"),
        "[[processes]]\nid = \"api\"\nname = \"API\"\ncommand = \"while [ ! -f fail-now ]; do sleep 0.1; done; exit 3\"\nrestart = \"on-workspace-open\"\n",
    )
    .unwrap();
    let mut h = app(dir.clone());
    assert_eq!(ws(h.state()).attention(), None);
    h.key_press_modifiers(CMD_SHIFT, Key::H); // se sale del proyecto antes de que falle
    h.run_steps(2);
    // Falla solo cuando ya no se está viendo (sin depender de lo que tarde el shell).
    std::fs::write(dir.join("fail-now"), "").unwrap();
    wait_until(&mut h, "aviso del proceso fallido", |a| {
        ws(a).attention().is_some()
    });
    assert_eq!(
        ws(h.state()).attention(),
        Some(crate::workspace::Attention::Failed("API".into()))
    );
    // Al entrar se apaga.
    h.key_press_modifiers(CMD, Key::Num1);
    h.run_steps(3);
    assert_eq!(ws(h.state()).attention(), None);
}

#[test]
#[cfg_attr(
    windows,
    ignore = "its agent uses `seq`, which PowerShell doesn't have"
)]
fn agent_that_finishes_in_the_background_lights_the_project() {
    let _serial = serial();
    let dir = project("agent");
    // Agente de prueba: trabaja un momento (escribe) y se queda esperando.
    std::fs::write(
        dir.join(".forge/agents.toml"),
        "[[agents]]\nid = \"fake\"\nname = \"Agente de prueba\"\ncommand = \"sleep 1; seq 1 2000; sleep 60\"\n",
    )
    .unwrap();
    let mut h = app(dir);
    let ctx = h.ctx.clone();
    h.state_mut().apply(
        &ctx,
        UiCmd::OpenAgent("fake".into(), false),
        Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1000.0, 700.0)),
    );
    h.run_steps(2);
    h.key_press_modifiers(CMD_SHIFT, Key::H);
    h.run_steps(2);
    wait_until(&mut h, "aviso del agente", |a| ws(a).attention().is_some());
    assert_eq!(
        ws(h.state()).attention(),
        Some(crate::workspace::Attention::Agent(
            "Agente de prueba".into()
        ))
    );
}

#[test]
fn cmd_f_opens_terminal_search_and_esc_closes_it() {
    let _serial = serial();
    let mut h = app(project("find"));
    let search = |h: &Harness<'_, App>| {
        h.query_by(|n| n.placeholder() == Some("Buscar en la terminal"))
            .is_some()
    };
    assert!(!search(&h));
    h.key_press_modifiers(CMD, Key::F);
    h.run_steps(3);
    assert!(search(&h), "⌘F abre la búsqueda");
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(!search(&h), "Esc la cierra");
}

#[test]
fn cmd_comma_opens_settings() {
    let _serial = serial();
    let mut h = app(project("settings"));
    h.key_press_modifiers(CMD, Key::Comma);
    h.run_steps(3);
    assert!(h.state().settings_open, "⌘, abre Ajustes");
    h.get_by_label("Tamaño de letra");
    let gear_selected = |h: &Harness<'_, App>| {
        use egui_kittest::kittest::NodeT;
        h.get_by_label(forge_core::tr!("Ajustes (⌘,)"))
            .accesskit_node()
            .toggled()
            == Some(egui::accesskit::Toggled::True)
    };
    assert!(gear_selected(&h), "el botón ⚙ se ve seleccionado");
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(!h.state().settings_open, "Esc la cierra");
    assert!(!gear_selected(&h));
}

/// Sin barra de menús nativa (Linux, Windows): el botón ☰ junto a la barra lateral abre los
/// mismos menús que la barra de menús de macOS.
#[test]
fn the_menu_button_opens_the_app_menus() {
    let _serial = serial();
    let mut h = app(project("menu"));
    // Fuera de macOS ya viene encendido; aquí se enciende para probarlo en cualquier sistema.
    h.state_mut().menu_button = true;
    h.run_steps(2);
    let open = h.state().sidebar;
    h.get_by_label("Menú").click();
    h.run_steps(3);
    // Los submenús llevan la flecha ⏵ en su nombre.
    h.get_by_label_contains("Ver").click();
    h.run_steps(3);
    h.get_by_label_contains("Barra lateral").click();
    h.run_steps(3);
    assert_ne!(
        h.state().sidebar,
        open,
        "Ver → Barra lateral la muestra u oculta"
    );
}

#[test]
fn clickable_controls_show_the_pointing_hand() {
    let _serial = serial();
    let mut h = app(project("cursor"));
    h.key_press_modifiers(CMD, Key::Comma);
    h.run_steps(3);
    for label in ["Buscar actualizaciones", "Español"] {
        h.get_by_label_contains(label).hover();
        h.run_steps(2);
        assert_eq!(
            h.output().platform_output.cursor_icon,
            egui::CursorIcon::PointingHand,
            "{label}"
        );
    }
}

/// Ajustes por pestañas: cada opción con su ⓘ, y la pestaña elegida muestra lo suyo.
#[test]
fn settings_tabs_show_their_options_with_help() {
    let _serial = serial();
    let mut h = app(project("settings-tabs"));
    h.key_press_modifiers(CMD, Key::Comma);
    h.run_steps(3);
    assert!(
        h.query_by_label_contains("Idioma de Forge").is_some(),
        "ⓘ del idioma"
    );
    assert!(
        h.query_by_label_contains("Qué se exige en cada paso")
            .is_none()
    );
    h.get_by_label_contains("Este proyecto").click();
    h.run_steps(3);
    assert!(
        h.query_by_label_contains("Qué se exige en cada paso")
            .is_some(),
        "formulario de Guard"
    );
    assert!(h.query_by_label_contains("Idioma").is_none());
}

/// Ajustes → Este proyecto → Revisión con IA: profundidad, lentes y refutador (plan #42).
#[test]
fn review_depth_lenses_and_refuter_are_set_from_settings() {
    let _serial = serial();
    let mut h = app(project("settings-review"));
    h.key_press_modifiers(CMD, Key::Comma);
    h.run_steps(3);
    h.get_by_label_contains("Este proyecto").click();
    h.run_steps(3);
    let form = |h: &Harness<'_, App>| h.state().routing_form.as_ref().unwrap().form.clone();
    assert_eq!(
        form(&h).review_depth,
        forge_core::reviewers::DepthMode::Risk
    );
    for label in [
        "Siempre completa",
        "Legibilidad",
        "Refutar antes de bloquear",
    ] {
        // Está abajo en la pestaña: primero se desplaza hasta él.
        h.get_by_label(label).scroll_to_me();
        h.run_steps(10);
        h.get_by_label(label).click();
        h.run_steps(2);
    }
    let f = form(&h);
    assert_eq!(f.review_depth, forge_core::reviewers::DepthMode::Full);
    assert_eq!(
        f.lenses,
        ["reliability", "resilience"],
        "legibilidad quitada"
    );
    assert!(!f.refuter);
}

/// Forge Jev se enciende en Ajustes → Agentes, apagado de serie, y llega a los agentes que
/// se abran después (FORGE_JEV_CHORES / --jev-chores).
#[test]
fn forge_jev_is_off_by_default_and_turns_on_from_settings() {
    let _serial = serial();
    let dir = project("forge-jev");
    let mut h = app(dir.clone());
    assert!(!h.state().modes(&dir).jev_chores);
    h.key_press_modifiers(CMD, Key::Comma);
    h.run_steps(3);
    h.get_by_label_contains("Agentes").click();
    h.run_steps(3);
    assert!(
        h.query_by_label_contains("Tareas de git a un subagente barato")
            .is_none(),
        "la parte se ve solo con Forge Jev encendido"
    );
    h.get_by_label("Elegir el modelo más barato que sirva")
        .click();
    h.run_steps(3);
    assert!(h.state().modes(&dir).jev_chores);
    let jev_launch = |h: &Harness<'_, App>| ws(h.state()).jev_launch;
    assert!(jev_launch(&h), "el modelo al abrir viene con Forge Jev");
    // Apagar una parte la quita aunque Forge Jev siga encendido.
    h.get_by_label_contains("Tareas de git a un subagente barato")
        .click();
    h.run_steps(3);
    assert!(!h.state().modes(&dir).jev_chores);
    h.get_by_label_contains("Modelo según el pedido al abrir Claude")
        .click();
    h.run_steps(3);
    assert!(!jev_launch(&h));
}

#[test]
fn an_agent_that_only_redraws_does_not_raise_an_alert() {
    let _serial = serial();
    let dir = project("redraw");
    // Espera y luego redibuja un poco (como la barra de estado de Claude mientras espera).
    std::fs::write(
        dir.join(".forge/agents.toml"),
        "[[agents]]\nid = \"fake\"\nname = \"Agente de prueba\"\ncommand = \"sleep 5; printf 'x'; sleep 60\"\n",
    )
    .unwrap();
    let mut h = app(dir);
    let ctx = h.ctx.clone();
    let area = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1000.0, 700.0));
    h.state_mut()
        .apply(&ctx, UiCmd::OpenAgent("fake".into(), false), area);
    // Con el proyecto a la vista hasta que el shell termina de arrancar (su salida de
    // inicio, p. ej. fastfetch, no es trabajo del agente).
    wait_until(&mut h, "el shell arranca", |a| {
        ws(a).is_quiet(Duration::from_millis(1500))
    });
    h.key_press_modifiers(CMD_SHIFT, Key::H);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(8) {
        h.run_steps(2);
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(ws(h.state()).attention(), None);
}

#[test]
fn git_panel_stages_unstages_and_commits() {
    let _serial = serial();
    let mut h = app(project("git"));
    h.key_press_modifiers(CMD_SHIFT, Key::G);
    h.run_steps(2);
    assert!(ws(h.state()).git.open, "⌘⇧G abre Git");
    wait_until(&mut h, "estado de git", |a| ws(a).git.data.is_some());
    h.run_steps(2);
    // project() deja rules.toml y main.ts preparados.
    h.get_by_label("PREPARADOS (2)"); // los títulos de sección van en mayúsculas
    h.get_by_label("Quitar todo").click();
    h.run_steps(2);
    wait_until(&mut h, "todo sin preparar", |a| {
        ws(a)
            .git
            .data
            .as_ref()
            .is_some_and(|(s, _, _)| s.files.iter().all(|f| !f.is_staged()))
    });
    h.run_steps(2);
    h.get_by_label("Preparar todo").click();
    h.run_steps(2);
    wait_until(&mut h, "todo preparado", |a| {
        ws(a)
            .git
            .data
            .as_ref()
            .is_some_and(|(s, _, _)| s.files.iter().all(|f| f.is_staged()))
    });
    h.run_steps(2);
    let message = h.get_by(|n| n.placeholder() == Some("Mensaje del commit"));
    message.focus();
    message.type_text("Primer commit");
    h.run_steps(2);
    let panels = ws(h.state()).panel_count();
    h.get_by_label_contains("Commit (2)").click();
    h.run_steps(3);
    assert_eq!(
        ws(h.state()).panel_count(),
        panels + 1,
        "git commit corre en un panel"
    );
    assert!(ws(h.state()).git.message.is_empty());
}

#[test]
fn welcome_tour_runs_once_and_is_remembered() {
    let _serial = serial();
    let mut h = app(project("welcome"));
    h.state_mut().welcome = Some(0); // como la primera vez en la app real
    h.run_steps(2);
    h.get_by_label("Bienvenido a Forge");
    for _ in 0..3 {
        h.get_by_label("Siguiente").click();
        h.run_steps(2);
    }
    h.get_by_label("Guard y Git");
    h.get_by_label("Empezar").click();
    h.run_steps(2);
    assert_eq!(h.state().welcome, None);
    let welcomed = h
        .state()
        .store
        .as_ref()
        .unwrap()
        .setting("welcomed")
        .unwrap();
    assert_eq!(welcomed.as_deref(), Some("1"), "no vuelve a salir");
}

#[test]
fn update_pill_shows_in_the_toolbar_and_follows_the_download() {
    let _serial = serial();
    let mut h = app(project("update"));
    assert!(h.query_by_label_contains("Actualizar Forge").is_none());
    let release = forge_core::updates::Release {
        tag: "v9.9.9".into(),
        page: "https://example.invalid".into(),
        dmg: None,
        checksum: None,
    };
    *h.state().update.lock().unwrap() = Some(super::update::Update::Available(release));
    h.run_steps(2);
    h.get_by_label_contains("Actualizar Forge");
    *h.state().update.lock().unwrap() = Some(super::update::Update::Downloading);
    h.run_steps(2);
    h.get_by_label_contains("Descargando…");
    // Búsqueda a mano: "Buscando…" y, sin novedades, "Forge está al día" unos segundos.
    *h.state().update.lock().unwrap() = Some(super::update::Update::Checking);
    h.run_steps(2);
    h.get_by_label_contains("Buscando…");
    let now = std::time::Instant::now();
    *h.state().update.lock().unwrap() = Some(super::update::Update::UpToDate(now));
    h.run_steps(2);
    h.get_by_label_contains("Forge está al día");
    let old = now - Duration::from_secs(10);
    *h.state().update.lock().unwrap() = Some(super::update::Update::UpToDate(old));
    h.run_steps(2);
    assert!(
        h.query_by_label_contains("Forge está al día").is_none(),
        "se va sola"
    );
    // Lista para reiniciar; fuera de un .app (tests) el clic no cierra nada.
    let staged = PathBuf::from("/tmp/forge-update-test/Forge.app");
    *h.state().update.lock().unwrap() = Some(super::update::Update::Ready(staged.clone()));
    h.run_steps(2);
    h.get_by_label_contains("Reiniciar para actualizar").click();
    h.run_steps(2);
    assert_eq!(
        h.state().update_state(),
        Some(super::update::Update::Ready(staged))
    );
}

/// Lo que se escribe en un campo de un panel (nota, idea, commit…) no llega a la terminal;
/// al salir del campo, o tras pulsar un botón, la terminal vuelve a recibir el teclado.
#[test]
fn typing_in_a_panel_field_does_not_reach_the_terminal() {
    let _serial = serial();
    let mut h = app(project("focus"));
    wait_until(&mut h, "shell listo", |a| {
        ws(a).is_quiet(Duration::from_millis(500))
    });
    h.key_press_modifiers(CMD_SHIFT, Key::M);
    h.run_steps(3);
    h.get_by_label_contains("Nueva nota").click();
    h.run_steps(3);
    // Como una persona: primero el clic en el campo, después escribe.
    h.get_by(|n| n.placeholder() == Some("Título")).focus();
    h.run_steps(3);
    h.get_by(|n| n.placeholder() == Some("Título"))
        .type_text("zzsolonota");
    h.run_steps(3);
    std::thread::sleep(Duration::from_millis(500));
    h.run_steps(3);
    assert!(
        !ws(h.state()).shell_text().contains("zzsolonota"),
        "el texto de la nota llegó a la terminal"
    );
    // Un clic fuera del campo (aquí un botón) lo suelta, y el botón no se queda el teclado.
    h.get_by_label_contains("Forge Memory · memoria del proyecto")
        .click();
    h.run_steps(3);
    h.event(egui::Event::Text("echo zzterm$((40+2))".into()));
    h.key_press(Key::Enter);
    wait_until(&mut h, "la terminal recibe el teclado", |a| {
        ws(a).shell_text().contains("zzterm42")
    });
    assert!(!ws(h.state()).shell_text().contains("zzsolonota"));
}

/// Seleccionar arrastrando por encima del borde desplaza la terminal hacia el historial y
/// la selección sigue creciendo; al soltar aparece "Copiar".
#[test]
#[cfg_attr(windows, ignore = "types `seq`, which PowerShell doesn't have")]
fn dragging_a_selection_past_the_edge_scrolls_and_offers_copy() {
    let _serial = serial();
    let mut h = app(project("select"));
    wait_until(&mut h, "shell listo", |a| {
        ws(a).is_quiet(Duration::from_millis(500))
    });
    h.event(egui::Event::Text("clear; seq 1 300".into()));
    h.key_press(Key::Enter);
    // "300" ya sale en el comando escrito: esperar a la salida completa y a que se calme.
    wait_until(&mut h, "salida larga", |a| {
        ws(a).shell_text().contains("\n299\n") && ws(a).is_quiet(Duration::from_millis(300))
    });
    let inside = egui::pos2(700.0, 500.0);
    let press = |pressed| egui::Event::PointerButton {
        pos: inside,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.event(egui::Event::PointerMoved(inside));
    h.event(press(true));
    h.run_steps(2);
    h.event(egui::Event::PointerMoved(inside + Vec2::new(40.0, -40.0)));
    h.run_steps(2);
    // Fuera, por encima de la terminal: se desplaza sola mientras se mantiene ahí.
    h.event(egui::Event::PointerMoved(egui::pos2(700.0, 20.0)));
    for _ in 0..20 {
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(45));
    }
    let (offset, selected) = ws(h.state()).shell_view();
    assert!(
        offset > 20,
        "no se desplazó al arrastrar fuera (offset {offset})"
    );
    let selected = selected.unwrap_or_default();
    assert!(
        selected.lines().count() > 40,
        "la selección no siguió al desplazamiento: {} líneas",
        selected.lines().count()
    );
    h.event(egui::Event::PointerButton {
        pos: egui::pos2(700.0, 20.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(3);
    h.get_by_label_contains("Copiar").click();
    h.run_steps(2);
    h.get_by_label_contains("Copiado");
}

/// Con una app que captura el ratón (Claude Code), arrastrar selecciona igual para ⌘C.
#[test]
#[cfg_attr(windows, ignore = "types `printf`, which PowerShell doesn't have")]
fn dragging_selects_even_when_the_app_captures_the_mouse() {
    let _serial = serial();
    let mut h = app(project("select-mouse"));
    wait_until(&mut h, "shell listo", |a| {
        ws(a).is_quiet(Duration::from_millis(500))
    });
    // 1000h: la app pide los clics del ratón, como hacen los programas de pantalla completa.
    h.event(egui::Event::Text(
        "clear; printf '\\e[?1000h'; seq 1 60".into(),
    ));
    h.key_press(Key::Enter);
    wait_until(&mut h, "salida", |a| {
        ws(a).shell_text().contains("\n59\n") && ws(a).is_quiet(Duration::from_millis(300))
    });
    let (from, to) = (egui::pos2(700.0, 500.0), egui::pos2(400.0, 300.0));
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.event(egui::Event::PointerMoved(from));
    h.event(button(from, true));
    h.run_steps(2);
    for k in 1..=4 {
        h.event(egui::Event::PointerMoved(
            from + (to - from) * k as f32 / 4.0,
        ));
        h.run_steps(1);
    }
    h.event(button(to, false));
    h.run_steps(3);
    let selected = ws(h.state()).shell_view().1.unwrap_or_default();
    assert!(
        selected.chars().any(|c| c.is_ascii_digit()),
        "no seleccionó: {selected:?}"
    );
    // Se copia al soltar: la app, al redibujar, borraría la selección antes del ⌘C.
    h.get_by_label_contains("Copiado");
}

/// Mientras se escribe una ruta, la terminal sugiere carpetas sin distinguir mayúsculas y
/// Tab escribe la elegida. (El zsh de Forge manda la línea en una secuencia invisible; aquí
/// la imprime el propio shell para no depender de su configuración.)
#[test]
#[cfg_attr(
    windows,
    ignore = "folder suggestions while typing come from Forge's zsh (OSC 7777)"
)]
fn typing_a_path_suggests_folders_and_tab_completes() {
    let _serial = serial();
    let dir = project("suggest");
    std::fs::create_dir_all(dir.join("Programar")).unwrap();
    std::fs::create_dir_all(dir.join("proyectos")).unwrap();
    let mut h = app(dir.clone());
    wait_until(&mut h, "shell listo", |a| {
        ws(a).is_quiet(Duration::from_millis(500))
    });
    let osc = format!("printf '\\e]7777;cd progr\\x1f{}\\a'", dir.display());
    h.event(egui::Event::Text(osc));
    h.key_press(Key::Enter);
    wait_until(&mut h, "lista de sugerencias", |a| {
        ws(a).shell_text().contains("printf") && a.workspaces[0].panel_count() > 0
    });
    for _ in 0..20 {
        h.run_steps(2);
        if h.query_by_label("Programar/").is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    h.get_by_label("Programar/");
    assert!(
        h.query_by_label("proyectos/").is_none(),
        "no empieza ni contiene «progr»"
    );
    h.key_press(Key::Tab);
    // (zsh-autosuggestions puede pintar texto fantasma detrás de lo escrito)
    wait_until(&mut h, "Tab escribe la carpeta", |a| {
        ws(a).shell_text().contains("Programar/")
    });
    assert!(
        h.query_by_label("Programar/").is_none(),
        "la lista se cierra al aceptar"
    );
    // Tab no se lleva el foco a otro control: la terminal sigue recibiendo el teclado.
    h.event(egui::Event::Text("zzsigo".into()));
    wait_until(&mut h, "la terminal sigue con el teclado", |a| {
        ws(a).shell_text().contains("zzsigo")
    });
}

/// Al dividir (⌘D) el foco pasa a la terminal nueva y es ella la que recibe el teclado
/// (la anterior suelta el foco).
#[test]
fn a_new_split_terminal_receives_the_keyboard() {
    let _serial = serial();
    let mut h = app(project("split-focus"));
    wait_until(&mut h, "shell listo", |a| {
        ws(a).is_quiet(Duration::from_millis(500))
    });
    h.key_press_modifiers(CMD, Key::D);
    wait_until(&mut h, "dos terminales listas", |a| {
        ws(a).panel_count() == 2 && ws(a).is_quiet(Duration::from_millis(500))
    });
    h.event(egui::Event::Text("echo zznueva$((40+2))".into()));
    h.key_press(Key::Enter);
    wait_until(&mut h, "la terminal nueva recibe el teclado", |a| {
        ws(a).shell_text().contains("zznueva42")
    });
}

/// Con un panel maximizado, la cabecera lista los demás: se salta a otro (también
/// maximizado) o se vuelven a mostrar todos.
#[test]
fn maximized_panel_lists_the_hidden_ones() {
    let _serial = serial();
    let mut h = app(project("maximize"));
    h.run_steps(3);
    h.key_press_modifiers(CMD, Key::D);
    h.run_steps(3);
    assert_eq!(ws(h.state()).panel_count(), 2);
    h.key_press_modifiers(CMD, Key::Enter);
    h.run_steps(3);
    let first = ws(h.state()).maximized().expect("⌘Enter maximiza");
    h.get_by_label("Paneles ocultos (1)").click();
    h.run_steps(3);
    // La otra terminal, en el desplegable: maximizarla en su lugar.
    h.get_by_label_contains(crate::theme::icon::TERMINAL_WINDOW)
        .click();
    h.run_steps(3);
    let second = ws(h.state()).maximized().expect("sigue maximizado");
    assert_ne!(first, second, "se maximizó el otro panel");
    h.get_by_label("Paneles ocultos (1)").click();
    h.run_steps(3);
    h.get_by_label_contains("Mostrar todos").click();
    h.run_steps(3);
    assert_eq!(ws(h.state()).maximized(), None, "vuelven a verse todos");
}

/// "Planificar con…": sin el agente abierto se abre con la petición como primer mensaje;
/// con él abierto, se le escribe en su panel (sin abrir otro).
#[test]
fn asking_an_agent_opens_it_with_the_prompt_or_writes_to_it() {
    let _serial = serial();
    let dir = project("ask-agent");
    // PowerShell: echo saca cada argumento en su línea; Write-Host los junta.
    let echo = if cfg!(windows) { "Write-Host" } else { "echo" };
    std::fs::write(
        dir.join(".forge/agents.toml"),
        format!(
            "[[agents]]\nid = \"fake\"\nname = \"Agente de prueba\"\ncommand = \"{echo} pedido:\"\n"
        ),
    )
    .unwrap();
    let mut h = app(dir);
    wait_until(&mut h, "shell listo", |a| {
        ws(a).is_quiet(Duration::from_millis(500))
    });
    let ctx = h.ctx.clone();
    let area = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1000.0, 700.0));
    h.state_mut().workspaces[0].ask_agent(&ctx, "fake", "planifica la idea 7", area);
    wait_until(&mut h, "se abre el agente con la petición", |a| {
        ws(a).panel_count() == 2 && ws(a).shell_text().contains("pedido: planifica la idea 7")
    });
    let second = if cfg!(windows) {
        "Write-Host zzsegunda$(40+2)"
    } else {
        "echo zzsegunda$((40+2))"
    };
    h.state_mut().workspaces[0].ask_agent(&ctx, "fake", second, area);
    wait_until(&mut h, "se escribe en el panel abierto", |a| {
        ws(a).shell_text().contains("zzsegunda42")
    });
    assert_eq!(ws(h.state()).panel_count(), 2, "no abre otro panel");
}

/// Una división se gira desde la cabecera del panel: lado a lado ↔ uno encima del otro.
#[test]
fn a_split_can_be_rotated_from_the_panel_header() {
    use crate::layout::Dir;
    let _serial = serial();
    let mut h = app(project("rotate"));
    h.run_steps(3);
    h.key_press_modifiers(CMD, Key::D);
    h.run_steps(3);
    assert_eq!(
        ws(h.state()).focused_split(),
        Some(Dir::Row),
        "⌘D: lado a lado"
    );
    // Cada panel tiene el botón; el del enfocado (el nuevo) basta.
    let rotate = h.get_all_by_label("Poner uno encima del otro").count();
    assert_eq!(rotate, 2, "un botón por panel de la división");
    h.get_all_by_label("Poner uno encima del otro")
        .next()
        .unwrap()
        .click();
    h.run_steps(3);
    assert_eq!(ws(h.state()).focused_split(), Some(Dir::Column));
    h.get_all_by_label("Poner lado a lado")
        .next()
        .unwrap()
        .click();
    h.run_steps(3);
    assert_eq!(ws(h.state()).focused_split(), Some(Dir::Row));
}

/// Elegir con ↑↓ y pulsar Enter acepta la sugerencia elegida (no ejecuta lo escrito).
#[test]
#[cfg_attr(
    windows,
    ignore = "folder suggestions while typing come from Forge's zsh (OSC 7777)"
)]
fn enter_accepts_the_suggestion_chosen_with_the_arrows() {
    let _serial = serial();
    let dir = project("suggest-enter");
    std::fs::create_dir_all(dir.join("Programar")).unwrap();
    std::fs::create_dir_all(dir.join("proyectos")).unwrap();
    let mut h = app(dir.clone());
    wait_until(&mut h, "shell listo", |a| {
        ws(a).is_quiet(Duration::from_millis(500))
    });
    let osc = format!("printf '\\e]7777;cd pro\\x1f{}\\a'", dir.display());
    h.event(egui::Event::Text(osc));
    h.key_press(Key::Enter);
    for _ in 0..40 {
        h.run_steps(2);
        if h.query_by_label("proyectos/").is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    h.get_by_label("Programar/");
    h.key_press(Key::ArrowDown);
    h.run_steps(2);
    h.key_press(Key::Enter);
    // "pro" ya está escrito: solo se envía lo que falta (aquí la línea real no tiene "cd
    // pro" porque el aviso lo simula printf).
    wait_until(&mut h, "Enter escribe la elegida", |a| {
        ws(a).shell_text().contains("yectos/")
    });
    assert!(
        h.query_by_label("proyectos/").is_none(),
        "la lista se cierra"
    );
}

/// Terminales tipo VS Code: ⌘T y el + añaden al mismo panel, la lista lateral cambia de
/// una a otra, ⌘⇧] pasa a la siguiente y la papelera cierra una.
#[test]
fn stacked_terminals_from_the_panel_ui() {
    let _serial = serial();
    let mut h = app(project("tabs-ui"));
    h.run_steps(3);
    let first = ws(h.state()).focused();
    // ⌘T y el + de la cabecera: terminales nuevas en el mismo panel.
    h.key_press_modifiers(CMD, Key::T);
    h.run_steps(3);
    h.get_all_by_label(forge_core::tr!("Nueva terminal en este panel (⌘T)"))
        .next()
        .unwrap()
        .click();
    h.run_steps(3);
    assert_eq!(ws(h.state()).panel_count(), 3);
    let tabs = ws(h.state()).panel_tabs(first);
    assert_eq!(tabs.len(), 3, "las tres en el mismo panel");

    // Lista lateral: clic en la primera fila la pone a la vista.
    h.get_by_label_contains("Terminal 1 de 3").click();
    h.run_steps(3);
    assert_eq!(ws(h.state()).focused(), first, "la primera fila");

    // ⌘⇧]: la siguiente.
    h.key_press_modifiers(CMD_SHIFT, Key::CloseBracket);
    h.run_steps(3);
    assert_eq!(ws(h.state()).focused(), tabs[1]);

    // Papelera de una fila: cierra esa terminal.
    let trash = |h: &Harness<'_, App>| {
        h.get_all_by(|n| n.label().is_some_and(|l| l.starts_with("Cerrar «")))
            .count()
    };
    assert_eq!(trash(&h), 3, "una papelera por fila");
    h.get_all_by(|n| n.label().is_some_and(|l| l.starts_with("Cerrar «")))
        .next()
        .unwrap()
        .click();
    h.run_steps(3);
    assert_eq!(ws(h.state()).panel_count(), 2);
}

/// Una terminal que escribe mientras está oculta lleva aviso en la lista; las filas se
/// reordenan arrastrándolas.
#[test]
fn hidden_terminals_show_activity_and_rows_reorder_by_drag() {
    let _serial = serial();
    let mut h = app(project("tabs-polish"));
    wait_until(&mut h, "shell listo", |a| {
        ws(a).is_quiet(Duration::from_millis(500))
    });
    let first = ws(h.state()).focused();
    // Escribe dentro de un segundo; mientras, se pasa a otra terminal del panel.
    h.event(egui::Event::Text("sleep 1; echo zzoculta".into()));
    h.key_press(Key::Enter);
    h.run_steps(2);
    h.key_press_modifiers(CMD, Key::T);
    h.run_steps(3);
    let second = ws(h.state()).focused();
    assert_ne!(first, second);
    wait_until(&mut h, "aviso de la terminal oculta", |a| {
        ws(a).has_tab_alert(first)
    });
    assert!(
        !ws(h.state()).has_tab_alert(second),
        "la visible no lleva aviso"
    );

    // Arrastrar la segunda fila por encima de la primera.
    h.key_press_modifiers(CMD, Key::T);
    h.run_steps(3);
    let tabs = ws(h.state()).panel_tabs(first);
    assert_eq!(tabs.len(), 3);
    let row = |h: &Harness<'_, App>, n: usize| {
        h.get_by_label_contains(&format!("Terminal {n} de 3"))
            .rect()
            .center()
    };
    let (from, to) = (row(&h, 3), row(&h, 1));
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.event(egui::Event::PointerMoved(from));
    h.event(button(from, true));
    h.run_steps(2);
    for k in 1..=6 {
        let y = from.y + (to.y - 10.0 - from.y) * k as f32 / 6.0;
        h.event(egui::Event::PointerMoved(egui::pos2(from.x, y)));
        h.run_steps(1);
    }
    h.event(button(egui::pos2(from.x, to.y - 10.0), false));
    h.run_steps(3);
    assert_eq!(
        ws(h.state()).panel_tabs(first),
        vec![tabs[2], tabs[0], tabs[1]],
        "la tercera pasa a la primera posición"
    );
}

#[test]
fn projects_reorder_by_dragging_their_icon() {
    let _serial = serial();
    let (a, b, c) = (project("drag-a"), project("drag-b"), project("drag-c"));
    let store = Store::in_memory().unwrap();
    for p in [&a, &b, &c] {
        store
            .touch(p, p.file_name().unwrap().to_str().unwrap())
            .unwrap();
    }
    store.set_setting("active", &a.to_string_lossy()).unwrap();
    let mut h = app_with(store, None);
    h.run_steps(3);
    let name = |p: &PathBuf| p.file_name().unwrap().to_str().unwrap().to_string();
    let rect = |h: &Harness<'_, App>, p: &PathBuf| h.get_by_label(&name(p)).rect();
    // Se agarra por el icono de carpeta y se suelta encima del primero.
    let from = egui::pos2(rect(&h, &c).min.x + 14.0, rect(&h, &c).center().y);
    let to = egui::pos2(from.x, rect(&h, &a).min.y + 4.0);
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.event(egui::Event::PointerMoved(from));
    h.event(button(from, true));
    h.run_steps(2);
    for k in 1..=6 {
        let y = from.y + (to.y - from.y) * k as f32 / 6.0;
        h.event(egui::Event::PointerMoved(egui::pos2(from.x, y)));
        h.run_steps(1);
    }
    h.event(button(to, false));
    h.run_steps(3);
    let order: Vec<_> = h
        .state()
        .workspaces
        .iter()
        .map(|w| w.project.path.clone())
        .collect();
    assert_eq!(
        order,
        vec![c.clone(), a.clone(), b.clone()],
        "C pasa arriba"
    );
    assert_eq!(h.state().active, Some(1), "sigue activo A");
    // Y el orden queda guardado para la próxima vez.
    let saved = h.state().store.as_ref().unwrap().open_projects().unwrap();
    assert_eq!(saved, order);
}

#[test]
fn the_app_closes_phases_whose_branch_reached_main() {
    let _serial = serial();
    let dir = project("merged-app");
    let ok = std::process::Command::new("sh")
        .args([
            "-c",
            "git commit -qm inicio && git switch -qc feat/uno && echo 2 > b.txt && git add -A \
             && git commit -qm uno && git switch -q main && git merge -q --squash feat/uno \
             && git commit -qm 'uno (#1)'",
        ])
        .current_dir(&dir)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    // Base en archivo: la comprobación corre en otro hilo con su propia conexión.
    let db = dir.join(".forge/test.db");
    let store = Store::open(&db).unwrap();
    let id = store
        .add_idea(Some(&dir), "Plan", "", "claude-code")
        .unwrap();
    let phase = forge_core::ideas::Phase {
        title: "Uno".into(),
        notes: String::new(),
        status: "doing".into(),
        branch: Some("feat/uno".into()),
        tasks: Vec::new(),
    };
    store
        .set_phases(Some(&dir), id, &[phase], "claude-code")
        .unwrap();
    let mut h = app_with(store, Some(dir.clone()));
    h.run_steps(3);
    h.state_mut().merged_check = Instant::now() - Duration::from_secs(600);
    wait_until(&mut h, "la fase se cierra sola", |a| {
        a.store
            .as_ref()
            .unwrap()
            .get_idea(Some(&dir), id)
            .unwrap()
            .unwrap()
            .status
            == "done"
    });
}

#[test]
fn opened_projects_keep_a_plain_path() {
    // En Windows, `canonicalize` antepone `\\?\`: PowerShell lo mostraba en el prompt
    // (`Microsoft.PowerShell.Core\FileSystem::\\?\C:\…`) y llegaba así al MCP.
    let _serial = serial();
    let h = app(project("plain-path"));
    let path = ws(h.state()).project.path.to_string_lossy().into_owned();
    assert!(!path.starts_with(r"\\?\"), "{path}");
    let server = ws(h.state()).mcp_server().unwrap();
    assert!(!server.command.starts_with(r"\\?\"), "{}", server.command);
}

#[test]
fn composed_text_reaches_the_shell() {
    // Teclas muertas (´ + a = á) y métodos de entrada: macOS entrega el texto compuesto
    // como Ime::Commit, solo si quien tiene el foco lo pidió.
    let _serial = serial();
    let mut h = app(project("ime"));
    wait_until(&mut h, "shell listo", |a| {
        ws(a).is_quiet(Duration::from_millis(500))
    });
    assert!(
        h.output().platform_output.ime.is_some(),
        "la terminal pide el IME"
    );
    h.event(egui::Event::Text("echo zz".into()));
    h.event(egui::Event::Ime(egui::ImeEvent::Commit("áñ@".into())));
    h.key_press(Key::Enter);
    wait_until(&mut h, "el shell recibe lo compuesto", |a| {
        ws(a).shell_text().matches("zzáñ@").count() >= 2
    });
}

#[test]
fn the_close_button_hides_forge_on_macos_instead_of_quitting() {
    // La X roja cortaba agentes y procesos en marcha: en macOS esconde Forge (como ⌘H) y
    // ⌘Q sale. Los cierres que pide Forge (reiniciar tras actualizar) sí cierran.
    let _serial = serial();
    let mut h = app(project("close-hides"));
    let close = |h: &mut Harness<'_, App>| {
        h.input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
        h.run_steps(1);
        h.output().viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::CancelClose)
    };
    assert_eq!(close(&mut h), cfg!(target_os = "macos"));
    h.state_mut().quitting = true;
    assert!(!close(&mut h), "Forge pidió cerrar: se cierra");
}

#[test]
fn the_compact_offer_types_slash_compact_or_waits_for_later() {
    // Con mucho contexto, el mod de Claude Code avisa y el panel ofrece compactar.
    let _serial = serial();
    let mut h = app(project("compact"));
    wait_until(&mut h, "shell listo", |a| {
        ws(a).is_quiet(Duration::from_millis(500))
    });
    let panel = ws(h.state()).focused();
    h.state_mut().workspaces[0].compact_offer(panel);
    // Mientras la terminal escribe, el botón espera (no corta la respuesta del agente).
    h.event(egui::Event::Text("echo ocupado".into()));
    h.key_press(Key::Enter);
    h.run_steps(3);
    use egui_kittest::kittest::NodeT;
    assert!(
        h.get_by_label("Compactar").accesskit_node().is_disabled(),
        "con la terminal escribiendo, el botón espera"
    );
    wait_until(&mut h, "terminal tranquila", |a| {
        ws(a).is_quiet(Duration::from_millis(2200))
    });
    h.run_steps(3);
    h.get_by_label("Compactar").click();
    wait_until(&mut h, "la terminal recibe /compact", |a| {
        ws(a).shell_text().contains("/compact")
    });
    h.run_steps(2);
    assert!(h.query_by_label("Compactar").is_none(), "la franja se va");
    // «Luego»: se va sin escribir nada (el mod vuelve a ofrecerlo más adelante).
    h.state_mut().workspaces[0].compact_offer(panel);
    h.run_steps(3);
    h.get_by_label("Luego").click();
    h.run_steps(3);
    assert!(h.query_by_label("Compactar").is_none());
}

#[test]
fn clicking_an_agent_goes_to_its_open_terminal_instead_of_opening_another() {
    let _serial = serial();
    let dir = project("agent-click");
    std::fs::write(
        dir.join(".forge/agents.toml"),
        "[[agents]]\nid = \"fake\"\nname = \"Agente de prueba\"\ncommand = \"sleep 60\"\n",
    )
    .unwrap();
    let mut h = app(dir);
    let ctx = h.ctx.clone();
    let area = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1000.0, 700.0));
    let before = ws(h.state()).panel_count();
    let root = ws(h.state()).focused();
    let click = |h: &mut Harness<'_, App>, new: bool| {
        h.state_mut()
            .apply(&ctx, UiCmd::AgentClick("fake".into(), new), area);
        h.run_steps(2);
    };
    click(&mut h, false);
    assert_eq!(ws(h.state()).panel_count(), before + 1, "abre el agente");
    let agent = ws(h.state()).agent_panels("fake")[0];
    h.state_mut().apply(&ctx, UiCmd::GoToPanel(root), area);
    assert_eq!(ws(h.state()).focused(), root);
    click(&mut h, false);
    assert_eq!(ws(h.state()).panel_count(), before + 1, "no abre otro");
    assert_eq!(ws(h.state()).focused(), agent, "lleva a su terminal");
    click(&mut h, true);
    assert_eq!(
        ws(h.state()).panel_count(),
        before + 2,
        "+ abre una sesión nueva"
    );
    assert_eq!(ws(h.state()).agent_panels("fake").len(), 2);
}

/// Informe: cambiar el nombre de una cuenta nueva colgaba el Mac. Escribir en el campo y
/// guardarlo con Enter tiene que ser instantáneo y no crear nada en disco.
#[test]
fn renaming_a_new_account_is_cheap() {
    let _serial = serial();
    let dir = project("account-rename");
    let mut store = Store::in_memory().unwrap();
    store.accounts_root = Some(dir.join(".accounts"));
    store.home = None;
    let mut h = app_with(store, Some(dir.clone()));
    let ctx = h.ctx.clone();
    let area = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1000.0, 700.0));
    h.state_mut()
        .apply(&ctx, UiCmd::AccountAdd("claude".into()), area);
    // Las cuentas solo se ven con `claude` instalado (en CI no lo está): tras la detección
    // de fondo, que no lo vuelva a pisar.
    wait_until(&mut h, "detección de agentes", |a| {
        !a.detecting.load(std::sync::atomic::Ordering::SeqCst)
    });
    h.state().agents.lock().unwrap().insert(
        "claude".into(),
        agents::Detection {
            path: Some("/usr/local/bin/claude".into()),
            version: None,
        },
    );
    h.key_press_modifiers(CMD, Key::Comma);
    h.run_steps(3);
    h.get_by_label_contains("Agentes").click();
    h.run_steps(3);
    let before = std::fs::read_dir(dir.join(".accounts")).unwrap().count();
    let start = Instant::now();
    let new = h.state().store.as_ref().unwrap().accounts("claude")[1]
        .name
        .clone();
    h.query_all_by_role(egui::accesskit::Role::TextInput)
        .find(|n| n.value().as_deref() == Some(new.as_str()))
        .expect("el campo del nombre")
        .focus();
    h.run_steps(2);
    h.input_mut()
        .events
        .push(egui::Event::Text(" de la empresa".into()));
    // El cursor parpadea mientras se escribe: muchos fotogramas seguidos.
    h.run_steps(120);
    h.key_press(Key::Enter);
    h.run_steps(10);
    let took = start.elapsed();
    let name = h.state().store.as_ref().unwrap().accounts("claude")[1]
        .name
        .clone();
    assert_eq!(name, format!("{new} de la empresa"));
    assert!(took < Duration::from_secs(10), "renombrar tardó {took:?}");
    assert_eq!(
        std::fs::read_dir(dir.join(".accounts")).unwrap().count(),
        before,
        "renombrar no crea carpetas"
    );
}

/// Con otra cuenta elegida en el proyecto, un `claude` escrito a mano en una terminal
/// nueva usa su carpeta (CLAUDE_CONFIG_DIR).
#[test]
fn a_chosen_account_reaches_new_terminals_of_the_project() {
    let _serial = serial();
    let dir = project("accounts");
    let mut store = Store::in_memory().unwrap();
    store.accounts_root = Some(dir.join(".accounts"));
    store.home = None;
    let mut h = app_with(store, Some(dir.clone()));
    let ctx = h.ctx.clone();
    let area = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1000.0, 700.0));
    h.state_mut()
        .apply(&ctx, UiCmd::AccountAdd("claude".into()), area);
    h.state_mut()
        .apply(&ctx, UiCmd::UseAccount("claude".into(), 1), area);
    let w = ws(h.state());
    assert!(w.has_accounts("claude"));
    assert_eq!(w.account_for("claude", None).map(|a| a.id), Some(1));
    assert!(dir.join(".accounts/claude-1").is_dir());

    h.state_mut().workspaces[0].new_tab(&ctx);
    assert_eq!(ws(h.state()).panel_count(), 2);
    wait_until(&mut h, "shell listo", |a| {
        ws(a).is_quiet(Duration::from_millis(500))
    });
    let show = if cfg!(windows) {
        "\"$(Split-Path -Leaf $env:CLAUDE_CONFIG_DIR)zz\""
    } else {
        "echo \"${CLAUDE_CONFIG_DIR##*/}\"zz"
    };
    h.event(egui::Event::Text(show.into()));
    h.key_press(Key::Enter);
    wait_until(&mut h, "la terminal ve la cuenta", |a| {
        ws(a).shell_text().contains("claude-1zz")
    });

    // Una cuenta con terminales abiertas no se borra; sin ellas, sí (y su carpeta).
    h.state_mut()
        .apply(&ctx, UiCmd::UseAccount("claude".into(), 0), area);
    h.state_mut()
        .apply(&ctx, UiCmd::AccountDelete("claude".into(), 1), area);
    assert!(!dir.join(".accounts/claude-1").exists());
    assert!(!ws(h.state()).has_accounts("claude"));
}

/// El formulario de Guard guarda solo poco después de un cambio, sin botón.
#[test]
fn the_guard_form_saves_by_itself() {
    let _serial = serial();
    let dir = project("guard-form");
    let mut h = app(dir.clone());
    let ctx = h.ctx.clone();
    super::settings::show_tab(&ctx, super::settings::SettingsTab::Project);
    h.state_mut().settings_open = true;
    h.run_steps(3);
    // Bajar hasta "Secretos y archivos".
    h.hover_at(egui::pos2(640.0, 400.0));
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -500.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    h.run_steps(5);
    h.get_by_label("Bloquear archivos .env").click();
    let rules = dir.join(".forge/rules.toml");
    wait_until(&mut h, "se guarda solo", |_| {
        std::fs::read_to_string(&rules).is_ok_and(|t| t.contains("block_env_files = false"))
    });
    // Lo que había en el archivo sigue (el check de tests del proyecto de prueba).
    let text = std::fs::read_to_string(&rules).unwrap();
    assert!(text.contains("id = \"tests\""), "{text}");
}

/// Agentes del proyecto desde Ajustes: desactivar uno lo quita de la barra lateral.
#[test]
fn turning_an_agent_off_in_settings_hides_it() {
    let _serial = serial();
    let dir = project("agents-form");
    std::fs::write(
        dir.join(".forge/agents.toml"),
        "[[agents]]\nid = \"fake\"\nname = \"Agente de prueba\"\ncommand = \"sleep 60\"\n",
    )
    .unwrap();
    let mut h = app(dir.clone());
    let ctx = h.ctx.clone();
    assert!(
        ws(h.state())
            .project
            .agents
            .iter()
            .any(|a| a.id == "fake" && a.enabled)
    );
    super::settings::show_tab(&ctx, super::settings::SettingsTab::Project);
    h.state_mut().settings_open = true;
    h.run_steps(3);
    // Desactivar el propio (el formulario está más abajo: se cambia su estado y se espera
    // a que se guarde solo, como al pulsar la casilla).
    let g = h
        .state_mut()
        .agents_form
        .as_mut()
        .expect("formulario de agentes cargado");
    g.form.iter_mut().find(|a| a.id == "fake").unwrap().enabled = false;
    wait_until(&mut h, "se guarda y se recarga", |a| {
        ws(a)
            .project
            .agents
            .iter()
            .any(|x| x.id == "fake" && !x.enabled)
    });
    let text = std::fs::read_to_string(dir.join(".forge/agents.toml")).unwrap();
    assert!(text.contains("enabled = false"), "{text}");
}

/// Un proceso añadido en Ajustes aparece en la barra lateral sin recargar a mano.
#[test]
fn a_process_added_in_settings_shows_up() {
    let _serial = serial();
    let dir = project("processes-form");
    let mut h = app(dir.clone());
    let ctx = h.ctx.clone();
    super::settings::show_tab(&ctx, super::settings::SettingsTab::Project);
    h.state_mut().settings_open = true;
    h.run_steps(3);
    let g = h
        .state_mut()
        .processes_form
        .as_mut()
        .expect("formulario de procesos cargado");
    g.form.processes.push(forge_core::project::ProcessForm {
        id: "api".into(),
        name: "API".into(),
        command: "sleep 60".into(),
        ..Default::default()
    });
    wait_until(&mut h, "se guarda y se recarga", |a| {
        ws(a).project.config.processes.iter().any(|p| p.id == "api")
    });
    assert!(
        ws(h.state()).processes.get("api").is_some(),
        "en el panel PROCESOS"
    );
}

/// Sin procesos, la barra lateral ofrece añadir uno y lleva al formulario de Ajustes.
#[test]
fn add_process_in_the_sidebar_opens_its_form() {
    let _serial = serial();
    let mut h = app(project("no-processes"));
    h.run_steps(3);
    assert!(ws(h.state()).processes.list.is_empty());
    h.get_by_label("Añadir proceso").click();
    h.run_steps(3);
    assert!(h.state().settings_open);
    // El formulario lee .forge/project.toml: en un CI lento tarda más de unos fotogramas.
    let start = Instant::now();
    while h
        .query_by_label_contains("Servidores y tareas que Forge arranca")
        .is_none()
    {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "abre en Este proyecto, con el formulario de procesos"
        );
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Memoria, hilos y descriptores del proceso: (RSS en bytes, hilos, archivos abiertos).
fn usage() -> (u64, u64, usize) {
    let fds = std::fs::read_dir("/dev/fd").map(|d| d.count()).unwrap_or(0);
    #[cfg(target_os = "macos")]
    {
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        // SAFETY: proc_pidinfo escribe como máximo `size` bytes en `info`, que es POD.
        let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let written = unsafe {
            libc::proc_pidinfo(
                std::process::id() as libc::c_int,
                libc::PROC_PIDTASKINFO,
                0,
                (&mut info as *mut libc::proc_taskinfo).cast(),
                size,
            )
        };
        assert_eq!(written, size);
        (info.pti_resident_size, info.pti_threadnum as u64, fds)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let field = |name: &str| {
            status
                .lines()
                .find_map(|l| l.strip_prefix(name))
                .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
                .unwrap()
        };
        (field("VmRSS:") * 1024, field("Threads:"), fds)
    }
}

/// Abrir y cerrar terminales una y otra vez no deja nada atrás: los hilos y los archivos
/// abiertos vuelven a su nivel y la memoria no crece con cada vuelta. Corre en un proceso
/// propio (`memory_stays_flat_inner`): los demás tests abren hilos y archivos a la vez.
#[test]
#[cfg_attr(
    windows,
    ignore = "measures with /proc and /dev/fd; Windows needs GetProcessMemoryInfo and handle counts"
)]
fn opening_and_closing_terminals_does_not_leak() {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "app::e2e::memory_stays_flat_inner",
            "--exact",
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .output()
        .unwrap();
    let log =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{log}");
    assert!(log.contains("1 passed"), "no corrió el test interno: {log}");
}

#[test]
#[ignore = "lo lanza opening_and_closing_terminals_does_not_leak en su propio proceso"]
fn memory_stays_flat_inner() {
    let mut h = app(project("leaks"));
    let cycle = |h: &mut Harness<'_, App>, n: usize| {
        for _ in 0..n {
            h.key_press_modifiers(CMD, Key::D);
            h.run_steps(3);
            h.key_press_modifiers(CMD, Key::W);
            h.run_steps(3);
        }
        assert_eq!(ws(h.state()).panel_count(), 1);
    };
    // Calentar: cachés de egui, fuentes, el primer shell.
    cycle(&mut h, 5);
    let settle = |h: &mut Harness<'_, App>, (_, threads, fds): (u64, u64, usize)| {
        // Los hilos lectores terminan cuando el shell cerrado suelta su PTY.
        let start = Instant::now();
        loop {
            h.run_steps(2);
            let now = usage();
            if now.1 <= threads + 2 && now.2 <= fds + 2 {
                return now;
            }
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "hilos o archivos sin cerrar: antes {threads} hilos y {fds} archivos, ahora {} y {}",
                now.1,
                now.2
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    let before = settle(&mut h, usage());
    cycle(&mut h, 40);
    let after = settle(&mut h, before);
    let grown = after.0.saturating_sub(before.0) / (1 << 20);
    eprintln!(
        "40 terminales abiertas y cerradas: RSS {} → {} MB, hilos {} → {}, archivos {} → {}",
        before.0 >> 20,
        after.0 >> 20,
        before.1,
        after.1,
        before.2,
        after.2
    );
    // ponytail: margen amplio para el asignador; una fuga por terminal (su rejilla, su
    // historial) pasaría de largo.
    assert!(grown < 40, "la memoria creció {grown} MB en 40 terminales");
}
