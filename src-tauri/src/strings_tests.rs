//! PACK-STRINGS (audit R-15 + R-16) — the desk prints door sentences, never
//! engineering chrome.
//!
//! What is pinned: no operator-visible catch on Money or Settings renders
//! String(e) — the raw form that prefixes "Error: " onto a thrown Error —
//! every caught error goes through the errMessage shape, which passes the
//! door's own sentence through verbatim; and the cash-mismatch sentence has
//! exactly one source, the cash door in leftover.rs (lo_b_cash_tests pins
//! the refusal verbatim), with zero TSX copies — the "zero times if the
//! door owns it" arm of the pack's ruling 6.
use std::fs;
use std::path::Path;
fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}
#[test]
fn strings_pack_money_prints_door_sentences_not_string_e() {
    let money = read("../src/screens/Money.tsx");
    assert_eq!(
        money.matches("setError(String(e))").count(),
        0,
        "Money must render caught errors through errMessage, never String(e)"
    );
    assert_eq!(
        money.matches("Error: ${").count(),
        0,
        "no engineering prefix template on Money"
    );
    assert!(
        money.contains("function errMessage"),
        "Money keeps its errMessage helper"
    );
}
#[test]
fn strings_pack_settings_prints_door_sentences_not_string_e() {
    let settings = read("../src/screens/Settings.tsx");
    assert_eq!(
        settings.matches("setError(String(e))").count(),
        0,
        "Settings must render caught errors through errMessage, never String(e)"
    );
    assert!(
        settings.contains("function errMessage"),
        "Settings keeps its errMessage helper"
    );
}
#[test]
fn strings_pack_cash_mismatch_has_one_source_the_door() {
    let sentence = "cash payment does not match the listing total";
    let door = read("src/leftover.rs");
    assert_eq!(
        door.matches(sentence).count(),
        1,
        "the cash door owns the mismatch sentence exactly once"
    );
    let money = read("../src/screens/Money.tsx");
    assert_eq!(
        money.matches(sentence).count(),
        0,
        "zero TSX copies — the door's sentence reaches the desk through the catch"
    );
}
