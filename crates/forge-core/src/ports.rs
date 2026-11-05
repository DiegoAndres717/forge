// Puertos por worktree (plan #194, fase 5): varias copias del proyecto con su servidor de
// desarrollo chocaban en el mismo puerto (3000, 5173…). Cada worktree registrado con
// `forge worktree new` recibe una ranura estable (1, 2, 3…) guardada en su registro
// (`<rama>.json`), y su desplazamiento es `ranura × STEP`. El worktree principal y los que
// no son de Forge se quedan en 0: nada cambia para quien trabaja en una sola copia.
//
// Esquema: la ranura se asigna la primera vez que se mira (al crearlo `forge worktree new`
// ya la pide) y no cambia mientras el worktree exista; al quitarlo su ranura queda libre y
// la reutiliza el siguiente. Si dos registros acaban con la misma (dos `worktree new` a la
// vez), la conserva el más antiguo (fecha de creación y luego ruta) y el otro pasa a la
// primera libre: determinista, sin depender del orden en que se lea la carpeta.
use crate::worktrees::{self, Record};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// Separación entre worktrees: el 3000 del primero es 3010, el del segundo 3020… Deja
/// sitio a proyectos con varios puertos seguidos (3000 y 3001) sin pisarse entre copias.
pub const STEP: u32 = 10;

/// Variable con el desplazamiento que reciben siempre los procesos del proyecto.
pub const OFFSET_VAR: &str = "FORGE_PORT_OFFSET";

/// Reparte las ranuras de los registros de un proyecto: conserva las que ya son válidas y da
/// la primera libre a los que no tienen o la comparten con uno más antiguo. Devuelve los
/// índices que cambiaron (para guardarlos).
fn settle(records: &mut [Record]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..records.len()).collect();
    order.sort_by(|&a, &b| {
        (records[a].created_at, &records[a].path).cmp(&(records[b].created_at, &records[b].path))
    });
    let mut taken = BTreeSet::new();
    let pending: Vec<usize> = order
        .into_iter()
        .filter(|&i| records[i].slot == 0 || !taken.insert(records[i].slot))
        .collect();
    for &i in &pending {
        let free = (1..).find(|s| !taken.contains(s)).unwrap_or(u32::MAX);
        taken.insert(free);
        records[i].slot = free;
    }
    pending
}

/// `(carpeta del worktree, desplazamiento)` de los worktrees registrados del repositorio de
/// `repo`, con sus ranuras ya asignadas y guardadas en el registro.
pub fn offsets(repo: &Path, root: &Path) -> Vec<(PathBuf, u32)> {
    let mut records = worktrees::records(repo, root);
    for i in settle(&mut records) {
        // Sin poder guardarla (carpeta de solo lectura) vale igual hasta la próxima lectura.
        worktrees::write_record(&records[i]);
    }
    records
        .into_iter()
        .map(|r| (r.path, r.slot.saturating_mul(STEP)))
        .collect()
}

/// El desplazamiento de puertos de la carpeta `path` (raíz de un worktree): 0 en el
/// principal, en lo que no es un repositorio y en worktrees que Forge no registró.
pub fn offset(path: &Path, root: &Path) -> u32 {
    let Ok(main) = worktrees::main_root(path) else {
        return 0;
    };
    let here = crate::canonical(path);
    if crate::canonical(&main) == here {
        return 0;
    }
    offsets(path, root)
        .into_iter()
        .find(|(p, _)| crate::canonical(p) == here)
        .map_or(0, |(_, o)| o)
}

/// El desplazamiento de `path` con la carpeta de worktrees de Forge (la de la base de datos).
pub fn offset_here(path: &Path) -> u32 {
    worktrees::root().map_or(0, |root| offset(path, &root))
}

/// `port + offset`, si cabe en un puerto.
pub fn shifted(port: u16, offset: u32) -> Option<u16> {
    u16::try_from(u32::from(port).checked_add(offset)?).ok()
}

/// El `PORT` que Forge le da a un proceso que declara `port` (su puerto base): ninguno si
/// el proceso ya fija su propio `PORT` en su entorno (ese no se pisa).
pub fn assigned(port: Option<u16>, own: &HashMap<String, String>, offset: u32) -> Option<u16> {
    if own.keys().any(|k| k.eq_ignore_ascii_case("PORT")) {
        return None;
    }
    shifted(port?, offset)
}

