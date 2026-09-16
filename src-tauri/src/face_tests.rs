//! PACK-FACE (audit R-12 + R-13) — the bill sits in the face on Money, and the
//! name printed on it comes from the one farm_config reader Settings already
//! uses.
//!
//! What is pinned, from the tree the way list_state_tests pins desk order: the
//! invoice sheet renders exactly once and sits inside the Wholesale section,
//! after the To collect group and before the Settled group, so a live unpaid
//! wholesale or leftover row opens its bill without first opening "Show N
//! older settled" — and it still sits above Leftover, the tree's own slot, not
//! a third one; the INV-A field bytes are unchanged (the SEND-THE-BILL title,
//! line, total, Print, Close, window.print); Money loads the display name through
//! farmDisplayName(), the same reader and the same command Settings uses, with
//! no second store and no setter on Money; and a failed load prints the
//! errMessage sentence, never String(e). No SCHEMA_VERSION literal here —
//! H-11(b) keeps that pin in its three files.
use std::fs;
use std::path::Path;
fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}
fn money() -> String {
    read("../src/screens/Money.tsx")
}
fn at(src: &str, needle: &str) -> usize {
    src.find(needle)
        .unwrap_or_else(|| panic!("Money.tsx lost: {needle}"))
}
#[test]
fn face_invoice_mounts_under_to_collect_and_above_settled() {
    let src = money();
    assert_eq!(
        src.matches("invoice-print").count(),
        1,
        "exactly one invoice sheet on Money — no third slot"
    );
    let to_collect = at(
        &src,
        r#"<h3 className="text-base font-medium">To collect ({liveRows.length})</h3>"#,
    );
    let bill = at(&src, "{invoiceBill != null && (");
    let settled = at(&src, "{settledRows.length > 0 && (");
    // J1-PINS: on the live tree Leftover is a Card on Money, not an h2.
    let leftover = at(&src, "<CardTitle>Leftover</CardTitle>");
    assert!(
        to_collect < bill,
        "the bill mounts under the live To collect group"
    );
    assert!(
        bill < settled,
        "the bill mounts above the Settled group, not under settled history"
    );
    assert!(bill < leftover, "the bill stays above Leftover");
}
#[test]
fn face_invoice_is_not_behind_the_settled_disclosure() {
    let src = money();
    let bill = at(&src, "{invoiceBill != null && (");
    let show = at(
        &src,
        "Show {settledRows.length - SETTLED_VISIBLE} older settled",
    );
    assert!(
        bill < show,
        "a live unpaid row reaches its bill without opening the older-settled control"
    );
    assert!(
        !src.contains("showAllSettled && invoiceBill"),
        "the bill is never gated on the settled disclosure"
    );
    for opener in [
        "onClick={() => void onInvoiceWholesale(o)}",
        "onClick={() => void onInvoiceLeftover(l)}",
    ] {
        assert_eq!(
            src.matches(opener).count(),
            1,
            "both live doors keep one Invoice opener: {opener}"
        );
    }
}
#[test]
fn face_invoice_keeps_the_inv_a_field_bytes() {
    let src = money();
    let fields = [
        r#"<p className="text-sm">{billTitle(invoiceBill)}</p>"#,
        "{line.cropName} · {line.trays} trays × {cents(line.priceCentsPerTray)} = {cents(line.lineTotalCents)}",
        r#"<p className="text-base font-medium">Total {cents(invoiceBill.totalCents)}</p>"#,
        "onClick={() => window.print()}",
        "onClick={() => setInvoiceBill(null)}",
        "\n                  Print\n",
        "\n                  Close\n",
    ];
    for field in fields {
        assert_eq!(
            src.matches(field).count(),
            1,
            "INV-A field byte changed or duplicated: {field}"
        );
    }
}
#[test]
fn face_money_loads_the_farm_name_through_the_settings_reader() {
    let src = money();
    assert!(
        src.contains("\n  farmDisplayName,\n"),
        "Money imports the reader Settings uses"
    );
    assert!(
        src.contains("\n        farmDisplayName(),\n"),
        "the reader is called in Money's load()"
    );
    assert!(
        src.contains("setFarmName(farmNameRow);"),
        "the loaded name lands in Money's render state"
    );
    assert_eq!(
        src.matches(r#"<h2 className="text-lg font-medium">{farmName}</h2>"#)
            .count(),
        1,
        "the invoice header prints the name Money loaded"
    );
    assert_eq!(
        src.matches("invoiceBill.farmName").count(),
        0,
        "one source: the header reads the load, not a second name off the bill"
    );
    assert_eq!(
        src.matches("setFarmDisplayName").count(),
        0,
        "Money never writes the farm name — Settings owns the save"
    );
    let settings = read("../src/screens/Settings.tsx");
    assert!(
        settings.contains("const current = await farmDisplayName();"),
        "Settings keeps the same reader, unchanged"
    );
    let api = read("../src/farm/api.ts");
    assert_eq!(
        api.matches(r#"invoke("farm_display_name")"#).count(),
        1,
        "one door to the farm_config display name"
    );
    let door = read("src/invoice.rs");
    assert!(
        door.contains("SELECT display_name FROM farm_config WHERE id = 1"),
        "the reader still reads farm_config — no second farm-name store"
    );
}
#[test]
fn face_failed_farm_name_load_speaks_errmessage_not_string_e() {
    let src = money();
    assert!(
        src.contains("void load().catch((e: unknown) => setError(errMessage(e)));"),
        "the load that now carries the farm name still lands on errMessage"
    );
    assert_eq!(
        src.matches("String(e)").count(),
        0,
        "PACK-STRINGS holds: no raw String(e) on Money"
    );
}
// INTEGRITY-RECEIPT (LINE A / STAMP A / QR A) — the bill's footer line reads
// facts the desk already holds and invents none: the stamp is sliced from the
// one Healthy H4 sentence health.rs formats, the bill carries no code image,
// and Money never runs a verify.
#[test]
fn receipt_stamp_prefix_is_the_h4_healthy_sentence_once_each_side() {
    let src = money();
    let prefix = "H4 Core data intact — quick_check ok, verify passed ";
    assert_eq!(
        src.matches(prefix).count(),
        1,
        "Money carries the desk prefix once — the const the stamp is sliced from"
    );
    let health = read("src/health.rs");
    assert_eq!(
        health
            .matches(r#"format!("H4 Core data intact — quick_check ok, verify passed {when}")"#)
            .count(),
        1,
        "health.rs formats the Healthy H4 sentence from that prefix + when, once"
    );
    assert!(
        src.contains(r#"h4.severity !== "Healthy""#),
        "STAMP A: only a Healthy H4 yields a when"
    );
    assert_eq!(
        src.matches("`VERIFY-REPLAY PASS ${verifyWhen}`").count(),
        1,
        "LINE A: the stamp segment prints the when H4 reported, once"
    );
    for segment in [
        "`Harvest ${invoiceBill.harvestDate}`",
        "`Invoice ${invoiceBill.number}`",
    ] {
        assert_eq!(
            src.matches(segment).count(),
            1,
            "LINE A: footer segment changed or duplicated: {segment}"
        );
    }
    assert!(
        src.contains(".join(\" · \")"),
        "LINE A: segments join on the middle dot the bill already speaks"
    );
}
#[test]
fn receipt_bill_carries_no_code_image() {
    let src = money();
    let start = at(&src, r#"<section className="invoice-print"#);
    let close = at(&src[start..], "</section>");
    let bill = &src[start..start + close];
    for banned in ["<svg", "linkQr", "leftoverQr"] {
        assert_eq!(
            bill.matches(banned).count(),
            0,
            "QR A: nothing drawn from a code inside the bill: {banned}"
        );
    }
}
#[test]
fn receipt_money_reads_the_stored_pass_and_never_runs_verify() {
    let src = money();
    for banned in ["runFullVerify", "run_full_verify"] {
        assert_eq!(
            src.matches(banned).count(),
            0,
            "STAMP A: Money reads the stored pass, never runs a verify: {banned}"
        );
    }
    assert!(
        src.contains("\n  healthStatus,\n"),
        "Money imports the same reader Health uses"
    );
    assert_eq!(
        src.matches("await healthStatus()").count(),
        1,
        "one read of the stored pass on Money"
    );
    assert_eq!(
        src.matches("invoiceBill.farmName").count(),
        0,
        "LINE A: the footer's farm name is Money's loaded state, never the bill's"
    );
}
