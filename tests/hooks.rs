// Fase 6: un commit o un push se bloquean aunque se hagan fuera de Forge (git directo).
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FORGE: &str = env!("CARGO_BIN_EXE_forge");

struct Repo {
    dir: PathBuf,
    db: PathBuf,
}

impl Repo {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("forge-it-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("repo");
        std::fs::create_dir_all(dir.join(".forge")).unwrap();
        let repo = Self {
            dir: dir.canonicalize().unwrap(),
            db: base.join("forge.db"),
        };
        repo.ok("git", &["init", "-q", "-b", "main"]);
        repo.ok("git", &["config", "user.email", "t@t"]);
        repo.ok("git", &["config", "user.name", "Tester"]);
        repo.write("README.md", "hola\n");
        repo.ok("git", &["add", "."]);
        repo.ok("git", &["commit", "-qm", "inicio"]);
        repo
    }

    fn write(&self, path: &str, content: &str) {
        std::fs::write(self.dir.join(path), content).unwrap();
    }

    fn run(&self, program: &str, args: &[&str]) -> Output {
        Command::new(program)
            .args(args)
            .current_dir(&self.dir)
            // Base de datos propia: los tests no tocan la de la app.
            .env("FORGE_DB", &self.db)
            .output()
            .unwrap()
    }