/// Añade al entorno de un proceso `FORGE_PORT_OFFSET` y, si le toca, `PORT`.
pub fn apply(
    env: &mut HashMap<String, String>,
    port: Option<u16>,
    own: &HashMap<String, String>,
    offset: u32,
) {
    env.insert(OFFSET_VAR.into(), offset.to_string());
    if let Some(p) = assigned(port, own, offset) {
        env.insert("PORT".into(), p.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::test_repo;

    /// Plan #194, fase 5: dos worktrees del mismo proyecto no comparten desplazamiento, y el
    /// principal (y una carpeta que no es un repositorio) sigue en 0.
    #[test]
    fn two_worktrees_get_different_offsets_and_the_main_stays_at_zero() {
        let dir = test_repo("ports-two", "true");
        let root = dir.join("forge-data/worktrees");
        let a = worktrees::create(&dir, "feat/a", None, &root, Record::default()).unwrap();
        let b = worktrees::create(&dir, "feat/b", None, &root, Record::default()).unwrap();
        let (oa, ob) = (offset(&a, &root), offset(&b, &root));
        assert!(oa > 0 && ob > 0, "{oa} {ob}");
        assert_ne!(oa, ob);
        assert_eq!(oa % STEP, 0);
        assert_eq!(offset(&dir, &root), 0, "el principal no cambia");
        assert_eq!(offset(&root, &root), 0, "fuera de un repositorio");
        let all = offsets(&dir, &root);
        assert_eq!(all.len(), 2, "{all:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// El desplazamiento queda en el registro: tras «reiniciar» (leerlo de nuevo del disco)
    /// es el mismo, quitar otro worktree no lo mueve, y la ranura libre se reutiliza.
    #[test]
    fn the_offset_is_stored_in_the_registry_and_survives_a_restart() {
        let dir = test_repo("ports-keep", "true");
        let root = dir.join("forge-data/worktrees");
        let a = worktrees::create(&dir, "feat/a", None, &root, Record::default()).unwrap();
        let b = worktrees::create(&dir, "feat/b", None, &root, Record::default()).unwrap();
        let (oa, ob) = (offset(&a, &root), offset(&b, &root));
        assert!(oa > 0 && ob > 0 && oa != ob, "{oa} {ob}");
        let stored = worktrees::records(&dir, &root);
        let slot_b = stored.iter().find(|r| r.branch == "feat/b").unwrap().slot;
        assert_eq!(slot_b * STEP, ob, "guardado en {stored:?}");
        // Otra lectura desde cero: lo mismo.
        assert_eq!((offset(&a, &root), offset(&b, &root)), (oa, ob));
        // Se quita el primero (y su registro, como hace la limpieza de Forge): el segundo
        // conserva el suyo y uno nuevo toma la ranura libre.
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .env(worktrees::ALLOW, "1")
                .output()
                .unwrap()
        };
        assert!(
            git(&["worktree", "remove", &a.to_string_lossy()])
                .status
                .success()
        );
        let _ = std::fs::remove_file(a.with_file_name("feat-a.json"));
        assert_eq!(offset(&b, &root), ob);
        let c = worktrees::create(&dir, "feat/c", None, &root, Record::default()).unwrap();
        assert_eq!(offset(&c, &root), oa, "reutiliza la ranura libre");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Guardar la ranura nunca pisa un `.json` ajeno al lado de la carpeta (otra ruta dentro).
    #[test]
    fn a_foreign_json_next_to_a_worktree_is_never_rewritten() {
        let dir = std::env::temp_dir().join(format!("forge-ports-foreign-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.json");
        let foreign = r#"{"path":"/elsewhere/x","branch":"b","project":"/p"}"#;
        std::fs::write(&file, foreign).unwrap();
        let record = Record {
            path: dir.join("x"),
            slot: 3,
            ..Default::default()
        };
        assert!(!worktrees::write_record(&record));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), foreign);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Registros de antes de esta fase (sin ranura) o con la misma ranura repetida: la
    /// conserva el más antiguo y los demás pasan a la primera libre, siempre igual.
    #[test]
    fn missing_or_repeated_slots_are_settled_the_same_way_every_time() {
        let rec = |path: &str, slot: u32, created_at: i64| Record {
            path: path.into(),
            slot,
            created_at,
            ..Default::default()
        };
        let mut list = vec![
            rec("/w/new", 2, 30),
            rec("/w/legacy", 0, 5),
            rec("/w/old", 2, 10),
            rec("/w/first", 1, 1),
        ];
        let changed = settle(&mut list);
        let slots: Vec<u32> = list.iter().map(|r| r.slot).collect();
        // /w/old (más antiguo) se queda el 2; /w/legacy y /w/new reciben 3 y 4 por antigüedad.
        assert_eq!(slots, [4, 3, 2, 1]);
        assert_eq!(changed.len(), 2, "{changed:?}");
        // Ya repartidas: otra pasada no cambia nada.
        assert!(settle(&mut list).is_empty());
    }

    /// El proceso recibe siempre FORGE_PORT_OFFSET; si declara `port`, PORT = base +
    /// desplazamiento; si fija su propio PORT, ese no se toca.
    #[test]
    fn a_process_gets_its_shifted_port_unless_it_sets_its_own() {
        let none = HashMap::new();
        let mut env = HashMap::new();
        apply(&mut env, Some(3000), &none, 20);
        assert_eq!(env.get(OFFSET_VAR).map(String::as_str), Some("20"));
        assert_eq!(env.get("PORT").map(String::as_str), Some("3020"));
        // Sin `port`: solo el desplazamiento (el proceso decide qué hacer con él).
        let mut env = HashMap::new();
        apply(&mut env, None, &none, 20);
        assert_eq!(env.get(OFFSET_VAR).map(String::as_str), Some("20"));
        assert!(!env.contains_key("PORT"));
        // Con su propio PORT: se queda el suyo.
        let own = HashMap::from([("PORT".to_string(), "4000".to_string())]);
        let mut env = own.clone();
        apply(&mut env, Some(3000), &own, 20);
        assert_eq!(env.get("PORT").map(String::as_str), Some("4000"));
        assert_eq!(assigned(Some(3000), &own, 20), None);
        // En el principal todo sigue igual: PORT es el base.
        assert_eq!(assigned(Some(3000), &none, 0), Some(3000));
        // Lo que no cabe en un puerto no se inventa.
        assert_eq!(shifted(65_530, 10), None);
    }
}
