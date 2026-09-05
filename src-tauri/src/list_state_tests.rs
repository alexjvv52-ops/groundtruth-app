//! PACK-LIST-STATE (audit R-11 + R-20) — the wholesale list files voided as
//! history and keeps live-collect doors off settled rows.
//!
//! What is pinned, from the tree the way correction_tests c12 pins desk
//! prose: SETTLED_STATES names paid, written off AND voided, and both list
//! predicates read it, so a voided order sits under Settled and never in To
//! collect; the list sorts the unfiltered reader rows while counts, the owed
//! figure and the pickers keep reading the voided-free liveWholesale; the
//! voided stateLabel is the Void control's word, never the raw token; the
//! wholesale link + Copy + QR block is gated off settled rows; the leftover
//! block keeps its paid gate; and the Settled caption names all three
//! members.
use std::fs;
use std::path::Path;
fn money_src() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/screens/Money.tsx");
    fs::read_to_string(&path).expect("read Money.tsx")
}
#[test]
fn list_state_voided_is_settled_never_to_collect() {
    let src = money_src();
    let set = r#"const SETTLED_STATES = new Set(["paid", "written_off", "voided"]);"#;
    assert!(src.contains(set), "SETTLED_STATES must name voided");
    assert!(
        src.contains("const liveRows = wholesaleList.filter((o) => !SETTLED_STATES.has(o.state));"),
        "To collect must exclude every settled state"
    );
    assert!(
        src.contains(
            "const settledRows = wholesaleList.filter((o) => SETTLED_STATES.has(o.state));"
        ),
        "Settled must be fed by the same set"
    );
    assert!(
        src.contains("const wholesaleList = sortForCollection(wholesale);"),
        "the list must sort the unfiltered reader rows so voided can render"
    );
}
#[test]
fn list_state_counts_and_pickers_keep_excluding_voided() {
    let src = money_src();
    assert!(
        src.contains(r#"const liveWholesale = wholesale.filter((o) => o.state !== "voided");"#),
        "counts, owed figure and pickers must keep reading the voided-free rows"
    );
    assert!(
        src.contains("Paid, written off, and voided. No action here."),
        "the Settled caption must name its third member"
    );
}
#[test]
fn list_state_voided_label_is_the_control_word_not_the_token() {
    let src = money_src();
    assert!(
        src.contains(r#"if (state === "voided") return "Void";"#),
        "stateLabel must map voided to the Void control's word"
    );
}
#[test]
fn list_state_settled_wholesale_rows_draw_no_payment_qr() {
    let src = money_src();
    let gated = "{!SETTLED_STATES.has(o.state) && o.paymentLinkUrl != null && (";
    assert_eq!(
        src.matches(gated).count(),
        1,
        "the wholesale link + Copy + QR block must be gated off settled rows"
    );
    let ungated = "{o.paymentLinkUrl != null && (";
    assert_eq!(
        src.matches(ungated).count(),
        0,
        "no ungated wholesale payment-link block may remain"
    );
}
#[test]
fn list_state_paid_leftover_row_keeps_drawing_no_qr() {
    let src = money_src();
    assert!(
        src.contains("{l.paidAt == null && l.paymentLinkUrl != null && ("),
        "the leftover link + QR block stays behind its paid gate"
    );
}
