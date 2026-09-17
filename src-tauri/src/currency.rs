//! GT-D26 WORLD-PAY (J1 PRESENTMENT-STORE). One farm currency, read from
//! `farm_config` (v41 config row, id = 1). PRESENTMENT only: new mints post
//! this code, every event that already carries a currency keeps the code it
//! was minted with, and nothing here converts anything into anything.

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

/// SCOPE B, staged. This chip seals the eight codes the tree already mints and
/// replays, each with the glyph its printers put in front of an amount. The
/// list is append-only and widens only by a signed job (J4); a code the desk's
/// printers cannot speak never joins it. One table — no printer carries a
/// second one.
const SEALED: &[(&str, &str, &str)] = &[
    ("usd", "US dollars", "$"),
    ("cad", "Canadian dollars", "$"),
    ("eur", "Euros", "€"),
    ("gbp", "British pounds", "£"),
    ("zar", "South African rand", "R"),
    ("inr", "Indian rupees", "₹"),
    ("kes", "Kenyan shillings", "KSh"),
    ("thb", "Thai baht", "฿"),
];

/// A farm whose `farm_config.currency` is NULL reads `usd`: no farm changes
/// the currency it mints in by upgrading.
pub const DEFAULT_CURRENCY: &str = "usd";
const PAY_LINE_KEY_REFUSAL: &str =
    "Refusing to save a how-to-pay line that looks like it contains a Stripe key.";
const PAY_LINE_CAP: usize = 120;
const PAY_LINE_CAP_REFUSAL: &str = "How to pay is capped at 120 characters. Shorten it.";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrencyChoice {
    pub code: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FarmCurrencyView {
    pub code: String,
    pub name: String,
    pub symbol: String,
    pub choices: Vec<CurrencyChoice>,
}

/// The one seal. Lowercases both sides: a stored or imported `USD` is the
/// same code as `usd`, and anything off the list is refused.
pub fn is_sealed(code: &str) -> bool {
    let code = code.trim().to_ascii_lowercase();
    SEALED.iter().any(|(c, _, _)| *c == code)
}

/// The sealed codes as one printable line, for refusals. No refusal spells
/// the list out by hand.
pub fn sealed_codes_line() -> String {
    SEALED
        .iter()
        .map(|(c, _, _)| *c)
        .collect::<Vec<_>>()
        .join(", ")
}

fn name_for(code: &str) -> String {
    let lower = code.trim().to_ascii_lowercase();
    SEALED
        .iter()
        .find(|(c, _, _)| *c == lower)
        .map(|(_, n, _)| (*n).to_string())
        .unwrap_or_else(|| lower.to_uppercase())
}

/// The glyph a printer puts in front of an amount in this currency. The one
/// table: no printer decides a symbol for itself. An unsealed code cannot
/// reach here through `farm_currency`, so the fallback is unreachable by
/// construction — it returns the code itself, because an honest `EUR 12.50`
/// beats a wrong `$12.50`.
pub fn symbol_for(code: &str) -> String {
    let lower = code.trim().to_ascii_lowercase();
    SEALED
        .iter()
        .find(|(c, _, _)| *c == lower)
        .map(|(_, _, s)| (*s).to_string())
        .unwrap_or_else(|| lower.to_uppercase())
}

fn choices() -> Vec<CurrencyChoice> {
    SEALED
        .iter()
        .map(|(c, n, _)| CurrencyChoice {
            code: (*c).to_string(),
            name: (*n).to_string(),
        })
        .collect()
}

/// The farm currency every new mint is billed in. NULL, blank or a value this
/// build is not sealed for reads as `usd` — a farm file from a wider build
/// never mints a code this build cannot speak.
pub fn farm_currency(conn: &Connection) -> Result<String, String> {
    let stored: Option<Option<String>> = conn
        .query_row("SELECT currency FROM farm_config WHERE id = 1", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?;
    let code = stored
        .flatten()
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| is_sealed(c))
        .unwrap_or_else(|| DEFAULT_CURRENCY.to_string());
    Ok(code)
}

pub fn farm_currency_view(conn: &Connection) -> Result<FarmCurrencyView, String> {
    let code = farm_currency(conn)?;
    Ok(FarmCurrencyView {
        name: name_for(&code),
        symbol: symbol_for(&code),
        code,
        choices: choices(),
    })
}

/// Settings writes the farm currency. Config, not an event: no Kind, no
/// conversion, and nothing already minted moves.
pub fn set_farm_currency(conn: &Connection, code: &str) -> Result<FarmCurrencyView, String> {
    if !is_sealed(code) {
        return Err(format!(
            "currency must be one of: {} (GT-D26 WORLD-PAY)",
            sealed_codes_line()
        ));
    }
    let stored = code.trim().to_ascii_lowercase();
    conn.execute(
        "UPDATE farm_config SET currency = ?1 WHERE id = 1",
        rusqlite::params![stored],
    )
    .map_err(|e| e.to_string())?;
    farm_currency_view(conn)
}

pub fn farm_pay_instructions(conn: &Connection) -> Result<Option<String>, String> {
    let stored: Option<Option<String>> = conn
        .query_row(
            "SELECT pay_instructions FROM farm_config WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(stored
        .flatten()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty()))
}

