// Tests del motor de Guard.
use super::*;

mod unit {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("package-lock.json", "web/package-lock.json"));
        assert!(glob_match("*.generated.ts", "src/api/types.generated.ts"));
        assert!(glob_match(
            "drizzle/meta/**",
            "drizzle/meta/0001_snapshot.json"
        ));
        assert!(!glob_match("drizzle/meta/**", "src/drizzle/meta/x.json"));
        assert!(glob_match("**/meta/*.json", "src/drizzle/meta/x.json"));
        assert!(glob_match("coverage/", "coverage/lcov/index.html"));
        assert!(glob_match(".env.*", "apps/api/.env.local"));
        assert!(!glob_match("*.key", "src/key.ts"));
        assert!(glob_match("id_rs?", "id_rsa"));
    }

    #[test]
    fn parses_git_output() {
        let files = parse_numstat("10\t2\tsrc/a.ts\0-\t-\tlogo.png\0");
        assert_eq!(
            files[0],
            FileChange {
                path: "src/a.ts".into(),
                added: 10,
                deleted: 2,
                binary: false,
                removed: false
            }
        );
        assert!(files[1].binary);

        let patch = "diff --git a/x b/x\n--- a/x\n+++ b/src/x.ts\n@@ -3,0 +4,2 @@\n+const a = 1;\n+const b = 2;\n@@ -10 +12 @@\n-old\n+new\n";
        let lines = parse_added_lines(patch);
        assert_eq!(
            lines,
            vec![
                ("src/x.ts".into(), 4, "const a = 1;".into()),
                ("src/x.ts".into(), 5, "const b = 2;".into()),
                ("src/x.ts".into(), 12, "new".into()),
            ]
        );
    }

    #[test]
    fn detects_and_masks_secrets() {
        let line = |t: &str| ("a.ts".to_string(), 1, t.to_string());
        let found = scan_secrets(&[
            line("const k = 'AKIAIOSFODNN7EXAMPLE';"), // forge:allow-secret
            line("-----BEGIN OPENSSH PRIVATE KEY-----"), // forge:allow-secret
            line("ANTHROPIC_API_KEY=sk-ant-api03-abcdefghijklmnopqrstuvwxyz"), // forge:allow-secret
            line(r#"password: "hunter2hunter2""#),     // forge:allow-secret
            line("const password = process.env.DB_PASSWORD;"),
            line("const apiKey = getKey();"),
        ]);
        let kinds: Vec<&str> = found.iter().map(|f| f.kind).collect();
        assert_eq!(
            kinds,
            [
                "clave de AWS",
                "clave privada",
                "clave de Anthropic",
                "contraseña o clave en el código"
            ]
        );
        assert!(
            found
                .iter()
                .all(|f| f.preview.ends_with("••••") && f.preview.chars().count() <= 10)
        );
        let allowed = format!("const k = 'AKIAIOSFODNN7EXAMPLE'; // {ALLOW_SECRET}"); // forge:allow-secret
        assert!(scan_secrets(&[line(&allowed)]).is_empty());
    }

    fn rules(toml_text: &str) -> Rules {
        toml::from_str(toml_text).unwrap()
    }

    #[test]
    fn evaluation_levels() {
        let r = rules(
            "[quality]\nmax_changed_lines = 100\nwarning_changed_lines = 50\n[commit]\nrequire_tests = true\nrequire_ai_review = true\n",
        );
        let file = |path: &str, added| FileChange {
            path: path.into(),
            added,
            deleted: 0,
            binary: false,
            removed: false,
        };
        let diff = Diff {
            files: vec![
                file("src/a.ts", 60),
                file("package-lock.json", 5000),
                file(".env", 1),
                file(".env.example", 1),
            ],
            added_lines: vec![(
                "src/a.ts".into(),
                3,
                "token = 'ghp_abcdefghijklmnopqrstuvwxyz0123456789'".into(), // forge:allow-secret
            )],
            ..Default::default()
        };
        let items = evaluate(&r, Stage::Commit, &diff);
        let level = |prefix: &str| {
            items
                .iter()
                .find(|i| i.label.contains(prefix))
                .map(|i| i.level)
        };
        assert_eq!(level("líneas"), Some(Level::Warn)); // 62 > 50, el lockfile no cuenta
        assert_eq!(level("prohibidos"), Some(Level::Block)); // .env sí, .env.example no
        assert_eq!(
            items
                .iter()
                .find(|i| i.label.contains("prohibidos"))
                .unwrap()
                .details,
            vec![".env"]
        );
        assert_eq!(level("secretos"), Some(Level::Block));
        assert_eq!(level("require_tests"), Some(Level::Block)); // falta [[checks]] tests
        assert_eq!(level("Revisión de IA"), Some(Level::Pending));
        assert_eq!(thousands(1284), "1.284");
        assert_eq!(thousands(4000000), "4.000.000");
    }

    #[test]
    fn template_round_trips() {
        for (file, content, expected) in [
            ("Cargo.toml", "[package]\nname = \"x\"\n", 4),
            (
                "package.json",
                r#"{"scripts":{"lint":"eslint .","test":"vitest","dev":"vite"}}"#,
                2,
            ),
        ] {
            let dir =
                std::env::temp_dir().join(format!("forge-rules-{}-{file}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(file), content).unwrap();
            write_template(&dir).unwrap();
            let rules = Rules::load(&dir).unwrap().unwrap();
            assert_eq!(rules.checks.len(), expected, "{file}");
            let (checks, missing) = rules.checks_for(Stage::Commit);
            assert!(missing.is_empty(), "{file}: faltan {missing:?}");
            assert!(checks.iter().any(|c| c.id == "tests"));
            assert!(write_template(&dir).is_err(), "no debe sobrescribir");
        }
    }

    fn wait(handle: &Handle) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !handle.snapshot(GuardRun::finished) {
            assert!(Instant::now() < deadline, "el guard no terminó");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Fase 7: los checks prueban el candidato exacto y la evidencia solo vale para él.
    #[test]
    fn checks_run_on_frozen_candidate_and_reuse_evidence() {
        let dir = std::env::temp_dir().join(format!("forge-frozen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let sh = |cmd: &str| {
            assert!(
                Command::new("sh")
                    .args(["-c", cmd])
                    .current_dir(&dir)
                    .status()
                    .unwrap()
                    .success(),
                "{cmd}"
            )
        };
        sh("git init -q -b main && git config user.email t@t && git config user.name t");
        sh("echo v0 > a.txt && git add . && git commit -qm inicio");
        sh("echo v1 > a.txt && git add a.txt && echo 'v2 sin preparar' > a.txt");
        let counter = dir
            .parent()
            .unwrap()
            .join(format!("forge-frozen-count-{}", std::process::id()));
        let _ = std::fs::remove_file(&counter);
        let rules = |_: &Path| -> Rules {
            toml::from_str(&format!(
                "[commit]\nrequire_tests = true\n[[checks]]\nid = \"tests\"\ncommand = \"echo x >> {} && grep -q v1 a.txt\"\n",
                counter.display()
            ))
            .unwrap()
        };
        let options = |evidence| RunOptions {
            env: Default::default(),
            exceptions: vec![],
            push_range: None,
            evidence,
            name: "frozen".into(),
            routing: None,
            month_spent: 0.0,
            reviewers: vec![],
        };

        // 1. El árbol de trabajo tiene v2, pero el commit lleva v1: el check debe ver v1.
        let first = run(
            &dir,
            &dir,
            rules(&dir),
            Stage::Commit,
            options(vec![]),
            || {},
        );
        wait(&first);
        let evidence = first.snapshot(|r| {
            assert!(
                r.frozen.as_ref().is_some_and(|c| c.isolated),
                "debe aislarse"
            );
            assert_eq!(
                r.checks[0].state,
                CheckState::Passed,
                "salida: {}",
                r.checks[0].output
            );
            r.new_evidence()
        });
        assert_eq!(evidence.len(), 1);
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "v2 sin preparar\n",
            "no se toca el árbol"
        );

        // 2. Mismo candidato: se reutiliza la evidencia y el check no se vuelve a ejecutar.
        let second = run(
            &dir,
            &dir,
            rules(&dir),
            Stage::Commit,
            options(evidence.clone()),
            || {},
        );
        wait(&second);
        second.snapshot(|r| {
            assert!(r.checks[0].reused.is_some(), "debía reutilizar");
            assert_eq!(r.verdict(), Verdict::Ready);
        });
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            1,
            "se ejecutó de nuevo"
        );

        // 3. Otro contenido preparado = otro candidato: la evidencia vieja no sirve.
        sh("echo v3 > a.txt && git add a.txt");
        let third = run(
            &dir,
            &dir,
            rules(&dir),
            Stage::Commit,
            options(evidence),
            || {},
        );
        wait(&third);
        third.snapshot(|r| {
            assert!(r.checks[0].reused.is_none(), "evidencia de otra versión");
            assert!(
                matches!(r.checks[0].state, CheckState::Failed(_)),
                "v3 no contiene v1"
            );
        });
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            2
        );
    }

    #[test]
    fn reviewers_disagreement() {
        let o = |name: &str, verdict| (name.to_string(), verdict, 0.9, String::new());
        let (level, label, lines) = compare_reviewers(&[
            o("BD", AiVerdict::Approve),
            o("Seguridad", AiVerdict::Block),
        ]);
        assert_eq!(
            (level, label.contains("no coinciden"), lines.len()),
            (Level::Warn, true, 2)
        );
        let (level, _, _) =
            compare_reviewers(&[o("BD", AiVerdict::Block), o("UI", AiVerdict::Changes)]);
        assert_eq!(level, Level::Info);
    }

    /// Repo real: diff preparado, checks en paralelo, fallo, timeout y veredicto.
    #[test]
    fn end_to_end_on_real_repo() {
        let dir = std::env::temp_dir().join(format!("forge-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".forge")).unwrap();
        let sh = |cmd: &str| {
            assert!(
                Command::new("sh")
                    .args(["-c", cmd])
                    .current_dir(&dir)
                    .status()
                    .unwrap()
                    .success(),
                "{cmd}"
            )
        };
        sh("git init -q && git config user.email t@t && git config user.name t");
        std::fs::write(dir.join("a.txt"), "uno\n").unwrap();
        sh("git add a.txt && git commit -qm inicio");
        std::fs::write(dir.join("a.txt"), "uno\ndos\ntres\n").unwrap();
        std::fs::write(dir.join("nuevo.txt"), "x\n").unwrap();
        sh("git add a.txt");

        let r = rules(
            "[commit]\nrequire_lint = true\nrequire_tests = true\n\
             [[checks]]\nid = \"lint\"\ncommand = \"echo lint ok\"\n\
             [[checks]]\nid = \"tests\"\ncommand = \"echo fallo de prueba; exit 4\"\n\
             [[checks]]\nid = \"lento\"\ncommand = \"sleep 30\"\ntimeout_seconds = 1\nseverity = \"warning\"\nstages = [\"commit\"]\n",
        );
        let diff = collect_diff(&dir, Stage::Commit, &r, None).unwrap();
        assert!(!diff.unstaged);
        assert_eq!(diff.files.len(), 1, "solo lo preparado: {:?}", diff.files);
        assert_eq!((diff.files[0].added, diff.files[0].deleted), (2, 0));

        let handle = run(
            &dir,
            &dir,
            r,
            Stage::Commit,
            RunOptions {
                env: Default::default(),
                exceptions: vec![],
                push_range: None,
                evidence: vec![],
                name: "test".into(),
                routing: None,
                month_spent: 0.0,
                reviewers: vec![],
            },
            || {},
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        while !handle.snapshot(GuardRun::finished) {
            assert!(Instant::now() < deadline, "el guard no terminó");
            std::thread::sleep(Duration::from_millis(50));
        }
        handle.snapshot(|run| {
            let state = |id: &str| {
                run.checks
                    .iter()
                    .find(|c| c.check.id == id)
                    .unwrap()
                    .state
                    .clone()
            };
            assert_eq!(state("lint"), CheckState::Passed);
            assert_eq!(state("tests"), CheckState::Failed(4));
            assert_eq!(state("lento"), CheckState::TimedOut);
            let tests = run.checks.iter().find(|c| c.check.id == "tests").unwrap();
            assert!(tests.output.contains("fallo de prueba"));
            assert_eq!(run.verdict(), Verdict::Blocked);
        });

        // Nada preparado: evalúa todo el árbol de trabajo, incluidos los archivos nuevos.
        sh("git reset -q");
        let diff = collect_diff(&dir, Stage::Commit, &Rules::default(), None).unwrap();
        assert!(diff.unstaged);
        let mut paths: Vec<&str> = diff.files.iter().map(|f| f.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, ["a.txt", "nuevo.txt"]);
    }
}

mod dogfood {
    use super::*;

    /// Guard sobre el propio repositorio de Forge (manual):
    /// `cargo test --release dogfood -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn guard_on_forge_itself() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let rules: Rules = toml::from_str(&template(&detect_checks(root))).unwrap();
        // Directorio de compilación aparte para no bloquear el `cargo test` que nos ejecuta.
        let target = std::env::temp_dir().join("forge-dogfood-target");
        let handle = run(
            root,
            root,
            rules,
            Stage::Commit,
            RunOptions {
                env: Default::default(),
                exceptions: vec![],
                push_range: None,
                evidence: vec![],
                name: "test".into(),
                routing: None,
                month_spent: 0.0,
                reviewers: vec![],
            },
            || {},
        );
        unsafe { std::env::set_var("CARGO_TARGET_DIR", &target) };
        while !handle.snapshot(GuardRun::finished) {
            std::thread::sleep(Duration::from_millis(200));
        }
        handle.snapshot(|r| {
            for i in &r.items {
                println!("{:?} {} {:?}", i.level, i.label, i.details);
            }
            for c in &r.checks {
                println!(
                    "{:?} {} {:?}\n{}",
                    c.state,
                    c.check.id,
                    c.duration,
                    tail(&c.output, 15)
                );
            }
            println!("VEREDICTO: {:?} {:?}", r.verdict(), r.progress());
        });
    }
}