    fn ok(&self, program: &str, args: &[&str]) -> String {
        let out = self.run(program, args);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "{program} {args:?} falló:\n{text}");
        text
    }

    fn fails(&self, program: &str, args: &[&str]) -> String {
        let out = self.run(program, args);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !out.status.success(),
            "{program} {args:?} debía fallar:\n{text}"
        );
        text
    }

    fn forge(&self, args: &[&str]) -> Output {
        let mut all = args.to_vec();
        let project = self.dir.to_string_lossy().into_owned();
        all.extend(["--project", &project]);
        self.run(FORGE, &all)
    }

    fn hook(&self, name: &str) -> PathBuf {
        self.dir.join(".git/hooks").join(name)
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn commit_blocked_outside_forge_and_exception_is_scoped() {
    // SAFETY: todos los tests fijan el mismo valor; los textos esperados están en español.
    unsafe { std::env::set_var("FORGE_LANG", "es") };
    let repo = Repo::new("commit");
    repo.write(
        ".forge/rules.toml",
        "[commit]\nrequire_tests = true\n[[checks]]\nid = \"tests\"\ncommand = \"test -f ok.txt\"\n",
    );
    // Hook previo del usuario: debe conservarse y ejecutarse antes.
    std::fs::create_dir_all(repo.dir.join(".git/hooks")).unwrap();
    std::fs::write(
        repo.hook("pre-commit"),
        "#!/bin/sh\ntouch \"$(git rev-parse --show-toplevel)/../hook-previo\"\n",
    )
    .unwrap();
    repo.ok("chmod", &["+x", ".git/hooks/pre-commit"]);

    let install = repo.forge(&["hooks", "install"]);
    assert!(install.status.success(), "{}", text(&install));
    assert!(text(&install).contains("se conserva"));

    // 1. Commit con git directo: bloqueado por el check de tests.
    repo.write("a.txt", "uno\n");
    repo.ok("git", &["add", "a.txt"]);
    let blocked = repo.fails("git", &["commit", "-m", "cambio"]);
    assert!(blocked.contains("BLOQUEADO"), "{blocked}");
    assert!(
        blocked.contains("forge guard allow check:tests"),
        "{blocked}"
    );
    assert!(
        repo.dir.join("../hook-previo").exists(),
        "el hook previo no se ejecutó"
    );

    // 2. Excepción con motivo para este commit: ahora pasa, y queda visible.
    let missing_reason = repo.forge(&["guard", "allow", "check:tests"]);
    assert!(
        !missing_reason.status.success(),
        "una excepción sin motivo no debe aceptarse"
    );
    let allow = repo.forge(&[
        "guard",
        "allow",
        "check:tests",
        "--reason",
        "tests en reparación",
    ]);
    assert!(allow.status.success(), "{}", text(&allow));
    let passed = repo.ok("git", &["commit", "-m", "cambio"]);
    assert!(
        passed.contains("tests en reparación"),
        "la omisión debe mostrarse: {passed}"
    );

    // 3. Otro cambio = otro candidato: la excepción ya no vale.
    repo.write("b.txt", "dos\n");
    repo.ok("git", &["add", "b.txt"]);
    repo.fails("git", &["commit", "-m", "otro"]);
    // Sin preparar no cuenta: el commit no lo incluiría (Fase 7: se prueba el candidato).
    repo.write("ok.txt", "");
    repo.fails("git", &["commit", "-m", "otro"]);
    repo.ok("git", &["add", "ok.txt"]);
    repo.ok("git", &["commit", "-m", "otro"]);

    // 4. Historial y desinstalación: vuelve el hook previo.
    let history = text(&repo.forge(&["guard", "exceptions"]));
    assert!(
        history.contains("check:tests") && history.contains("Tester"),
        "{history}"
    );
    assert!(repo.forge(&["hooks", "uninstall"]).status.success());
    let restored = std::fs::read_to_string(repo.hook("pre-commit")).unwrap();
    assert!(restored.contains("hook-previo") && !restored.contains("forge-managed-hook"));
    assert!(
        !Path::new(&format!(
            "{}.forge-backup",
            repo.hook("pre-commit").display()
        ))
        .exists()
    );
}

#[test]
fn secrets_and_push_are_blocked() {
    // SAFETY: todos los tests fijan el mismo valor; los textos esperados están en español.
    unsafe { std::env::set_var("FORGE_LANG", "es") };
    let repo = Repo::new("push");
    repo.write(
        ".forge/rules.toml",
        "[push]\nrequire_build = true\n[[checks]]\nid = \"build\"\ncommand = \"exit 3\"\n",
    );
    let remote = repo.dir.parent().unwrap().join("remote.git");
    repo.ok("git", &["init", "-q", "--bare", &remote.to_string_lossy()]);
    repo.ok(
        "git",
        &["remote", "add", "origin", &remote.to_string_lossy()],
    );
    repo.ok("git", &["push", "-q", "-u", "origin", "main"]);
    assert!(repo.forge(&["hooks", "install"]).status.success());

    // Secreto en el diff: el commit se bloquea (sin mostrar el valor completo).
    let key = ["AKIA", "IOSFODNN7EXAMPLE"].concat();
    repo.write("config.js", &format!("const k = '{key}';\n"));
    repo.ok("git", &["add", "config.js"]);
    let blocked = repo.fails("git", &["commit", "-m", "config"]);
    assert!(
        blocked.contains("posibles secretos") && !blocked.contains(&key),
        "{blocked}"
    );
    repo.write("config.js", "const k = process.env.KEY;\n");
    repo.ok("git", &["add", "config.js"]);
    repo.ok("git", &["commit", "-m", "config"]);

    // Push: el build exigido falla → no se envía.
    let push = repo.fails("git", &["push", "origin", "main"]);
    assert!(
        push.contains("BLOQUEADO") && push.contains("push"),
        "{push}"
    );
    let pushed = repo.run("git", &["ls-remote", "origin", "main"]);
    let local = repo.ok("git", &["rev-parse", "HEAD"]);
    assert!(
        !String::from_utf8_lossy(&pushed.stdout).contains(local.trim()),
        "el commit no debía llegar al remoto"
    );
}

/// Fase 7: validar una vez y hacer commit reutiliza la evidencia del mismo candidato,
/// y los checks prueban lo que se commitea aunque haya cambios sin preparar.
#[test]
fn hook_reuses_evidence_of_same_candidate() {
    // SAFETY: todos los tests fijan el mismo valor; los textos esperados están en español.
    unsafe { std::env::set_var("FORGE_LANG", "es") };
    let repo = Repo::new("evidence");
    let counter = repo.dir.parent().unwrap().join("runs");
    repo.write(
        ".forge/rules.toml",
        &format!(
            "[commit]\nrequire_tests = true\n[[checks]]\nid = \"tests\"\ncommand = \"echo x >> '{}' && grep -q listo estado.txt\"\n",
            counter.display()
        ),
    );
    assert!(repo.forge(&["hooks", "install"]).status.success());

    // Se prepara "listo" y se deja otro contenido sin preparar.
    repo.write("estado.txt", "listo\n");
    repo.ok("git", &["add", "estado.txt"]);
    repo.write("estado.txt", "a medias\n");

    let validate = repo.forge(&["guard", "commit"]);
    let out = text(&validate);
    assert!(validate.status.success(), "{out}");
    assert!(out.contains("copia aislada"), "{out}");

    let commit = repo.ok("git", &["commit", "-m", "listo"]);
    assert!(commit.contains("evidencia reutilizada"), "{commit}");
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap().lines().count(),
        1,
        "los tests se repitieron"
    );

    // Historial y reporte.
    let history = text(&repo.forge(&["guard", "history"]));
    assert_eq!(history.lines().count(), 2, "{history}");
    let report = text(&repo.forge(&["guard", "report"]));
    assert!(
        report.contains("# Forge Guard") && report.contains("evidencia del"),
        "{report}"
    );
    assert_eq!(
        std::fs::read_to_string(repo.dir.join("estado.txt")).unwrap(),
        "a medias\n"
    );
}

