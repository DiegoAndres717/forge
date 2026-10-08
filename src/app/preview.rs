// Capturas de la interfaz sin pantalla (para revisar el diseño):
// `FORGE_PREVIEW_DIR=/ruta cargo test --release ui_preview -- --ignored --nocapture`
use super::*;
use egui_kittest::Harness;

fn save(harness: &mut Harness<'_, App>, dir: &Path, name: &str) {
    let image = harness.render().expect("render");
    image
        .save(dir.join(format!("{name}.png")))
        .expect("guardar captura");
}

fn settle(harness: &mut Harness<'_, App>) {
    for _ in 0..6 {
        harness.run_steps(3);
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Icono de la app (PNG 1024 con transparencia), para `scripts/install-app.sh`:
/// `FORGE_ICON_OUT=assets/icon.png cargo test --release app_icon -- --ignored`
#[test]
#[ignore]
fn app_icon() {
    let out = PathBuf::from(std::env::var("FORGE_ICON_OUT").expect("FORGE_ICON_OUT"));
    const SIZE: f32 = 1024.0;
    // Rejilla de iconos de macOS: 824 px de contenido con radio ~185 dentro de 1024.
    let tile = Rect::from_center_size(egui::pos2(SIZE / 2.0, SIZE / 2.0), Vec2::splat(824.0));
    const RADIUS: f32 = 185.0;
    let mut harness = Harness::builder()
        .with_size([SIZE, SIZE])
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_ui(move |ui| {
            let p = ui.painter();
            p.rect_filled(ui.max_rect(), 0.0, Color32::BLACK);
            p.rect_filled(tile, RADIUS, Color32::from_rgb(0x1e, 0x1e, 0x21));
            // Degradado vertical real (malla con color por vértice); lo que sale de la forma se recorta después.
            let mut mesh = egui::Mesh::default();
            let (top, bottom) = (Color32::from_white_alpha(20), Color32::TRANSPARENT);
            mesh.colored_vertex(tile.left_top(), top);
            mesh.colored_vertex(tile.right_top(), top);
            mesh.colored_vertex(tile.right_bottom(), bottom);
            mesh.colored_vertex(tile.left_bottom(), bottom);
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            p.add(mesh);
            p.rect_stroke(
                tile,
                RADIUS,
                egui::Stroke::new(3.0, Color32::from_white_alpha(28)),
                egui::StrokeKind::Inside,
            );
            // Resplandor cálido radial detrás de la llama.
            for i in 0..60 {
                let r = 330.0 * (1.0 - i as f32 / 60.0);
                p.circle_filled(
                    egui::pos2(SIZE / 2.0, 470.0),
                    r,
                    Color32::from_rgba_unmultiplied(0xff, 0x8a, 0x00, 2),
                );
            }
            p.text(
                egui::pos2(SIZE / 2.0, 455.0),
                egui::Align2::CENTER_CENTER,
                icon::FLAME,
                FontId::proportional(430.0),
                theme::ORANGE,
            );
            p.text(
                egui::pos2(SIZE / 2.0, 735.0),
                egui::Align2::CENTER_CENTER,
                ">_",
                FontId::monospace(120.0),
                Color32::from_rgb(0xe5, 0xe5, 0xea),
            );
        });
    crate::install_fonts(&harness.ctx, &Settings::default()).unwrap();
    harness.run_steps(3);
    let mut image = harness.render().expect("render");
    // Fuera del cuadrado redondeado: transparente (con borde suavizado).
    for (x, y, px) in image.enumerate_pixels_mut() {
        let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
        let dx = (tile.min.x + RADIUS - cx)
            .max(cx - (tile.max.x - RADIUS))
            .max(0.0);
        let dy = (tile.min.y + RADIUS - cy)
            .max(cy - (tile.max.y - RADIUS))
            .max(0.0);
        let inside = (RADIUS - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
        let inside = if cx < tile.min.x || cx > tile.max.x || cy < tile.min.y || cy > tile.max.y {
            0.0
        } else {
            inside
        };
        px.0[3] = (px.0[3] as f32 * inside) as u8;
    }
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    image.save(&out).expect("guardar icono");
}

#[test]
#[ignore]
fn ui_preview() {
    // Como la app: inglés salvo FORGE_LANG=es.
    forge_core::i18n::init(None);
    let out = PathBuf::from(std::env::var("FORGE_PREVIEW_DIR").expect("FORGE_PREVIEW_DIR"));
    std::fs::create_dir_all(&out).unwrap();
    let base = std::env::temp_dir().join(format!("forge-preview-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("Bovinapp/.forge")).unwrap();
    // Ruta canónica (/private/var…): la que guarda la app al abrir el proyecto.
    let project = base.join("Bovinapp").canonicalize().unwrap();
    std::fs::create_dir_all(project.join("src/vacunas")).unwrap();
    let sh = |c: &str| {
        assert!(
            std::process::Command::new("sh")
                .args(["-c", c])
                .current_dir(&project)
                .status()
                .unwrap()
                .success()
        )
    };
    sh(
        "git init -q -b main && git config user.email t@t && git config user.name t && echo '# Bovinapp' > README.md && git add . && git commit -qm inicio",
    );
    std::fs::write(
        project.join(".forge/project.toml"),
        "[project]\nname = \"Bovinapp\"\n\
         [[commands]]\nname = \"Tests\"\ncommand = \"npm test\"\n\
         [[processes]]\nid = \"frontend\"\nname = \"Frontend\"\ncommand = \"sleep 600\"\nrestart = \"on-workspace-open\"\n\
         [[processes]]\nid = \"api\"\nname = \"API\"\ncommand = \"sleep 600\"\n",
    )
    .unwrap();
    std::fs::write(project.join(".forge/rules.toml"), "[commit]\nrequire_tests = true\n[[checks]]\nid = \"tests\"\nname = \"Tests\"\ncommand = \"sleep 0.3\"\n").unwrap();
    std::fs::write(
        project.join("src/vacunas/lotes.ts"),
        "export const lote = 1;\n",
    )
    .unwrap();
    sh("git add src");
    // SAFETY: test aislado; la base de datos de la vista previa no es la del usuario.
    unsafe { std::env::set_var("FORGE_DB", base.join("forge.db")) };
    {
        let store = Store::open(&base.join("forge.db")).unwrap();
        for (name, ago) in [("ReparAppi", 86_400), ("wabio", 5 * 86_400)] {
            let p = base.join(name);
            std::fs::create_dir_all(&p).unwrap();
            store.touch(&p, name).unwrap();
            store.conn.execute("UPDATE projects SET last_opened = last_opened - ?1, is_open = 0 WHERE path = ?2", rusqlite::params![ago, p.to_string_lossy()]).unwrap();
        }
        store
            .add_memory(
                &project,
                "decision",
                "Vacunación por lotes",
                "Se registran por lote para no duplicar animales.",
                "vacunas",
                "claude-code",
            )
            .unwrap();
        store
            .add_memory(
                &project,
                "error",
                "Migración con índice duplicado",
                "drizzle-kit falló; se renombró el índice.",
                "",
                "usuario",
            )
            .unwrap();
        store
            .add_idea(Some(&project), "Exportar vacunas a Excel", "", "usuario")
            .unwrap();
        let b = store
            .add_idea(
                Some(&project),
                "Notificar vacunas vencidas",
                "Push al celular y correo al veterinario.",
                "claude-code",
            )
            .unwrap();
        store
            .add_idea(Some(&project), "Modo offline para el campo", "", "codex")
            .unwrap();
        let c = store
            .add_idea(Some(&project), "Filtro por lote", "", "usuario")
            .unwrap();
        store
            .update_idea(Some(&project), b, Some("doing"), None, None, "claude-code")
            .unwrap();
        store
            .update_idea(Some(&project), c, Some("done"), None, None, "claude-code")
            .unwrap();
        store
            .add_idea(None, "App para talleres de motos", "", "usuario")
            .unwrap();
        store
            .add_idea(None, "Probar Zed como editor", "", "usuario")
            .unwrap();
    }

    let project_arg = project.clone();
    let mut harness = Harness::builder()
        .with_size([1440.0, 880.0])
        .with_pixels_per_point(2.0)
        .wgpu()
        .build_eframe(move |cc| {
            crate::install_fonts(&cc.egui_ctx, &Settings::default()).unwrap();
            App::new(&cc.egui_ctx, Settings::default(), None, Some(project_arg))
        });
    settle(&mut harness);
    // Dos paneles más: división a la derecha y abajo.
    let ctx = harness.ctx.clone();
    harness.state_mut().workspaces[0].show_process(
        &ctx,
        "frontend",
        Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 800.0)),
    );
    // Panel de agente (Claude Code) para ver su logo en la cabecera. // claude-panel
    let ctx = harness.ctx.clone();
    harness.state_mut().workspaces[0].open_agent(
        &ctx,
        "claude",
        false,
        Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 800.0)),
    );
    settle(&mut harness);
    save(&mut harness, &out, "1-workspace");

    harness.state_mut().workspaces[0].guard.open = true;
    harness
        .state_mut()
        .apply(&ctx, UiCmd::GuardRun, Rect::NOTHING);
    for _ in 0..12 {
        settle(&mut harness);
        if harness.state().workspaces[0]
            .guard
            .run
            .as_ref()
            .is_some_and(|r| r.snapshot(guard::GuardRun::finished))
        {
            break;
        }
    }
    settle(&mut harness);
    save(&mut harness, &out, "2-guard");

    harness.state_mut().workspaces[0].guard.open = false;
    harness.state_mut().workspaces[0].memory.open = true;
    harness.state_mut().workspaces[0].memory.dirty = true;
    settle(&mut harness);
    save(&mut harness, &out, "3-memoria");

    harness.state_mut().workspaces[0].memory.open = false;
    harness.state_mut().open_palette();
    settle(&mut harness);
    save(&mut harness, &out, "5-paleta");
    if let Some(p) = harness.state_mut().palette.as_mut() {
        p.query = "guard".into();
    }
    settle(&mut harness);
    save(&mut harness, &out, "6-paleta-guard");
    harness.state_mut().palette = None;

    // Diálogo de comando peligroso.
    let approvals = base.join("approvals");
    std::fs::create_dir_all(&approvals).unwrap();
    let request = forge_core::danger::Request {
        id: "1-1".into(),
        command: "git push --force origin main".into(),
        reason: "reescribe la historia del remoto (git push --force)".into(),
        cwd: project.clone(),
        project: Some(project.clone()),
        origin: Some("Claude Code".into()),
        pid: std::process::id(),
    };
    std::fs::write(
        approvals.join("1-1.json"),
        serde_json::to_vec(&request).unwrap(),
    )
    .unwrap();
    let ctx = harness.ctx.clone();
    harness.state_mut().watch_approvals(&ctx, approvals.clone());
    settle(&mut harness);
    save(&mut harness, &out, "7-autorizacion");
    forge_core::danger::answer(&approvals, "1-1", false).unwrap();
    settle(&mut harness);

    harness.state_mut().workspaces[0].ideas.open = true;
    harness.state_mut().workspaces[0].ideas.show_done = true;
    harness.state_mut().workspaces[0].ideas.dirty = true;
    settle(&mut harness);
    save(&mut harness, &out, "8-ideas");
    // Panel de Git con un cambio sin preparar.
    std::fs::write(
        project.join("README.md"),
        "# Bovinapp\n\nControl de vacunación.\n",
    )
    .unwrap();
    harness.state_mut().workspaces[0].ideas.open = false;
    harness.state_mut().workspaces[0].git.open = true;
    harness.state_mut().workspaces[0].git.dirty = true;
    settle(&mut harness);
    save(&mut harness, &out, "12-git");
    harness.state_mut().workspaces[0].git.open = false;
    harness.state_mut().settings_open = true;
    settle(&mut harness);
    save(&mut harness, &out, "11-ajustes");
    harness.state_mut().settings_open = false;
    harness.state_mut().workspaces[0].ideas.open = false;

    harness.state_mut().active = None;
    settle(&mut harness);
    save(&mut harness, &out, "4-inicio");

    // Muchos proyectos abiertos: lista compacta con dormidos.
    let ctx = harness.ctx.clone();
    for name in [
        "ReparAppi",
        "wabio",
        "his-erp",
        "mintiplay",
        "zai-proxy",
        "alexa-nova",
        "tiendawa",
        "landing",
    ] {
        let p = base.join(name);
        std::fs::create_dir_all(&p).unwrap();
        harness.state_mut().open_project_as(&ctx, &p, false);
    }
    harness.state_mut().active = Some(0);
    settle(&mut harness);
    save(&mut harness, &out, "9-muchos-proyectos");
}