pub fn set_farm_pay_instructions(conn: &Connection, text: &str) -> Result<Option<String>, String> {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("rk_") || lower.contains("sk_") {
        return Err(PAY_LINE_KEY_REFUSAL.to_string());
    }
    if trimmed.chars().count() > PAY_LINE_CAP {
        return Err(PAY_LINE_CAP_REFUSAL.to_string());
    }
    let value = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    };
    conn.execute(
        "UPDATE farm_config SET pay_instructions = ?1 WHERE id = 1",
        rusqlite::params![value],
    )
    .map_err(|e| e.to_string())?;
    farm_pay_instructions(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn currency_seal_takes_usd_and_cad_in_any_case_and_refuses_the_rest() {
        for ok in [
            "usd", "cad", "USD", " Cad ", "eur", "gbp", "zar", "inr", "kes", "thb",
        ] {
            assert!(is_sealed(ok), "{ok} is sealed this chip");
        }
        for no in ["krw", "vnd", "xxx", ""] {
            assert!(!is_sealed(no), "{no} is not sealed this chip");
        }
    }

    #[test]
    fn currency_null_reads_usd_and_the_view_carries_both_choices() {
        let conn = db::open_in_memory().unwrap();
        assert_eq!(farm_currency(&conn).unwrap(), DEFAULT_CURRENCY);
        let view = farm_currency_view(&conn).unwrap();
        assert_eq!(view.code, "usd");
        assert_eq!(view.name, "US dollars");
        assert_eq!(view.symbol, "$");
        let codes: Vec<String> = view.choices.iter().map(|c| c.code.clone()).collect();
        assert_eq!(
            codes,
            vec![
                "usd".to_string(),
                "cad".to_string(),
                "eur".to_string(),
                "gbp".to_string(),
                "zar".to_string(),
                "inr".to_string(),
                "kes".to_string(),
                "thb".to_string()
            ]
        );
    }

    #[test]
    fn currency_set_stores_a_sealed_code_and_refuses_an_unsealed_one() {
        let conn = db::open_in_memory().unwrap();
        let view = set_farm_currency(&conn, "CAD").unwrap();
        assert_eq!(view.code, "cad");
        assert_eq!(farm_currency(&conn).unwrap(), "cad");
        let err = set_farm_currency(&conn, "xxx").unwrap_err();
        assert!(err.contains(&sealed_codes_line()), "{err}");
        assert_eq!(
            farm_currency(&conn).unwrap(),
            "cad",
            "a refusal writes nothing"
        );
    }

    #[test]
    fn currency_an_unsealed_stored_value_reads_as_the_default() {
        let conn = db::open_in_memory().unwrap();
        conn.execute("UPDATE farm_config SET currency = 'krw' WHERE id = 1", [])
            .unwrap();
        assert_eq!(farm_currency(&conn).unwrap(), DEFAULT_CURRENCY);
    }

    #[test]
    fn currency_both_sealed_codes_print_a_dollar_sign_from_the_one_table() {
        assert_eq!(SEALED.len(), 8, "eight rows this chip — J4 widens the list");
        for (code, _, _) in SEALED {
            assert!(is_sealed(code), "{code} is on the seal");
        }
        assert_eq!(symbol_for("usd"), "$");
        assert_eq!(symbol_for("cad"), "$");
        assert_eq!(symbol_for(" CAD "), "$", "the table trims and lowercases");
    }

    #[test]
    fn currency_pay_line_stores_trimmed_and_an_empty_save_clears_it() {
        let conn = db::open_in_memory().unwrap();
        assert_eq!(farm_pay_instructions(&conn).unwrap(), None);
        let stored = set_farm_pay_instructions(&conn, "  M-Pesa Till 123456  ").unwrap();
        assert_eq!(stored.as_deref(), Some("M-Pesa Till 123456"));
        assert_eq!(
            farm_pay_instructions(&conn).unwrap().as_deref(),
            Some("M-Pesa Till 123456")
        );
        let cleared = set_farm_pay_instructions(&conn, "   ").unwrap();
        assert_eq!(cleared, None);
        assert_eq!(farm_pay_instructions(&conn).unwrap(), None);
    }

    #[test]
    fn currency_pay_line_refuses_a_stripe_key_and_writes_nothing() {
        let conn = db::open_in_memory().unwrap();
        set_farm_pay_instructions(&conn, "M-Pesa Till 123456").unwrap();
        for bad in ["rk_test_xxx", "sk_live_xxx", "  RK_foo  ", "pay sk_bar"] {
            let err = set_farm_pay_instructions(&conn, bad).unwrap_err();
            assert_eq!(err, PAY_LINE_KEY_REFUSAL, "{err}");
        }
        assert_eq!(
            farm_pay_instructions(&conn).unwrap().as_deref(),
            Some("M-Pesa Till 123456"),
            "a refusal writes nothing"
        );
    }

    #[test]
    fn currency_pay_line_refuses_over_the_cap_and_writes_nothing() {
        let conn = db::open_in_memory().unwrap();
        set_farm_pay_instructions(&conn, "M-Pesa Till 123456").unwrap();
        let err = set_farm_pay_instructions(&conn, &"x".repeat(121)).unwrap_err();
        assert_eq!(err, PAY_LINE_CAP_REFUSAL, "{err}");
        assert_eq!(
            farm_pay_instructions(&conn).unwrap().as_deref(),
            Some("M-Pesa Till 123456"),
            "a refusal writes nothing"
        );
        let ok = set_farm_pay_instructions(&conn, &"x".repeat(120)).unwrap();
        assert_eq!(ok.as_deref().map(|s| s.chars().count()), Some(120));
    }
}