/// Fase 9: revisión con IA en el hook, con proveedores simulados (sin gastar).
#[test]
fn ai_review_in_hook_with_fake_providers() {
    // SAFETY: todos los tests fijan el mismo valor; los textos esperados están en español.
    unsafe { std::env::set_var("FORGE_LANG", "es") };
    let repo = Repo::new("ai");
    let verdict_file = repo.dir.parent().unwrap().join("veredicto");
    std::fs::write(&verdict_file, "approve").unwrap();
    let provider = format!(
        "cat > /dev/null; v=$(cat '{}'); echo \"{{\\\"verdict\\\":\\\"$v\\\",\\\"confidence\\\":0.9,\\\"summary\\\":\\\"revisado\\\",\\\"findings\\\":[]}}\"",
        verdict_file.display()
    );
    repo.write(".forge/rules.toml", "[commit]\nrequire_ai_review = true\n");
    repo.write(
        ".forge/routing.toml",
        &format!(
            "[routing]\nprefer_local = false\nmonthly_budget_usd = 5.0\nmax_cost_per_call_usd = 0.1\n\
             [routing.providers.falso]\ncommand = '''{provider}'''\ncost_per_call_usd = 0.02\n\
             [routing.tasks.classification]\nprovider = \"falso\"\nmodel = \"m\"\n\
             [routing.tasks.code_review]\nprovider = \"falso\"\nmodel = \"m\"\n\
             [routing.tasks.architecture]\nprovider = \"falso\"\nmodel = \"m\"\n\
             [routing.tasks.security]\nprovider = \"falso\"\nmodel = \"m\"\n"
        ),
    );
    assert!(repo.forge(&["hooks", "install"]).status.success());

    repo.write("a.txt", "uno\n");
    repo.ok("git", &["add", "a.txt"]);
    let out = repo.ok("git", &["commit", "-m", "con ia"]);
    assert!(out.contains("Revisión de IA: aprobada"), "{out}");

    // El revisor bloquea → el commit no se crea.
    std::fs::write(&verdict_file, "block").unwrap();
    repo.write("b.txt", "dos\n");
    repo.ok("git", &["add", "b.txt"]);
    let blocked = repo.fails("git", &["commit", "-m", "bloqueado"]);
    assert!(
        blocked.contains("Revisión de IA: bloquea")
            && blocked.contains("forge guard allow ai-review"),
        "{blocked}"
    );

    let usage = text(&repo.forge(&["ai", "usage"]));
    assert!(
        usage.contains("falso") && usage.contains("Total de pago: $0.08"),
        "{usage}"
    );
    let route = text(&repo.forge(&["ai", "route"]));
    assert!(
        route.contains("Riesgo bajo") && route.contains("falso m"),
        "{route}"
    );
}

