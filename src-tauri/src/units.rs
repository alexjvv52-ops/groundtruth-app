//! GT-D27 UNITS (J1 UNITS-STORE). One display system for mass, read from
//! `farm_config` (v41 config row, id = 1). DISPLAY only: the farm file weighs
//! in ounces and keeps weighing in ounces, every row already written keeps the
//! number it was written with, and nothing here converts anything on the way
//! into an event.

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

/// SCOPE A, mass only. Two rows: the system the farm picks, the name Settings
/// prints, and the short unit a printer puts after a figure. One table — no
/// printer carries a second one, and the list widens only by a signed job.
const SEALED: &[(&str, &str, &str)] = &[
    ("imperial", "Imperial (ounces)", "oz"),
    ("metric", "Metric (grams)", "g"),
];

/// A farm whose `farm_config.units` is NULL reads `imperial`: no farm changes
/// the units it prints in by upgrading.
pub const DEFAULT_UNITS: &str = "imperial";

/// PRECISION G. Exact by definition: a pound is 453.59237 g and sixteen
/// ounces, so an ounce is 28.349523125 g. The one constant — no printer
/// carries its own, and nothing here picks 28 or 28.3 by feel.
pub const OZ_TO_G: f64 = 28.349523125;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnitsChoice {
    pub system: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FarmUnitsView {
    pub system: String,
    pub name: String,
    pub unit: String,
    pub choices: Vec<UnitsChoice>,
}

/// The one seal. Lowercases both sides: a stored or imported `Metric` is the
/// same system as `metric`, and anything off the list is refused.
pub fn is_sealed(system: &str) -> bool {
    let system = system.trim().to_ascii_lowercase();
    SEALED.iter().any(|(s, _, _)| *s == system)
}

/// The sealed systems as one printable line, for refusals. No refusal spells
/// the list out by hand.
pub fn sealed_systems_line() -> String {
    SEALED
        .iter()
        .map(|(s, _, _)| *s)
        .collect::<Vec<_>>()
        .join(", ")
}

fn name_for(system: &str) -> String {
    let lower = system.trim().to_ascii_lowercase();
    SEALED
        .iter()
        .find(|(s, _, _)| *s == lower)
        .map(|(_, n, _)| (*n).to_string())
        .unwrap_or_else(|| lower.to_uppercase())
}

/// The short unit a printer puts after a figure in this system. The one
/// table: no printer decides a unit word for itself. An unsealed system
/// cannot reach here through `farm_units`, so the fallback is unreachable by
/// construction — it returns the system's own name, because an honest
/// `12.0 IMPERIAL` beats a wrong `12.0 g`.
pub fn unit_for(system: &str) -> String {
    let lower = system.trim().to_ascii_lowercase();
    SEALED
        .iter()
        .find(|(s, _, _)| *s == lower)
        .map(|(_, _, u)| (*u).to_string())
        .unwrap_or_else(|| lower.to_uppercase())
}

/// CANON A. Ounces on the books become whole grams on a face, and only on a
/// face: never on the way into an event, never back into a column. Half away
/// from zero, the one rounding rule both sides of the wire use. A tenth of an
/// ounce is 2.8 g, so no two stored tenths ever print the same gram figure.
pub fn grams(oz: f64) -> i64 {
    if !oz.is_finite() {
        return 0;
    }
    (oz * OZ_TO_G).round() as i64
}

fn choices() -> Vec<UnitsChoice> {
    SEALED
        .iter()
        .map(|(s, n, _)| UnitsChoice {
            system: (*s).to_string(),
            name: (*n).to_string(),
        })
        .collect()
}

/// The display system this desk prints mass in. NULL, blank or a value this
/// build is not sealed for reads `imperial` — a farm file from a wider build
/// never prints a system this build cannot speak.
pub fn farm_units(conn: &Connection) -> Result<String, String> {
    let stored: Option<Option<String>> = conn
        .query_row("SELECT units FROM farm_config WHERE id = 1", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?;
    let system = stored
        .flatten()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| is_sealed(s))
        .unwrap_or_else(|| DEFAULT_UNITS.to_string());
    Ok(system)
}

pub fn farm_units_view(conn: &Connection) -> Result<FarmUnitsView, String> {
    let system = farm_units(conn)?;
    Ok(FarmUnitsView {
        name: name_for(&system),
        unit: unit_for(&system),
        system,
        choices: choices(),
    })
}

/// Settings writes the display system. Config, not an event: no Kind, no
/// conversion, and nothing already weighed moves.
pub fn set_farm_units(conn: &Connection, system: &str) -> Result<FarmUnitsView, String> {
    if !is_sealed(system) {
        return Err(format!(
            "units must be one of: {} (GT-D27 UNITS)",
            sealed_systems_line()
        ));
    }
    let stored = system.trim().to_ascii_lowercase();
    conn.execute(
        "UPDATE farm_config SET units = ?1 WHERE id = 1",
        rusqlite::params![stored],
    )
    .map_err(|e| e.to_string())?;
    farm_units_view(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn units_seal_takes_both_systems_in_any_case_and_refuses_the_rest() {
        for ok in ["imperial", "metric", "IMPERIAL", " Metric "] {
            assert!(is_sealed(ok), "{ok} is sealed this chip");
        }
        for no in ["us", "si", "kg", "stone", ""] {
            assert!(!is_sealed(no), "{no} is not sealed this chip");
        }
    }

    #[test]
    fn units_null_reads_imperial_and_the_view_carries_both_choices() {
        let conn = db::open_in_memory().unwrap();
        assert_eq!(farm_units(&conn).unwrap(), DEFAULT_UNITS);
        let view = farm_units_view(&conn).unwrap();
        assert_eq!(view.system, "imperial");
        assert_eq!(view.name, "Imperial (ounces)");
        assert_eq!(view.unit, "oz");
        let systems: Vec<String> = view.choices.iter().map(|c| c.system.clone()).collect();
        assert_eq!(systems, vec!["imperial".to_string(), "metric".to_string()]);
    }

    #[test]
    fn units_set_stores_a_sealed_system_and_refuses_an_unsealed_one() {
        let conn = db::open_in_memory().unwrap();
        let view = set_farm_units(&conn, "METRIC").unwrap();
        assert_eq!(view.system, "metric");
        assert_eq!(view.unit, "g");
        assert_eq!(farm_units(&conn).unwrap(), "metric");
        let err = set_farm_units(&conn, "kg").unwrap_err();
        assert!(err.contains(&sealed_systems_line()), "{err}");
        assert_eq!(
            farm_units(&conn).unwrap(),
            "metric",
            "a refusal writes nothing"
        );
    }

    #[test]
    fn units_an_unsealed_stored_value_reads_as_the_default() {
        let conn = db::open_in_memory().unwrap();
        conn.execute("UPDATE farm_config SET units = 'stone' WHERE id = 1", [])
            .unwrap();
        assert_eq!(farm_units(&conn).unwrap(), DEFAULT_UNITS);
    }

    #[test]
    fn units_the_gram_table_is_the_signed_fixture() {
        assert_eq!(OZ_TO_G, 28.349523125);
        for (oz, g) in [
            (0.1, 3),
            (0.6, 17),
            (1.0, 28),
            (2.0, 57),
            (2.4, 68),
            (8.0, 227),
            (9.1, 258),
            (12.0, 340),
            (16.0, 454),
            (72.0, 2041),
            (100.3, 2843),
        ] {
            assert_eq!(grams(oz), g, "{oz} oz is {g} g");
        }
        assert_eq!(grams(f64::NAN), 0, "a non-finite weight prints nothing");
        assert_eq!(grams(0.0), 0);
    }

    /// UNITS J6-R9, PIN A SHAPE B. The desk twin (src/farm/mass.ts) has no
    /// test runner, so its constant, its rounding line and its non-finite
    /// guard are pinned here, read off disk the way dock_port_tests reads
    /// healthScopes.ts. The eleven rows above stay Rust's; the desk is held to
    /// the same bytes, not to a second table.
    #[test]
    fn units_the_desk_twin_carries_the_same_constant_and_rounding_rule() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/farm/mass.ts");
        let desk = std::fs::read_to_string(&path).expect("read mass.ts");
        assert!(
            desk.contains("export const OZ_TO_G = 28.349523125;"),
            "mass.ts must carry the one PRECISION G constant"
        );
        assert!(
            desk.contains("return Math.round(oz * OZ_TO_G);"),
            "mass.ts grams must round once, the way units::grams does"
        );
        assert!(
            desk.contains("if (!Number.isFinite(oz)) return 0;"),
            "mass.ts grams must keep the non-finite guard"
        );
    }

    #[test]
    fn units_a_tenth_of_an_ounce_never_collapses_into_one_gram_figure() {
        let mut previous = grams(0.1);
        for step in 2..=200 {
            let next = grams(f64::from(step) / 10.0);
            assert!(
                next > previous,
                "0.1 oz is 2.8 g: step {step} must print a larger gram figure"
            );
            previous = next;
        }
    }
}
