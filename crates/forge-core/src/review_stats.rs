// Medir si la revisión por otra empresa sirve (plan #84, fase 3): la gravedad que el revisor
// pone a cada hallazgo, lo que el dueño decide de él y el resumen por pareja autor → revisor.
// Un revisor casi siempre devuelve algo: aquí se separan los fallos reales del estilo.
use std::collections::BTreeMap;

use crate::cross_review::{self, Company};
use crate::ideas::ReviewComment;
use crate::tr;

/// La gravedad que el revisor pone al empezar el hallazgo: `[bug/high]`, `[bug/medium]`,
/// `[bug/low]` o `[style]`. Sin marcador reconocible, desconocida.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    High,
    Medium,
    Low,
    Style,
    Unknown,
}

impl Severity {
    /// La del primer marcador reconocible del texto (mayúsculas y espacios dentro dan igual).
    pub fn parse(text: &str) -> Self {
        let lower = text.to_lowercase();
        lower
            .split('[')
            .skip(1)
            .filter_map(|part| part.split_once(']'))
            .find_map(|(label, _)| {
                let label: String = label.chars().filter(|c| !c.is_whitespace()).collect();
                Some(match label.as_str() {
                    "bug/high" => Self::High,
                    "bug/medium" => Self::Medium,
                    "bug/low" => Self::Low,
                    "style" => Self::Style,
                    _ => return None,
                })
            })
            .unwrap_or(Self::Unknown)
    }

    /// Cómo se guarda en la base (`''` = desconocida).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Style => "style",
            Self::Unknown => "",
        }
    }

    pub fn from_stored(text: &str) -> Self {
        match text {
            "high" => Self::High,
            "medium" => Self::Medium,
            "low" => Self::Low,
            "style" => Self::Style,
            _ => Self::Unknown,
        }
    }
}

/// Lo que se decidió de un hallazgo: un fallo de verdad (se arregló o se dio por válido), no
/// lo era (descartado), era estilo, o aún nada.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pending,
    Accepted,
    Dismissed,
    Style,
}

impl Outcome {
    /// Cómo se guarda en la base (`''` = pendiente).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "",
            Self::Accepted => "accepted",
            Self::Dismissed => "dismissed",
            Self::Style => "style",
        }
    }

    pub fn from_stored(text: &str) -> Self {
        match text {
            "accepted" => Self::Accepted,
            "dismissed" => Self::Dismissed,
            "style" => Self::Style,
            _ => Self::Pending,
        }
    }
}

/// Lo que cuenta un hallazgo en el resumen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bucket {
    Real,
    Style,
    Dismissed,
    Pending,
}

/// Descartado gana a todo; luego estilo (lo diga el dueño o el marcador); aceptado es un fallo
/// real; el resto, pendiente.
pub fn bucket(comment: &ReviewComment) -> Bucket {
    match comment.outcome {
        Outcome::Dismissed => Bucket::Dismissed,
        Outcome::Style => Bucket::Style,
        _ if comment.severity == Severity::Style => Bucket::Style,
        Outcome::Accepted => Bucket::Real,
        Outcome::Pending => Bucket::Pending,
    }
}

/// Los hallazgos de una pareja autor → revisor (ids de agente; `""` si no se sabe).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PairStats {
    pub author: String,
    pub reviewer: String,
    pub real: usize,
    pub style: usize,
    pub dismissed: usize,
    pub pending: usize,
}

impl PairStats {
    /// Porcentaje de hallazgos ya decididos que eran fallos reales; `None` sin ninguno decidido.
    pub fn hit_rate(&self) -> Option<u32> {
        let decided = self.real + self.style + self.dismissed;
        (decided > 0).then(|| ((self.real * 100 + decided / 2) / decided) as u32)
    }
}

