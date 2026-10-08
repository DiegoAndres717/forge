// Tests de extremo a extremo de la interfaz: la app real (shells, Git, SQLite en memoria)
// manejada como lo haría el usuario, con atajos de teclado y clics sobre los controles
// por su nombre accesible. Sin GPU: corren en CI.
use super::*;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

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
fn ideas_are_noted_crossed_out_and_refreshed_from_agents() {
    let dir = project("ideas");
    let mut h = app(dir.clone());
    h.key_press_modifiers(CMD_SHIFT, Key::I);
    h.run_steps(2);
    assert!(ws(h.state()).ideas.open, "⌘⇧I abre Ideas");

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

    // Un agente anota otra por MCP: aparece sola en el panel.
    h.state()
        .store
        .as_ref()
        .unwrap()
        .add_idea(Some(&dir), "Notificar vencidas", "", "claude-code")
        .unwrap();
    wait_until(&mut h, "la idea del agente aparece", |a| {
        ws(a).ideas.items.len() == 2
    });
    h.run_steps(2);
    h.get_by_label("Notificar vencidas");

    h.get_by_label("Marcar como hecha: Exportar vacunas a Excel")
        .click();
    h.run_steps(3);
    assert!(
        store(&h, Some(&dir))
            .iter()
            .any(|i| i.title == "Exportar vacunas a Excel" && i.done())
    );
    h.get_by_label_contains("Mostrar hechas (1)");

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
    let dir = project("attention");
    std::fs::write(
        dir.join(".forge/project.toml"),
        "[[processes]]\nid = \"api\"\nname = \"API\"\ncommand = \"sleep 1; exit 3\"\nrestart = \"on-workspace-open\"\n",
    )
    .unwrap();
    let mut h = app(dir);
    assert_eq!(ws(h.state()).attention(), None);
    h.key_press_modifiers(CMD_SHIFT, Key::H); // se sale del proyecto antes de que falle
    h.run_steps(2);
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
    // Con el proyecto a la vista mientras arranca el shell.
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        h.run_steps(2);
        std::thread::sleep(Duration::from_millis(50));
    }
    h.key_press_modifiers(CMD_SHIFT, Key::H);
    while start.elapsed() < Duration::from_secs(11) {
        h.run_steps(2);
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(ws(h.state()).attention(), None);
}

#[test]
fn git_panel_stages_unstages_and_commits() {
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
