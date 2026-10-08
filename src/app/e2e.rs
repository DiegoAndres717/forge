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
            start.elapsed() < Duration::from_secs(20),
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
