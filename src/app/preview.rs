// Capturas de la interfaz sin pantalla (para revisar el diseño):
// `FORGE_PREVIEW_DIR=/ruta cargo test --release ui_preview -- --ignored --nocapture`
use super::*;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

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
    // Una carpeta personal de demostración: el shell no lee la configuración del usuario
    // (sin fastfetch, nombre del Mac ni rutas reales) y las rutas salen como ~/Developer/…
    let base = std::env::temp_dir().join(format!("forge-preview-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home = base.join("home");
    std::fs::create_dir_all(home.join("Developer")).unwrap();
    let home = home.canonicalize().unwrap();
    std::fs::write(
        home.join(".zshrc"),
        concat!(
            "PROMPT='%F{39}%1~%f %F{76}❯%f '\nexport LANG=en_US.UTF-8\nexport CLICOLOR=1\n",
            // npm de demostración: salida realista sin Node ni dependencias.
            "npm() {\n  case \"$1 $2\" in\n",
            "    'test ') printf '\\n> acme-store@1.4.0 test\\n> vitest run\\n\\n \\033[42;30m PASS \\033[0m src/cart.test.ts \\033[2m(12 tests)\\033[0m\\n \\033[42;30m PASS \\033[0m src/checkout/pay.test.ts \\033[2m(8 tests)\\033[0m\\n\\n \\033[1mTests\\033[0m  \\033[32m20 passed\\033[0m (20)\\n \\033[1mTime\\033[0m   1.84s\\n' ;;\n",
            "    'run dev') printf '\\n> acme-store@1.4.0 dev\\n> vite\\n\\n  \\033[32mVITE v6.0.3\\033[0m  ready in \\033[1m284\\033[0m ms\\n\\n  \\033[32m➜\\033[0m  \\033[1mLocal\\033[0m:   \\033[36mhttp://localhost:\\033[1m5173\\033[0;36m/\\033[0m\\n  \\033[2m➜  Network: use --host to expose\\033[0m\\n'; sleep 600 ;;\n",
            "    'run api') printf '\\n> acme-store@1.4.0 api\\n> tsx watch server.ts\\n\\napi listening on \\033[36mhttp://localhost:3000\\033[0m\\n'; sleep 600 ;;\n",
            "  esac\n}\n",
        ),
    )
    .unwrap();
    // SAFETY: test aislado y manual (--ignored): el shell de las terminales usa este HOME.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("SHELL", "/bin/zsh");
        std::env::remove_var("ZDOTDIR");
    }
    let project = home.join("Developer/acme-store");
    std::fs::create_dir_all(project.join(".forge")).unwrap();
    std::fs::create_dir_all(project.join("src/checkout")).unwrap();
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
        "git init -q -b main && git config user.email dev@acme.test && git config user.name Dev \
         && echo '# Acme Store' > README.md && git add . && git commit -qm 'Initial commit' \
         && echo 'export const cart = [];' > src/cart.ts && git add . && git commit -qm 'Add cart' \
         && echo 'export const pay = () => {};' > src/checkout/pay.ts && git add . && git commit -qm 'Checkout with Stripe'",
    );
    std::fs::write(
        project.join(".forge/project.toml"),
        "[project]\nname = \"Acme Store\"\n\
         [workspace]\ndefault_layout = \"dev\"\n\
         [layouts.dev]\nsplit = \"row\"\nratio = 0.5\n\
         first = { name = \"Terminal\", command = \"git --no-pager log --oneline --decorate\" }\n\
         second = { name = \"Tests\", command = \"npm test\" }\n\
         [[commands]]\nname = \"Tests\"\ncommand = \"npm test\"\n\
         [[processes]]\nid = \"web\"\nname = \"Web\"\ncommand = \"npm run dev\"\nrestart = \"on-workspace-open\"\n\
         [[processes]]\nid = \"api\"\nname = \"API\"\ncommand = \"npm run api\"\n",
    )
    .unwrap();
    std::fs::write(project.join(".forge/rules.toml"), "[commit]\nrequire_tests = true\n[[checks]]\nid = \"tests\"\nname = \"Tests\"\ncommand = \"sleep 0.3\"\n").unwrap();
    std::fs::write(
        project.join("src/checkout/coupons.ts"),
        "export const coupon = 'WELCOME10';\n",
    )
    .unwrap();
    sh("git add src");
    // SAFETY: test aislado; la base de datos de la vista previa no es la del usuario.
    unsafe { std::env::set_var("FORGE_DB", base.join("forge.db")) };
    {
        let store = Store::open(&base.join("forge.db")).unwrap();
        store.set_setting("welcomed", "1").unwrap(); // la bienvenida tiene su propia captura
        for (name, ago) in [("inventory-api", 86_400), ("landing-site", 5 * 86_400)] {
            let p = home.join("Developer").join(name);
            std::fs::create_dir_all(&p).unwrap();
            store.touch(&p, name).unwrap();
            store.conn.execute("UPDATE projects SET last_opened = last_opened - ?1, is_open = 0 WHERE path = ?2", rusqlite::params![ago, p.to_string_lossy()]).unwrap();
        }
        store
            .add_memory(
                &project,
                "decision",
                "Payments go through Stripe Checkout",
                "Hosted checkout keeps card data off our servers (no PCI scope).",
                "payments",
                "claude-code",
            )
            .unwrap();
        store
            .add_memory(
                &project,
                "error",
                "Duplicate index in the orders migration",
                "drizzle-kit failed; renamed the index to orders_customer_idx.",
                "",
                "user",
            )
            .unwrap();
        store
            .add_idea(Some(&project), "Export orders to CSV", "", "user")
            .unwrap();
        // Un plan en progreso (fase 2 de 3) y otro planificado en el backlog.
        use forge_core::ideas::{Phase, Task};
        let phase = |title: &str, status: &str, branch: Option<&str>, tasks: &[(&str, bool)]| {
            let mut p = Phase::new(title);
            p.status = status.into();
            p.branch = branch.map(str::to_string);
            p.tasks = tasks
                .iter()
                .map(|(text, done)| Task {
                    text: text.to_string(),
                    done: *done,
                })
                .collect();
            p
        };
        store
            .add_plan(
                Some(&project),
                "Email a receipt after payment",
                "Use the Stripe webhook, not the redirect.",
                &[
                    phase(
                        "Webhook and receipt model",
                        "done",
                        Some("feat/receipt-webhook"),
                        &[
                            ("Verify the Stripe signature", true),
                            ("Store receipts", true),
                        ],
                    ),
                    phase(
                        "Email template and sending",
                        "doing",
                        Some("feat/receipt-email"),
                        &[
                            ("React Email template", true),
                            ("Send through Resend", false),
                            ("Retry on failure", false),
                        ],
                    ),
                    phase(
                        "Tests and QA",
                        "pending",
                        None,
                        &[("Webhook e2e test", false)],
                    ),
                ],
                "claude-code",
            )
            .unwrap();
        store
            .add_plan(
                Some(&project),
                "Offline cart for mobile",
                "",
                &[
                    phase(
                        "Local storage sync",
                        "pending",
                        Some("feat/offline-cart"),
                        &[],
                    ),
                    phase("Conflict resolution", "pending", None, &[]),
                ],
                "codex",
            )
            .unwrap();
        let c = store
            .add_idea(Some(&project), "Coupon codes at checkout", "", "user")
            .unwrap();
        store
            .update_idea(Some(&project), c, Some("done"), None, None, "claude-code")
            .unwrap();
        store
            .add_idea(None, "Try the new Forge release on the laptop", "", "user")
            .unwrap();
        store
            .add_idea(None, "Side project: habit tracker", "", "user")
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
        "web",
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
    harness.state_mut().workspaces[0].memory.form = Some(crate::workspace::NoteForm {
        kind: "decision".into(),
        ..Default::default()
    });
    settle(&mut harness);
    save(&mut harness, &out, "3b-nueva-nota");
    harness.state_mut().workspaces[0].memory.form = None;

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
        reason: "rewrites remote history (git push --force)".into(),
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
        "# Acme Store\n\nOnline store with Stripe checkout.\n",
    )
    .unwrap();
    harness.state_mut().workspaces[0].ideas.open = false;
    harness.state_mut().workspaces[0].git.open = true;
    harness.state_mut().workspaces[0].git.dirty = true;
    settle(&mut harness);
    save(&mut harness, &out, "12-git");
    harness.state_mut().workspaces[0].git.open = false;
    harness.state_mut().welcome = Some(0);
    settle(&mut harness);
    save(&mut harness, &out, "13-bienvenida");
    harness.state_mut().welcome = None;
    harness.state_mut().settings_open = true;
    settle(&mut harness);
    save(&mut harness, &out, "11-ajustes");
    harness.state_mut().settings_open = false;
    *harness.state().update.lock().unwrap() = Some(super::update::Update::Available(
        forge_core::updates::Release {
            tag: "v0.2.0".into(),
            page: String::new(),
            dmg: None,
            checksum: None,
        },
    ));
    settle(&mut harness);
    save(&mut harness, &out, "14-actualizacion");
    *harness.state().update.lock().unwrap() = None;
    // Sugerencias de carpetas: la secuencia que manda el zsh de Forge al escribir `cd s`.
    harness.state_mut().workspaces[0].focus_first_shell();
    let line = format!("printf '\\e]7777;cd s\\x1f{}\\a'", project.display());
    harness.event(egui::Event::Text(line));
    harness.key_press(egui::Key::Enter);
    settle(&mut harness);
    save(&mut harness, &out, "15-sugerencias");
    // Panel maximizado con el desplegable de los demás abierto.
    harness.key_press_modifiers(
        egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND,
        egui::Key::Enter,
    );
    settle(&mut harness);
    let hidden = forge_core::tr!("Paneles ocultos ({n})", n = 2);
    harness.get_by_label(&hidden).click();
    settle(&mut harness);
    save(&mut harness, &out, "16-maximizado");
    harness.key_press(egui::Key::Escape);
    harness.key_press_modifiers(
        egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND,
        egui::Key::Enter,
    );
    settle(&mut harness);
    harness.state_mut().settings_open = false;
    harness.state_mut().workspaces[0].ideas.open = false;

    harness.state_mut().active = None;
    settle(&mut harness);
    save(&mut harness, &out, "4-inicio");

    // Muchos proyectos abiertos: lista compacta con dormidos.
    let ctx = harness.ctx.clone();
    for name in [
        "inventory-api",
        "landing-site",
        "mobile-app",
        "admin-panel",
        "billing-service",
        "design-system",
        "docs",
        "infra",
    ] {
        let p = home.join("Developer").join(name);
        std::fs::create_dir_all(&p).unwrap();
        harness.state_mut().open_project_as(&ctx, &p, false);
    }
    harness.state_mut().active = Some(0);
    settle(&mut harness);
    save(&mut harness, &out, "9-muchos-proyectos");
}
