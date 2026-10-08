// Ejecución: checks en paralelo, evidencias, revisión con IA y revisores.
use super::*;
use crate::tr;

// ---------------------------------------------------------------- ejecución

#[derive(Clone, Debug, PartialEq)]
pub enum CheckState {
    Waiting,
    Running,
    Passed,
    Failed(i32),
    TimedOut,
    Cancelled,
    Error(String),
}

#[derive(Clone, Debug)]
pub struct CheckRun {
    pub check: Check,
    pub state: CheckState,
    /// Últimas líneas de la salida (sin colores).
    pub output: String,
    pub started: Option<Instant>,
    pub duration: Option<Duration>,
    /// No se ejecutó: había evidencia de este mismo candidato (marca de tiempo).
    pub reused: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Verdict {
    Ready,
    Warnings,
    Blocked,
    Running,
    Pending,
    NothingToCheck,
}

#[derive(Debug)]
pub struct GuardRun {
    pub stage: Stage,
    pub started_at: SystemTime,
    pub items: Vec<Item>,
    pub checks: Vec<CheckRun>,
    pub diff_command: String,
    pub unstaged: bool,
    pub analyzing: bool,
    pub error: Option<String>,
    pub candidate: String,
    pub branch: String,
    /// Excepciones válidas para este candidato.
    pub exceptions: Vec<Exception>,
    /// Candidato congelado (árbol exacto evaluado).
    pub frozen: Option<Candidate>,
    pub finished_at: Option<SystemTime>,
    pub project_name: String,
    /// Consumo de modelos de esta ejecución (la app/CLI lo guarda).
    pub usage: Vec<Usage>,
    /// Evidencias que no son checks (aprobaciones de IA del candidato).
    pub extra_evidence: Vec<Evidence>,
}

pub(crate) fn unix(t: SystemTime) -> i64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl GuardRun {
    fn check_status(c: &CheckRun) -> String {
        match &c.state {
            CheckState::Waiting => tr!("en espera").into(),
            CheckState::Running => tr!("ejecutándose").into(),
            CheckState::Passed if c.reused.is_some() => tr!("pasó (evidencia reutilizada)").into(),
            CheckState::Passed => tr!("pasó").into(),
            CheckState::Failed(code) => tr!("código {code}", code = code),
            CheckState::TimedOut => format!("tiempo agotado ({} s)", c.check.timeout_seconds),
            CheckState::Cancelled => tr!("cancelado").into(),
            CheckState::Error(e) => e.clone(),
        }
    }

    /// Reporte serializable (historial y exportación).
    pub fn report(&self) -> Report {
        Report {
            project: self.project_name.clone(),
            stage: self.stage,
            candidate: self.frozen.clone(),
            branch: self.branch.clone(),
            started_at: unix(self.started_at),
            finished_at: self
                .finished_at
                .map_or_else(|| unix(SystemTime::now()), unix),
            verdict: self.verdict(),
            progress: self.progress(),
            items: self
                .items
                .iter()
                .map(|i| ReportItem {
                    id: i.id.clone(),
                    label: i.label.clone(),
                    level: i.level,
                    details: i.details.clone(),
                })
                .collect(),
            checks: self
                .checks
                .iter()
                .map(|c| ReportCheck {
                    id: c.check.id.clone(),
                    name: c.check.name.clone().unwrap_or_else(|| c.check.id.clone()),
                    command: c.check.command.clone(),
                    level: self.check_level(c).unwrap_or(Level::Pending),
                    status: Self::check_status(c),
                    duration_ms: c.duration.map_or(0, |d| d.as_millis() as u64),
                    output: c.output.clone(),
                    reused_at: c.reused,
                })
                .collect(),
            error: self.error.clone(),
        }
    }

    /// Checks ejecutados ahora que pasaron: evidencia nueva para guardar.
    pub fn new_evidence(&self) -> Vec<Evidence> {
        let Some(frozen) = &self.frozen else {
            return Vec::new();
        };
        let now = unix(SystemTime::now());
        let extra = self.extra_evidence.iter().cloned();
        self.checks
            .iter()
            .filter(|c| c.state == CheckState::Passed && c.reused.is_none())
            .map(|c| Evidence {
                check_id: c.check.id.clone(),
                command: c.check.command.clone(),
                tree: frozen.tree.clone(),
                duration_ms: c.duration.map_or(0, |d| d.as_millis() as u64),
                output: c.output.clone(),
                created_at: now,
            })
            .chain(extra)
            .collect()
    }

    pub fn check_rule(c: &CheckRun) -> String {
        format!("check:{}", c.check.id)
    }

    pub fn exception_for(&self, rule: &str) -> Option<&Exception> {
        self.exceptions.iter().find(|e| e.rule == rule)
    }

    pub fn check_level(&self, c: &CheckRun) -> Option<Level> {
        let bad = match c.check.severity {
            Severity::Blocking => Level::Block,
            Severity::Warning => Level::Warn,
        };
        let bad = if self.exception_for(&Self::check_rule(c)).is_some() {
            Level::Excepted
        } else {
            bad
        };
        match &c.state {
            CheckState::Waiting | CheckState::Running => None,
            CheckState::Passed => Some(Level::Pass),
            CheckState::Cancelled => Some(Level::Pending),
            _ => Some(bad),
        }
    }

    /// Reglas que bloquean ahora mismo (para ofrecer excepciones).
    pub fn blocking_rules(&self) -> Vec<String> {
        let items = self
            .items
            .iter()
            .filter(|i| i.level == Level::Block)
            .map(|i| i.id.clone());
        let checks = self
            .checks
            .iter()
            .filter(|c| self.check_level(c) == Some(Level::Block))
            .map(Self::check_rule);
        items.chain(checks).collect()
    }

    pub fn verdict(&self) -> Verdict {
        if self.error.is_some() {
            return Verdict::Blocked;
        }
        if self.analyzing {
            return Verdict::Running;
        }
        let levels: Vec<Option<Level>> = self
            .items
            .iter()
            .filter(|i| i.level != Level::Info)
            .map(|i| Some(i.level))
            .chain(self.checks.iter().map(|c| self.check_level(c)))
            .collect();
        if self
            .items
            .first()
            .is_some_and(|i| i.label.starts_with("0 archivos"))
            && self.checks.is_empty()
        {
            return Verdict::NothingToCheck;
        }
        if levels.contains(&Some(Level::Block)) {
            Verdict::Blocked
        } else if levels.contains(&None) {
            Verdict::Running
        } else if levels.contains(&Some(Level::Pending)) {
            Verdict::Pending
        } else if levels.contains(&Some(Level::Warn)) {
            Verdict::Warnings
        } else {
            Verdict::Ready
        }
    }

    /// Sin análisis ni checks en curso.
    pub fn finished(&self) -> bool {
        self.finished_at.is_some()
            && !self.analyzing
            && !self
                .checks
                .iter()
                .any(|c| matches!(c.state, CheckState::Waiting | CheckState::Running))
    }

    /// (completados, total) de los controles con resultado.
    pub fn progress(&self) -> (usize, usize) {
        let items: Vec<&Item> = self
            .items
            .iter()
            .filter(|i| i.level != Level::Info)
            .collect();
        let done_items = items.iter().filter(|i| i.level != Level::Pending).count();
        let done_checks = self
            .checks
            .iter()
            .filter(|c| self.check_level(c).is_some_and(|l| l != Level::Pending))
            .count();
        (done_items + done_checks, items.len() + self.checks.len())
    }
}

/// Ejecución en curso. Al soltarla se cancela (se matan los checks).
pub struct Handle {
    pub state: Arc<Mutex<GuardRun>>,
    cancel: Arc<AtomicBool>,
}

impl Handle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn snapshot<R>(&self, f: impl FnOnce(&GuardRun) -> R) -> R {
        f(&self.state.lock().unwrap())
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Analiza el diff y ejecuta en paralelo los checks de la etapa. `notify` se llama
/// en cada avance (la app repinta; la CLI podrá imprimir).
pub struct RunOptions {
    pub env: std::collections::HashMap<String, String>,
    /// Excepciones del proyecto; se filtran por candidato, rama y etapa.
    pub exceptions: Vec<Exception>,
    pub push_range: Option<String>,
    /// Evidencias previas del proyecto (se usan solo las del mismo árbol y comando).
    pub evidence: Vec<Evidence>,
    /// Nombre del proyecto (para el id del candidato y el reporte).
    pub name: String,
    /// Router de modelos para las revisiones con IA (None: quedan pendientes).
    pub routing: Option<RouterConfig>,
    /// Gasto del mes en proveedores de pago (para respetar el presupuesto).
    pub month_spent: f64,
    /// Revisores especializados del proyecto.
    pub reviewers: Vec<crate::reviewers::ReviewerSpec>,
}

pub fn run(
    project: &Path,
    workdir: &Path,
    rules: Rules,
    stage: Stage,
    options: RunOptions,
    notify: impl Fn() + Send + Sync + 'static,
) -> Handle {
    let state = Arc::new(Mutex::new(GuardRun {
        stage,
        started_at: SystemTime::now(),
        items: Vec::new(),
        checks: Vec::new(),
        diff_command: String::new(),
        unstaged: false,
        analyzing: true,
        error: None,
        candidate: String::new(),
        branch: String::new(),
        exceptions: Vec::new(),
        frozen: None,
        finished_at: None,
        project_name: options.name.clone(),
        usage: Vec::new(),
        extra_evidence: Vec::new(),
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    let (s, c, project, workdir) = (
        state.clone(),
        cancel.clone(),
        project.to_path_buf(),
        workdir.to_path_buf(),
    );
    let notify = Arc::new(notify);
    std::thread::spawn(move || {
        let range = options.push_range.as_deref();
        let fail = |e: String| {
            let mut st = s.lock().unwrap();
            st.error = Some(e);
            st.analyzing = false;
            st.finished_at = Some(SystemTime::now());
            drop(st);
            notify();
        };
        // Diff y candidato congelado: el árbol exacto que se va a commitear/subir.
        let analysis = repo_root(&project).and_then(|repo| {
            let diff = collect_diff(&repo, stage, &rules, range)?;
            let frozen = candidate::freeze(
                &repo,
                &options.name,
                stage,
                diff.unstaged,
                range,
                &diff.candidate,
            )?;
            Ok((repo, diff, frozen))
        });
        let (repo, diff, frozen) = match analysis {
            Ok(a) => a,
            Err(e) => return fail(e),
        };
        let now = crate::store::now();
        let exceptions: Vec<Exception> = options
            .exceptions
            .into_iter()
            .filter(|e| e.applies(&e.rule, stage, &diff.candidate, &diff.branch, now))
            .collect();
        let mut items = evaluate(&rules, stage, &diff);
        // Revisores especializados: solo los del área que toca el diff (y solo si la etapa usa IA).
        let mut activations = Vec::new();
        if rules.stage(stage).require_ai_review && !options.reviewers.is_empty() {
            let files = counted_files(&rules, &diff.files);
            let (active, skipped) =
                crate::reviewers::activate(&options.reviewers, &files, &router::assess(&files, 0));
            for a in &active {
                let details = vec![tr!("archivos de su área: {p0}", p0 = a.files.join(", "))];
                items.push(item(
                    format!("reviewer:{}", a.reviewer.id),
                    Level::Pending,
                    a.reviewer.name.clone(),
                    details,
                ));
            }
            if !skipped.is_empty() {
                let label = tr!(
                    "{p0} revisores no activados (el diff no toca su área)",
                    p0 = skipped.len()
                );
                items.push(item(
                    "reviewers-skipped",
                    Level::Info,
                    label,
                    skipped
                        .iter()
                        .map(|(n, why)| format!("{n}: {why}"))
                        .collect(),
                ));
            }
            activations = active;
        }
        apply_exceptions(&mut items, &exceptions);
        let (checks, _) = rules.checks_for(stage);
        let checks: Vec<Check> = checks.into_iter().cloned().collect();
        let reuse = |check: &Check| {
            options
                .evidence
                .iter()
                .filter(|e| {
                    e.tree == frozen.tree && e.check_id == check.id && e.command == check.command
                })
                .max_by_key(|e| e.created_at)
                .cloned()
        };
        {
            let mut st = s.lock().unwrap();
            st.items = items;
            st.diff_command = diff.command.clone();
            st.unstaged = diff.unstaged;
            st.candidate = diff.candidate.clone();
            st.branch = diff.branch.clone();
            st.exceptions = exceptions;
            st.frozen = Some(frozen.clone());
            st.analyzing = false;
            st.checks = checks
                .iter()
                .map(|check| match reuse(check) {
                    Some(e) => CheckRun {
                        check: check.clone(),
                        state: CheckState::Passed,
                        output: e.output,
                        started: None,
                        duration: Some(Duration::from_millis(e.duration_ms)),
                        reused: Some(e.created_at),
                    },
                    None => CheckRun {
                        check: check.clone(),
                        state: CheckState::Waiting,
                        output: String::new(),
                        started: None,
                        duration: None,
                        reused: None,
                    },
                })
                .collect();
        }
        notify();

        // Solo los checks sin evidencia válida se ejecutan.
        let pending: Vec<(usize, Check)> = checks
            .into_iter()
            .enumerate()
            .filter(|(_, c)| reuse(c).is_none())
            .collect();
        // Si el árbol de trabajo difiere del candidato, los checks corren en una copia aislada.
        let snapshot = match (frozen.isolated && !pending.is_empty())
            .then(|| Snapshot::create(&repo, &frozen.tree, &frozen.head))
        {
            Some(Err(e)) => {
                return fail(tr!("no se pudo crear la copia del candidato: {e}", e = e));
            }
            Some(Ok(snap)) => Some(snap),
            None => None,
        };
        let workdir = match &snapshot {
            Some(snap) => {
                let real = workdir.canonicalize().unwrap_or(workdir.clone());
                snap.path
                    .join(real.strip_prefix(&repo).unwrap_or(Path::new("")))
            }
            None => workdir.clone(),
        };

        let threads: Vec<_> = pending
            .into_iter()
            .map(|(i, check)| {
                let (s, c, workdir, notify, env) = (
                    s.clone(),
                    c.clone(),
                    workdir.clone(),
                    notify.clone(),
                    options.env.clone(),
                );
                std::thread::spawn(move || {
                    let started = Instant::now();
                    {
                        let mut st = s.lock().unwrap();
                        st.checks[i].state = CheckState::Running;
                        st.checks[i].started = Some(started);
                    }
                    notify();
                    let (result, output) = run_check(&check, &workdir, &env, &c);
                    let mut st = s.lock().unwrap();
                    st.checks[i].state = result;
                    st.checks[i].output = output;
                    st.checks[i].duration = Some(started.elapsed());
                    drop(st);
                    notify();
                })
            })
            .collect();
        for t in threads {
            let _ = t.join();
        }
        drop(snapshot); // elimina la copia aislada

        if let Some(config) = &options.routing {
            let n = notify.clone();
            let ai = AiRun {
                config,
                rules: &rules,
                stage,
                repo: &repo,
                diff: &diff,
                frozen: &frozen,
                notify: &*n,
                activations: &activations,
            };
            ai.run(&s, &options.evidence, options.month_spent, &options.name);
        }
        s.lock().unwrap().finished_at = Some(SystemTime::now());
        notify();
    });
    Handle { state, cancel }
}

/// Revisiones con IA de una ejecución del Guard.
pub(crate) struct AiRun<'a> {
    config: &'a RouterConfig,
    rules: &'a Rules,
    stage: Stage,
    repo: &'a Path,
    diff: &'a Diff,
    frozen: &'a Candidate,
    notify: &'a dyn Fn(),
    activations: &'a [Activation],
}

impl AiRun<'_> {
    fn run(
        &self,
        s: &Arc<Mutex<GuardRun>>,
        evidence: &[Evidence],
        month_spent: f64,
        project: &str,
    ) {
        let wanted: Vec<(usize, String)> = {
            let st = s.lock().unwrap();
            st.items
                .iter()
                .enumerate()
                .filter(|(_, i)| {
                    i.id == "ai-review"
                        || i.id == "security-review"
                        || i.id.starts_with("reviewer:")
                })
                .map(|(n, i)| (n, i.id.clone()))
                .collect()
        };
        if wanted.is_empty() {
            return;
        }
        // La IA nunca sustituye a lo determinista: si algo bloquea, no se gasta en revisar.
        let (blocked, deterministic) = {
            let st = s.lock().unwrap();
            let blocked = st.items.iter().any(|i| i.level == Level::Block)
                || st
                    .checks
                    .iter()
                    .any(|c| st.check_level(c) == Some(Level::Block));
            let passed = st
                .items
                .iter()
                .filter(|i| matches!(i.level, Level::Pass | Level::Excepted))
                .map(|i| format!("✓ {}", i.label))
                .chain(
                    st.checks
                        .iter()
                        .filter(|c| c.state == CheckState::Passed)
                        .map(|c| format!("✓ {}", c.check.id)),
                )
                .collect::<Vec<_>>();
            (blocked, passed)
        };
        if blocked {
            let mut st = s.lock().unwrap();
            for (n, _) in &wanted {
                st.items[*n].details = vec![
                    tr!("en espera: primero hay que resolver los bloqueos deterministas").into(),
                ];
            }
            return;
        }
        let files = counted_files(self.rules, &self.diff.files);
        let risk = router::assess(&files, 0);
        let ctx = ReviewContext {
            project: project.to_string(),
            stage: self.stage,
            risk: risk.clone(),
            deterministic,
            files,
            repo: self.repo.to_path_buf(),
            range: self.diff.range.clone(),
        };
        let local = router::local_available();
        let mut spent = month_spent;
        for (n, rule) in wanted
            .iter()
            .filter(|(_, r)| !r.starts_with("reviewer:"))
            .cloned()
        {
            let security = rule == "security-review";
            let title = if security {
                tr!("Revisión de seguridad")
            } else {
                tr!("Revisión de IA")
            };
            // Firma de la ruta: si cambia el perfil o los modelos, la aprobación previa no vale.
            let signature = format!(
                "{:?}|{}",
                self.config.profile,
                router::plan(self.config, &risk, security)
                    .iter()
                    .map(|t| router::route(self.config, *t, local).label())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            if let Some(e) = evidence
                .iter()
                .filter(|e| {
                    e.tree == self.frozen.tree && e.check_id == rule && e.command == signature
                })
                .max_by_key(|e| e.created_at)
            {
                let mut st = s.lock().unwrap();
                st.items[n].level = Level::Pass;
                st.items[n].label = tr!(
                    "{title}: aprobada (evidencia del mismo candidato)",
                    title = title
                );
                st.items[n].details = vec![e.output.clone()];
                continue;
            }
            let started = Instant::now();
            let on_step = |step: &str| {
                s.lock().unwrap().items[n].label =
                    tr!("{title} · {step}…", title = title, step = step);
                (self.notify)();
            };
            let outcome = router::review(
                self.config,
                self.rules,
                &ctx,
                security,
                spent,
                local,
                &on_step,
            );
            spent += outcome
                .steps
                .iter()
                .filter_map(|st| st.usage.as_ref())
                .filter(|u| !u.local)
                .map(|u| u.cost_usd)
                .sum::<f64>();

            let mut st = s.lock().unwrap();
            st.usage
                .extend(outcome.steps.iter().filter_map(|step| step.usage.clone()));
            let (level, word) = match outcome.verdict {
                AiVerdict::Approve => (Level::Pass, "aprobada"),
                AiVerdict::Changes => (Level::Warn, tr!("pide cambios")),
                AiVerdict::Block => (Level::Block, "bloquea"),
                AiVerdict::Incomplete => (Level::Pending, "incompleta"),
            };
            let mut details = outcome.details();
            let level = match st.exception_for(&rule).cloned() {
                Some(e) if level != Level::Pass => {
                    details.insert(0, e.summary());
                    Level::Excepted
                }
                _ => level,
            };
            let cost = outcome.cost();
            st.items[n].level = level;
            st.items[n].label = format!("{title}: {word} · ${cost:.3}");
            st.items[n].details = details;
            if outcome.verdict == AiVerdict::Approve {
                st.extra_evidence.push(Evidence {
                    check_id: rule.clone(),
                    command: signature,
                    tree: self.frozen.tree.clone(),
                    duration_ms: started.elapsed().as_millis() as u64,
                    output: outcome.summary.clone(),
                    created_at: unix(SystemTime::now()),
                });
            }
            drop(st);
            (self.notify)();
        }
        self.run_reviewers(s, evidence, spent, &ctx, local);
    }

    /// Revisores especializados activados por el diff, en paralelo, y su comparación.
    fn run_reviewers(
        &self,
        s: &Arc<Mutex<GuardRun>>,
        evidence: &[Evidence],
        spent: f64,
        ctx: &ReviewContext,
        local: bool,
    ) {
        let jobs: Vec<(usize, Activation)> = {
            let st = s.lock().unwrap();
            st.items
                .iter()
                .enumerate()
                .filter_map(|(n, i)| {
                    let id = i.id.strip_prefix("reviewer:")?;
                    self.activations
                        .iter()
                        .find(|a| a.reviewer.id == id)
                        .map(|a| (n, a.clone()))
                })
                .collect()
        };
        if jobs.is_empty() {
            return;
        }
        let signature =
            |a: &Activation| router::reviewer_route(self.config, &a.reviewer, local).label();
        // Aprobaciones previas del mismo candidato (y misma ruta): no se repiten.
        let mut pending = Vec::new();
        for (n, a) in jobs {
            let rule = format!("reviewer:{}", a.reviewer.id);
            let reused = evidence
                .iter()
                .filter(|e| {
                    e.tree == self.frozen.tree && e.check_id == rule && e.command == signature(&a)
                })
                .max_by_key(|e| e.created_at);
            let mut st = s.lock().unwrap();
            match reused {
                Some(e) => {
                    st.items[n].level = Level::Pass;
                    st.items[n].label = tr!(
                        "{p0}: aprueba (evidencia del mismo candidato)",
                        p0 = a.reviewer.name
                    );
                    st.items[n].details = vec![e.output.clone()];
                }
                None => {
                    st.items[n].label = tr!(
                        "{p0} · revisando {p1} archivos…",
                        p0 = a.reviewer.name,
                        p1 = a.files.len()
                    );
                    pending.push((n, a));
                }
            }
        }
        (self.notify)();

        // ponytail: todos parten del mismo gasto del mes; con presupuesto muy justo se puede
        // superar como mucho en (revisores − 1) × max_cost_per_call_usd.
        let (config, rules) = (self.config, self.rules);
        let outcomes: Vec<(usize, Activation, AiOutcome)> = std::thread::scope(|scope| {
            let handles: Vec<_> = pending
                .into_iter()
                .map(|(n, a)| {
                    scope.spawn(move || {
                        let outcome = router::specialist(config, rules, ctx, &a, spent, local);
                        (n, a, outcome)
                    })
                })
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });

        let mut st = s.lock().unwrap();
        for (n, a, outcome) in &outcomes {
            let rule = format!("reviewer:{}", a.reviewer.id);
            st.usage
                .extend(outcome.steps.iter().filter_map(|step| step.usage.clone()));
            // Un revisor informativo nunca bloquea: sus objeciones quedan como aviso o nota.
            let (level, word) = match (outcome.verdict, a.reviewer.blocking) {
                (AiVerdict::Approve, _) => (Level::Pass, "aprueba"),
                (AiVerdict::Block, true) => (Level::Block, "bloquea"),
                (AiVerdict::Block, false) => (Level::Warn, "objeta (informativo)"),
                (AiVerdict::Changes, true) => (Level::Warn, tr!("pide cambios")),
                (AiVerdict::Changes, false) => (Level::Info, "comenta"),
                (AiVerdict::Incomplete, true) => (Level::Pending, "incompleto"),
                (AiVerdict::Incomplete, false) => (Level::Info, tr!("sin respuesta")),
            };
            let mut details = vec![tr!("archivos: {p0}", p0 = a.files.join(", "))];
            details.extend(outcome.details());
            let level = match st.exception_for(&rule).cloned() {
                Some(e) if matches!(level, Level::Block | Level::Warn | Level::Pending) => {
                    details.insert(0, e.summary());
                    Level::Excepted
                }
                _ => level,
            };
            st.items[*n].level = level;
            st.items[*n].label = format!("{}: {word} · ${:.3}", a.reviewer.name, outcome.cost());
            st.items[*n].details = details;
            if outcome.verdict == AiVerdict::Approve {
                st.extra_evidence.push(Evidence {
                    check_id: rule,
                    command: signature(a),
                    tree: self.frozen.tree.clone(),
                    duration_ms: outcome
                        .steps
                        .iter()
                        .map(|s| (s.seconds * 1000.0) as u64)
                        .sum(),
                    output: outcome.summary.clone(),
                    created_at: unix(SystemTime::now()),
                });
            }
        }
        // Comparación entre revisores (plan §11): desacuerdos a la vista.
        if outcomes.len() > 1 {
            let summary: Vec<(String, AiVerdict, f64, String)> = outcomes
                .iter()
                .map(|(_, a, o)| {
                    let confidence = o
                        .steps
                        .iter()
                        .find_map(|s| s.review.as_ref())
                        .map_or(0.0, |r| r.confidence);
                    (
                        a.reviewer.name.clone(),
                        o.verdict,
                        confidence,
                        o.summary.clone(),
                    )
                })
                .collect();
            let (level, label, lines) = compare_reviewers(&summary);
            st.items
                .push(item("reviewers-compare", level, label, lines));
        }
        drop(st);
        (self.notify)();
    }
}

/// Compara a los revisores: si unos aprueban y otros objetan, se avisa.
pub(crate) fn compare_reviewers(
    outcomes: &[(String, AiVerdict, f64, String)],
) -> (Level, &'static str, Vec<String>) {
    let lines = outcomes
        .iter()
        .map(|(name, verdict, confidence, summary)| {
            format!("{name}: {verdict:?} (confianza {confidence:.2}) — {summary}")
        })
        .collect();
    let approves = outcomes.iter().any(|o| o.1 == AiVerdict::Approve);
    let objects = outcomes
        .iter()
        .any(|o| matches!(o.1, AiVerdict::Block | AiVerdict::Changes));
    if approves && objects {
        (
            Level::Warn,
            tr!("Los revisores no coinciden: revisa sus objeciones"),
            lines,
        )
    } else {
        (
            Level::Info,
            tr!("Comparación de revisores: coinciden"),
            lines,
        )
    }
}

/// Ejecuta un check en un shell de login, en su propio grupo de procesos
/// (para poder matar todo al cancelar o al vencer el tiempo).
pub(crate) fn run_check(
    check: &Check,
    workdir: &Path,
    env: &std::collections::HashMap<String, String>,
    cancel: &AtomicBool,
) -> (CheckState, String) {
    use std::os::unix::process::CommandExt;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let child = Command::new(shell)
        .args(["-l", "-c", &format!("{{ {}\n}} 2>&1", check.command)])
        .current_dir(workdir)
        .envs(env)
        // Variables de los hooks: dentro de la copia aislada apuntarían al repositorio real.
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        // Modo no interactivo: vitest/jest no se quedan en "watch", sin colores ni prompts.
        .env("CI", "true")
        .env("FORCE_COLOR", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return (CheckState::Error(e.to_string()), String::new()),
    };
    let mut stdout = child.stdout.take().expect("stdout con pipe");
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + Duration::from_secs(check.timeout_seconds.max(1));
    let pgid = child.id() as libc::pid_t;
    let state = loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break CheckState::Passed,
            Ok(Some(status)) => break CheckState::Failed(status.code().unwrap_or(-1)),
            Err(e) => break CheckState::Error(e.to_string()),
            Ok(None) => {}
        }
        let stop = if cancel.load(Ordering::Relaxed) {
            Some(CheckState::Cancelled)
        } else if Instant::now() >= deadline {
            Some(CheckState::TimedOut)
        } else {
            None
        };
        if let Some(stop) = stop {
            // SAFETY: killpg solo envía señales al grupo creado con process_group(0).
            unsafe { libc::killpg(pgid, libc::SIGTERM) };
            std::thread::sleep(Duration::from_millis(500));
            unsafe { libc::killpg(pgid, libc::SIGKILL) };
            let _ = child.wait();
            break stop;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let output = reader.join().unwrap_or_default();
    (
        state,
        tail(&crate::strip_ansi(&String::from_utf8_lossy(&output)), 80),
    )
}

/// Últimas `n` líneas.
pub(crate) fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}
