//! PACK-LO-SPEAK (audit R-3 + R-4 + R-9 + R-17 + R-18 rider) — leftover money
//! already in the file stops wearing wholesale clothes.
//!
//! What is pinned, from the tree the way correction_tests c12 pins desk prose:
//! the two leftover unapplied tokens render leftover sentences (never "order",
//! never the raw token as the primary line); Books' Cash collected caption
//! names leftover listings only when leftover income is in the period and is
//! byte-identical to the old sentence otherwise; a paid leftover row shows its
//! cents exactly once, guarded so unpriced leftover invents no dollar; the
//! cash price sentence has one source — the door — with no TSX duplicate; and
//! the leftover test files carry no ignore attribute and are all registered in
//! lib.rs, so unfiltered `cargo test` runs every one of them.
use std::fs;
use std::path::Path;
fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}
#[test]
fn lo_speak_leftover_unapplied_sentences_say_leftover_not_order() {
    let money = read("../src/screens/Money.tsx");
    let already_paid =
        "A payment link was paid on a leftover listing that is already paid. Nothing was recorded; the money is at Stripe.";
    let mismatch =
        "A payment link was paid for an amount that is not the leftover listing's total. Nothing was recorded. Record it with Paid…";
    for (token, sentence) in [
        ("leftover_already_paid", already_paid),
        ("leftover_amount_mismatch", mismatch),
    ] {
        assert!(
            money.contains(&format!("status === \"{token}\"")),
            "unappliedSentence lost the {token} arm"
        );
        assert!(money.contains(sentence), "{token} sentence changed");
        assert!(
            sentence.contains("leftover listing") && !sentence.contains("order"),
            "{token} must speak leftover, never order"
        );
    }
}
#[test]
fn lo_speak_paid_leftover_row_shows_its_cents_once_and_only_when_priced() {
    let money = read("../src/screens/Money.tsx");
    // V-3 (d293434) gave this span the wholesale row's amount classes; the
    // needle follows the tree, not the bare span PACK-LO-SPEAK first landed.
    let span =
        "<span className=\"order-first text-base font-semibold tabular-nums\">{cents(l.pricedTotalCents)}</span>";
    assert_eq!(
        money.matches(span).count(),
        1,
        "the paid leftover row shows its total exactly once (ruling 5)"
    );
    assert_eq!(
        money.matches("cents(l.pricedTotalCents)").count(),
        1,
        "the leftover total renders through cents() once, whatever wraps it (ruling 5)"
    );
    let guard = "{l.paidAt != null && l.pricedTotalCents != null && (";
    assert!(
        money.contains(guard),
        "the leftover money span must be guarded paid AND priced — no dollar from ounces"
    );
}
#[test]
fn lo_speak_books_caption_names_leftover_only_when_present() {
    let books = read("../src/screens/Books.tsx");
    let with_leftover =
        "Every payment dated in this period — wholesale payments recorded on Money, paid online orders, and leftover listings paid on Money. Each dollar counted once.";
    let without =
        "Every payment dated in this period — wholesale payments recorded on Money, and paid online orders. Each dollar counted once.";
    assert!(
        books.contains("leftoverInPeriod"),
        "Books lost the leftover-period gate"
    );
    assert!(
        books.contains(with_leftover),
        "leftover caption branch changed"
    );
    assert!(
        books.contains(without),
        "zero-leftover caption must stay byte-identical"
    );
    assert!(
        books.contains("row.source.startsWith(\"Leftover \")"),
        "detection keys on the machine-written income source (leftover.rs:538,607)"
    );
}
#[test]
fn lo_speak_cash_price_sentence_has_one_source_the_door() {
    let money = read("../src/screens/Money.tsx");
    assert!(
        !money.contains("Price leftover in dollars before it can be marked paid."),
        "the cash price sentence lives only in leftover.rs LEFTOVER_CASH_UNPRICED_LINE"
    );
    assert!(
        money.contains("amountCents: amountCents ?? 0,"),
        "an unparsable amount goes to the door as 0 so the door's sentence returns"
    );
    // The door itself still owns the sentence verbatim (its refusal behavior
    // is already pinned by lo_b_cash_tests); here we pin the single source.
    let door = read("src/leftover.rs");
    assert!(door.contains("Price leftover in dollars before it can be marked paid."));
}
#[test]
fn lo_speak_leftover_tests_run_unfiltered_no_ignore() {
    // R-18 rider (ruling 7): unfiltered `cargo test` runs every leftover test.
    // No ignore attribute in any leftover test file, every file registered in
    // lib.rs, so no filter name can silently shelve them.
    let needle = ["#[", "ignore"].concat();
    let lib = read("src/lib.rs");
    for file in [
        "lo_a_tests",
        "lo_b_tests",
        "lo_b_cash_tests",
        "lo_c_tests",
        "owed_lo_tests",
        "lo_speak_tests",
    ] {
        let src = read(&format!("src/{file}.rs"));
        assert!(!src.contains(&needle), "{file} must not be ignored");
        assert!(src.contains("#[test]"), "{file} must hold tests");
        assert!(
            lib.contains(&format!("mod {file};")),
            "{file} not registered in lib.rs"
        );
    }
}
