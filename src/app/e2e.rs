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

const CMD: Modifiers = Modifiers {
    alt: false,
    ctrl: false,
    shift: false,
    mac_cmd: true,
    command: true,
};
const CMD_SHIFT: Modifiers = Modifiers { shift: true, ..CMD };

/// Repositorio Git temporal con un archivo preparado y un check que pasa.
fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("forge-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".forge")).unwrap();
    let dir = dir.canonicalize().unwrap();
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

    // En el backlog está plegado: se despliega, se empieza la fase 1 y se tacha la tarea.
    h.get_by_label("Ver fases").click();
    h.run_steps(2);
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
        h.get_by_label("Ajustes (⌘,)").accesskit_node().toggled()
            == Some(egui::accesskit::Toggled::True)
    };
    assert!(gear_selected(&h), "el botón ⚙ se ve seleccionado");
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(!h.state().settings_open, "Esc la cierra");
    assert!(!gear_selected(&h));
}

#[test]
fn clickable_controls_show_the_pointing_hand() {
    let _serial = serial();
    let mut h = app(project("cursor"));
    h.key_press_modifiers(CMD, Key::Comma);
    h.run_steps(3);
    for label in ["Avisar cuando", "Español"] {
        h.get_by_label_contains(label).hover();
        h.run_steps(2);
        assert_eq!(
            h.output().platform_output.cursor_icon,
            egui::CursorIcon::PointingHand,
            "{label}"
        );
    }
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
    h.get_by_label_contains("Memoria del proyecto").click();
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

/// Mientras se escribe una ruta, la terminal sugiere carpetas sin distinguir mayúsculas y
/// Tab escribe la elegida. (El zsh de Forge manda la línea en una secuencia invisible; aquí
/// la imprime el propio shell para no depender de su configuración.)
#[test]
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
    std::fs::write(
        dir.join(".forge/agents.toml"),
        "[[agents]]\nid = \"fake\"\nname = \"Agente de prueba\"\ncommand = \"echo pedido:\"\n",
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
    h.state_mut().workspaces[0].ask_agent(&ctx, "fake", "echo zzsegunda$((40+2))", area);
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