/// El resumen por pareja autor → revisor, en orden estable. Solo cuentan los hallazgos
/// medidos (con revisor anotado): los de antes de la medición no dicen de quién son.
pub fn summarize(comments: &[ReviewComment]) -> Vec<PairStats> {
    let mut pairs: BTreeMap<(&str, &str), PairStats> = BTreeMap::new();
    for c in comments.iter().filter(|c| !c.reviewer_agent.is_empty()) {
        let pair = pairs
            .entry((c.author.as_str(), c.reviewer_agent.as_str()))
            .or_insert_with(|| PairStats {
                author: c.author.clone(),
                reviewer: c.reviewer_agent.clone(),
                ..Default::default()
            });
        match bucket(c) {
            Bucket::Real => pair.real += 1,
            Bucket::Style => pair.style += 1,
            Bucket::Dismissed => pair.dismissed += 1,
            Bucket::Pending => pair.pending += 1,
        }
    }
    pairs.into_values().collect()
}

/// Nombre de un agente para mostrarlo: «Claude Code (Anthropic)».
fn who(id: &str) -> String {
    if id.is_empty() {
        return tr!("autor desconocido").to_string();
    }
    let name = crate::agents::builtins()
        .into_iter()
        .find(|a| a.id == id)
        .map(|a| a.name)
        .unwrap_or_else(|| id.to_string());
    match cross_review::company(id).map(Company::name) {
        Some(company) => format!("{name} ({company})"),
        None => name,
    }
}

