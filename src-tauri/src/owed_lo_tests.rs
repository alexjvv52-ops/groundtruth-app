//! OWED-LO (audit R-1 + R-2) — a priced, unpaid leftover listing is owed
//! money, and the owed surfaces must say so.
//!
//! What is pinned: leftover::owed_leftover is the one evaluator (priced AND
//! unpaid, ruling 1) and both owed surfaces read it — wholesale::owed_summary
//! for the Money / Today owed line, health compute for M1; a minted unpaid
//! listing makes M1 and the owed summary non-empty by its cents; a cash or
//! Stripe close takes the cents back out; an unpriced listing is never owed;
//! wholesale delivered-unpaid still works and the two amounts join; MoneyDebts
//! stays wholesale-only so Today raises no leftover card (ruling 3); and the
//! desk's two owedLine mirrors and Today's visibility gate carry the same
//! leftover terms.
use crate::attention::{self, CollectDebt, MoneyDebts};
use crate::db;
use crate::health::{severity_for, CheckInputs, CheckStatus, Severity};
use crate::leftover::{self, LeftoverOwed};
use crate::marketing;
use crate::money::fake::FakeGateway;
use crate::projection;
use crate::trays;
use crate::wholesale::{self, OrderLine};
use rusqlite::Connection;
use std::fs;
use std::path::Path;
const NOW: &str = "2026-08-29T12:00:00.000Z";
fn mem() -> Connection {
    db::open_in_memory().unwrap()
}
fn today() -> String {
    db::local_date_today()
}
/// Two kale trays harvested today at 6.0 oz, listed at 2.0 oz — the same
/// fixture lo_b_cash_tests uses.
fn listed_kale(conn: &mut Connection) -> leftover::LeftoverListingView {
    let mut ids = Vec::new();
    for _ in 0..2 {
        let t = trays::sow_tray(conn, "kale", 1).unwrap();
        trays::advance_tray(conn, &t.id).unwrap();
        ids.push(t.id);
    }
    trays::harvest_trays(conn, &ids, 6.0).unwrap();
    leftover::list_leftover(conn, "kale", &today(), 2.0).unwrap()
}
/// One dun-peas order at today's harvest date, priced 500, delivered —
/// wholesale delivered-unpaid as wholesale_tests builds it.
fn delivered_order(conn: &mut Connection) {
    let v = marketing::record_venue(conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap();
    let order = wholesale::record_order(
        conn,
        &v.venue_id,
        &today(),
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(500),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(conn, &order.id, None).unwrap();
}
/// M1 from the same two readers health.rs compute_status feeds it from.
fn m1_for(conn: &Connection) -> CheckStatus {
    let money = attention::money_debts(conn).unwrap();
    let leftover_owed = leftover::owed_leftover(conn).unwrap();
    let inputs = CheckInputs {
        money,
        leftover_owed,
        currency: "usd".into(),
        ..CheckInputs::default()
    };
    severity_for("M1", None, NOW, &inputs)
}
#[test]
fn owed_lo_minted_unpaid_listing_is_owed_by_its_cents() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    // Ruling 1: priced AND unpaid — counted.
    let lo = leftover::owed_leftover(&conn).unwrap();
    assert_eq!(lo.count, 1);
    assert_eq!(lo.cents, 700);
    // The owed line's summary carries the cents beside the wholesale figures.
    let owed = wholesale::owed_summary(&conn).unwrap();
    assert_eq!(owed.deliveries, 0);
    assert_eq!(owed.total_cents, None, "wholesale total keeps its meaning");
    assert_eq!(owed.leftover_count, 1);
    assert_eq!(owed.leftover_cents, 700);
    // M1 must not print the empty sentence, and the amount is the cents.
    let s = m1_for(&conn);
    assert_eq!(s.severity, Severity::Degraded, "{}", s.sentence);
    assert_eq!(
        s.sentence,
        "M1 Owed to you — USD 7.00 across 1 leftover listing. Collect the oldest on the Money tab."
    );
}
#[test]
fn owed_lo_cash_close_takes_the_cents_out_of_owed() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    leftover::pay_listing_cash(&mut conn, &listing.listing_id, 700, &today(), None).unwrap();
    let lo = leftover::owed_leftover(&conn).unwrap();
    assert_eq!(lo.count, 0);
    assert_eq!(lo.cents, 0);
    let owed = wholesale::owed_summary(&conn).unwrap();
    assert_eq!(owed.leftover_count, 0);
    assert_eq!(owed.leftover_cents, 0);
    let s = m1_for(&conn);
    assert_eq!(s.severity, Severity::Healthy);
    assert_eq!(s.sentence, "M1 Owed to you — nothing owed to you.");
}
#[test]
fn owed_lo_stripe_close_takes_the_cents_out_of_owed() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let now = projection::handler_now();
    leftover::pay_listing_from_link_session(
        &mut conn,
        &listing.listing_id,
        "cs_owed",
        None,
        700,
        &now,
    )
    .unwrap();
    let lo = leftover::owed_leftover(&conn).unwrap();
    assert_eq!(lo.count, 0);
    assert_eq!(lo.cents, 0);
    let s = m1_for(&conn);
    assert_eq!(s.severity, Severity::Healthy);
    assert_eq!(s.sentence, "M1 Owed to you — nothing owed to you.");
}
#[test]
fn owed_lo_unpriced_listing_is_not_owed() {
    let mut conn = mem();
    let _listing = listed_kale(&mut conn);
    // Ruling 1: listed but never priced — no dollar exists, nothing is owed.
    let lo = leftover::owed_leftover(&conn).unwrap();
    assert_eq!(lo.count, 0);
    assert_eq!(lo.cents, 0);
    let owed = wholesale::owed_summary(&conn).unwrap();
    assert_eq!(owed.leftover_count, 0);
    assert_eq!(owed.leftover_cents, 0);
    let s = m1_for(&conn);
    assert_eq!(s.severity, Severity::Healthy);
    assert_eq!(s.sentence, "M1 Owed to you — nothing owed to you.");
}
#[test]
fn owed_lo_wholesale_delivered_unpaid_still_counts_and_joins_leftover() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    delivered_order(&mut conn);
    let owed = wholesale::owed_summary(&conn).unwrap();
    assert_eq!(
        owed.deliveries, 1,
        "wholesale delivered-unpaid still counts"
    );
    assert_eq!(owed.total_cents, Some(500));
    assert_eq!(owed.leftover_count, 1);
    assert_eq!(owed.leftover_cents, 700);
    // Ruling 3: MoneyDebts stays wholesale-only — Today raises no leftover
    // card. The listing is owed, and it is not a collect debt.
    let money = attention::money_debts(&conn).unwrap();
    assert_eq!(money.collect.len(), 1);
    assert_eq!(money.collect[0].cents, 500);
    // M1 joins the two amounts: $5.00 wholesale + $7.00 leftover.
    let s = m1_for(&conn);
    assert_eq!(s.severity, Severity::Degraded, "{}", s.sentence);
    assert_eq!(
        s.sentence,
        "M1 Owed to you — USD 12.00 across 1 delivery and 1 leftover listing, delivered today. \
         Collect the oldest on the Money tab."
    );
}
#[test]
fn owed_lo_m1_wording_holds_and_leftover_alone_never_escalates() {
    // Leftover-only is Degraded at any size — it carries no delivered age,
    // and only age crosses OWED_UNHEALTHY_DAYS. Existing scale, no new band.
    let big = CheckInputs {
        leftover_owed: LeftoverOwed {
            count: 3,
            cents: 123_456,
        },
        currency: "usd".into(),
        ..CheckInputs::default()
    };
    let s = severity_for("M1", None, NOW, &big);
    assert_eq!(s.severity, Severity::Degraded, "{}", s.sentence);
    assert_eq!(
        s.sentence,
        "M1 Owed to you — USD 1234.56 across 3 leftover listings. \
         Collect the oldest on the Money tab."
    );
    // An unpriced wholesale debt beside leftover: still never a fake total.
    let unpriced = CheckInputs {
        money: MoneyDebts {
            collect: vec![CollectDebt {
                order_id: "order-unpriced".into(),
                venue_name: "Harvest Table".into(),
                delivered_on: "2026-08-01".into(),
                days: 2,
                age_countable: true,
                any_unpriced: true,
                cents: 0,
                message: "Collect".into(),
            }],
            ..MoneyDebts::default()
        },
        leftover_owed: LeftoverOwed {
            count: 1,
            cents: 700,
        },
        currency: "usd".into(),
        ..CheckInputs::default()
    };
    let s = severity_for("M1", None, NOW, &unpriced);
    assert!(!s.sentence.contains("USD"), "no fake total: {}", s.sentence);
    assert!(
        s.sentence
            .contains("1 delivery and 1 leftover listing, value partly unpriced"),
        "{}",
        s.sentence
    );
}
#[test]
fn owed_lo_desk_and_today_mirror_the_leftover_gate() {
    // Same convention as correction_tests c12: the desk prose is pinned from
    // the tree, so a one-sided edit of either owedLine mirror — or of Today's
    // visibility gate — goes red here.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let money = fs::read_to_string(root.join("../src/screens/Money.tsx")).expect("read Money.tsx");
    let today_src =
        fs::read_to_string(root.join("../src/screens/Today.tsx")).expect("read Today.tsx");
    let gate = r#"if (o.deliveries === 0 && o.leftoverCount === 0) return "Nothing owed to you.";"#;
    let clause = r#"leftover ${o.leftoverCount === 1 ? "listing" : "listings"}"#;
    for (name, src) in [("Money.tsx", &money), ("Today.tsx", &today_src)] {
        assert!(src.contains(gate), "{name} lost the leftover empty-gate");
        assert!(src.contains(clause), "{name} lost the leftover clause");
    }
    // Today's visibility gate: leftover-only owed renders the line; a clean
    // morning still hides "Nothing owed to you." (B1-F1 D4 kept).
    let visibility_gate = "owed == null || owed.deliveries > 0 || owed.leftoverCount > 0";
    assert!(
        today_src.contains(visibility_gate),
        "Today.tsx owed line must render for leftover-only owed"
    );
    let types = fs::read_to_string(root.join("../src/farm/types.ts")).expect("read types.ts");
    assert!(types.contains("leftoverCount: number"), "types.ts field");
    assert!(types.contains("leftoverCents: number"), "types.ts field");
}