/// Fase 11: revisores especializados activados por rutas, bloqueantes o informativos.
#[test]
fn specialist_reviewers_by_path() {
    // SAFETY: todos los tests fijan el mismo valor; los textos esperados están en español.
    unsafe { std::env::set_var("FORGE_LANG", "es") };
    let repo = Repo::new("reviewers");
    let dir = repo.dir.parent().unwrap().to_path_buf();
    // Proveedor simulado: guarda el prompt y responde el veredicto que se le indica.
    let fake = |name: &str, verdict: &str| {
        format!(
            "[routing.providers.{name}]\ncommand = '''cat > '{}/prompt-{name}'; echo '{{\"verdict\":\"{verdict}\",\"confidence\":0.9,\"summary\":\"{name} dice {verdict}\",\"findings\":[]}}' '''\ncost_per_call_usd = 0.01\n",
            dir.display()
        )
    };
    repo.write(".forge/rules.toml", "[commit]\nrequire_ai_review = true\n");
    repo.write(
        ".forge/routing.toml",
        &format!(
            "[routing]\nprefer_local = false\n\
             [routing.tasks.classification]\nprovider = \"general\"\nmodel = \"m\"\n\
             [routing.tasks.code_review]\nprovider = \"general\"\nmodel = \"m\"\n\
             [routing.tasks.architecture]\nprovider = \"general\"\nmodel = \"m\"\n\
             [routing.tasks.security]\nprovider = \"general\"\nmodel = \"m\"\n{}{}{}",
            fake("general", "approve"),
            fake("bd", "block"),
            fake("ui", "block")
        ),
    );
    repo.write(
        ".forge/agents.toml",
        "[[reviewers]]\nid = \"database\"\nprovider = \"bd\"\n[[reviewers]]\nid = \"frontend\"\nprovider = \"ui\"\n",
    );
    assert!(repo.forge(&["hooks", "install"]).status.success());

    std::fs::create_dir_all(repo.dir.join("drizzle")).unwrap();
    std::fs::create_dir_all(repo.dir.join("src")).unwrap();
    repo.write(
        "drizzle/0001_lotes.sql",
        "ALTER TABLE vacunas DROP COLUMN lote;\n",
    );
    repo.write("src/App.tsx", "export const App = () => <div>Hola</div>;\n");
    repo.ok("git", &["add", "drizzle", "src"]);

    let blocked = repo.fails("git", &["commit", "-m", "lotes"]);
    assert!(
        blocked.contains("Revisor de base de datos: bloquea"),
        "{blocked}"
    );
    assert!(
        blocked.contains("Revisor de frontend: objeta (informativo)"),
        "{blocked}"
    );
    assert!(
        blocked.contains("revisores no activados") || blocked.contains("BLOQUEADO"),
        "{blocked}"
    );
    assert!(
        blocked.contains("forge guard allow reviewer:database"),
        "{blocked}"
    );

    // Cada revisor recibe solo su área.
    let db_prompt = std::fs::read_to_string(dir.join("prompt-bd")).unwrap();
    assert!(
        db_prompt.contains("DROP COLUMN lote") && !db_prompt.contains("<div>Hola</div>"),
        "{db_prompt}"
    );
    let ui_prompt = std::fs::read_to_string(dir.join("prompt-ui")).unwrap();
    assert!(ui_prompt.contains("<div>Hola</div>") && !ui_prompt.contains("DROP COLUMN"));

    let route = text(&repo.forge(&["ai", "route"]));
    assert!(
        route.contains("Revisor de base de datos (bloqueante)")
            && route.contains("○ Revisor de seguridad"),
        "{route}"
    );

    // Con motivo registrado, el revisor bloqueante se puede omitir para este commit.
    let allow = repo.forge(&[
        "guard",
        "allow",
        "reviewer:database",
        "--reason",
        "migración aprobada por el DBA",
    ]);
    assert!(allow.status.success(), "{}", text(&allow));
    let passed = repo.ok("git", &["commit", "-m", "lotes"]);
    assert!(
        passed.contains("migración aprobada por el DBA")
            && passed.contains("Comparación de revisores: coinciden"),
        "{passed}"
    );
}