/// Una línea por pareja; sin datos, que aún no hay revisiones medidas.
pub fn render(stats: &[PairStats]) -> Vec<String> {
    if stats.is_empty() {
        return vec![tr!("Aún no hay revisiones medidas").to_string()];
    }
    stats
        .iter()
        .map(|s| {
            let rate = match s.hit_rate() {
                // Sin espacio partible: «100 %» no se corta en dos líneas.
                Some(rate) => format!("{rate}\u{a0}%"),
                None => tr!("sin decidir").to_string(),
            };
            tr!(
                "{author} → {reviewer}: fallos reales {real} · estilo {style} · descartados {dismissed} · pendientes {pending} · acierto {rate}",
                author = who(&s.author),
                reviewer = who(&s.reviewer),
                real = s.real,
                style = s.style,
                dismissed = s.dismissed,
                pending = s.pending,
                rate = rate
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(author: &str, reviewer: &str, text: &str, outcome: Outcome) -> ReviewComment {
        ReviewComment {
            id: 0,
            plan_id: 1,
            phase_id: "f1".into(),
            text: text.into(),
            file: None,
            line: None,
            reviewer: reviewer.into(),
            status: "open".into(),
            resolution: String::new(),
            created_at: 0,
            learned: false,
            reviewer_agent: reviewer.into(),
            author: author.into(),
            severity: Severity::parse(text),
            outcome,
        }
    }

    #[test]
    fn severity_markers_are_parsed_and_a_finding_without_one_is_unknown() {
        assert_eq!(
            Severity::parse("[bug/high] se pierde el pago"),
            Severity::High
        );
        assert_eq!(Severity::parse("[bug/medium] x"), Severity::Medium);
        assert_eq!(Severity::parse("  [Bug/Low] x"), Severity::Low);
        assert_eq!(Severity::parse("[style] nombre raro"), Severity::Style);
        // Con algo delante (una lista, negrita) y espacios dentro del marcador.
        assert_eq!(Severity::parse("1. **[ bug / high ]** x"), Severity::High);
        // Sin marcador, o uno que no es de gravedad: desconocida, sin romper nada.
        assert_eq!(
            Severity::parse("Falta validar la entrada"),
            Severity::Unknown
        );
        assert_eq!(Severity::parse("[nota] x [bug/low]"), Severity::Low);
        assert_eq!(Severity::parse("[sin cerrar"), Severity::Unknown);
        assert_eq!(Severity::parse(""), Severity::Unknown);
        for s in [
            Severity::High,
            Severity::Medium,
            Severity::Low,
            Severity::Style,
            Severity::Unknown,
        ] {
            assert_eq!(Severity::from_stored(s.as_str()), s);
        }
    }

    #[test]
    fn each_finding_counts_as_real_style_dismissed_or_pending() {
        use Outcome::*;
        let b = |text, outcome| bucket(&comment("claude", "codex", text, outcome));
        assert_eq!(b("[bug/high] x", Accepted), Bucket::Real);
        assert_eq!(b("[bug/low] x", Pending), Bucket::Pending);
        assert_eq!(b("[bug/low] x", Dismissed), Bucket::Dismissed);
        // Un [style] es estilo aunque se arreglara; un bug que el dueño ve como estilo, también.
        assert_eq!(b("[style] x", Accepted), Bucket::Style);
        assert_eq!(b("[style] x", Pending), Bucket::Style);
        assert_eq!(b("[bug/medium] x", Style), Bucket::Style);
        // Descartado gana: un [style] rechazado cuenta como descartado.
        assert_eq!(b("[style] x", Dismissed), Bucket::Dismissed);
        // Sin marcador y aceptado: el dueño lo dio por fallo.
        assert_eq!(b("sin marcador", Accepted), Bucket::Real);
    }

    #[test]
    fn the_summary_groups_by_author_and_reviewer_and_computes_the_hit_rate() {
        use Outcome::*;
        let comments = [
            comment("claude", "codex", "[bug/high] a", Accepted),
            comment("claude", "codex", "[bug/low] b", Accepted),
            comment("claude", "codex", "[style] c", Pending),
            comment("claude", "codex", "[bug/medium] d", Dismissed),
            comment("claude", "codex", "[bug/medium] e", Pending),
            comment("codex", "claude", "[bug/low] f", Dismissed),
            comment("", "opencode", "[bug/low] g", Pending),
            // De antes de la medición (sin revisor anotado): no cuenta.
            comment("claude", "", "[bug/high] h", Accepted),
        ];
        let stats = summarize(&comments);
        assert_eq!(stats.len(), 3, "{stats:?}");
        let pair = |a: &str, r: &str| {
            stats
                .iter()
                .find(|s| s.author == a && s.reviewer == r)
                .unwrap()
                .clone()
        };
        let cc = pair("claude", "codex");
        assert_eq!(
            (cc.real, cc.style, cc.dismissed, cc.pending),
            (2, 1, 1, 1),
            "{cc:?}"
        );
        // 2 reales de 4 decididos (el pendiente no cuenta).
        assert_eq!(cc.hit_rate(), Some(50));
        assert_eq!(pair("codex", "claude").hit_rate(), Some(0));
        assert_eq!(pair("", "opencode").hit_rate(), None, "nada decidido");
        // Redondeo: 2 de 3.
        let third = PairStats {
            real: 2,
            dismissed: 1,
            ..Default::default()
        };
        assert_eq!(third.hit_rate(), Some(67));
        // Estable: mismas entradas, mismo orden.
        assert_eq!(summarize(&comments), stats);
    }

    #[test]
    fn the_summary_says_so_when_there_is_no_measured_review() {
        assert_eq!(
            render(&[]),
            vec!["Aún no hay revisiones medidas".to_string()]
        );
        let old = [comment("claude", "", "[bug/high] x", Outcome::Accepted)];
        assert_eq!(
            render(&summarize(&old)),
            vec!["Aún no hay revisiones medidas".to_string()]
        );
    }

    #[test]
    fn each_pair_is_one_line_with_names_companies_and_counts() {
        let stats = [
            PairStats {
                author: "claude".into(),
                reviewer: "codex".into(),
                real: 2,
                style: 1,
                dismissed: 1,
                pending: 1,
            },
            PairStats {
                author: String::new(),
                reviewer: "mio".into(),
                pending: 1,
                ..Default::default()
            },
        ];
        let lines = render(&stats);
        assert_eq!(
            lines[0],
            "Claude Code (Anthropic) → Codex (OpenAI): fallos reales 2 · estilo 1 · descartados 1 · pendientes 1 · acierto 50\u{a0}%"
        );
        assert_eq!(
            lines[1],
            "autor desconocido → mio: fallos reales 0 · estilo 0 · descartados 0 · pendientes 1 · acierto sin decidir"
        );
    }
}
