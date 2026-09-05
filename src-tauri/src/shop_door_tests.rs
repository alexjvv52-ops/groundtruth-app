//! SHOP DOOR Job A (signed WHAT A · WHERE A · WRITE A · PAY A · CAP A · GT-D24
//! VIEWER OK) — a read-only page of minted, unpaid leftover lots.
//!
//! What is pinned: the composer is a viewer over leftover::listings that
//! writes <farm folder>/shop/index.html — the cart page's path, reused — and
//! nothing else (no event, no offer, no harvest link, no checkout address, no
//! Stripe call); the farm display name gates the write with INV-A's sentence
//! and no file or folder appears on a refusal; an empty leftover book says the
//! one empty sentence; one minted, unpaid lot prints the Money row — crop,
//! harvested day, listed ounces, price — and the stored payment_link_url byte
//! for byte; a paid lot drops off; an unminted lot never appears (no dollar
//! from ounces); the page carries no brand string, no script, no key and
//! stays under the budget; and the desk wiring exists exactly once while
//! SellOnlineSheet keeps zero importers (GT-D24). No SCHEMA_VERSION literal
//! here — H-11(b) keeps that pin in its three files.
use crate::db;
use crate::invoice::{self, INVOICE_NEEDS_FARM_NAME_LINE};
use crate::leftover;
use crate::money::{self, fake::FakeGateway};
use crate::shop::{self, SHOP_DOOR_AVAILABILITY_TAIL, SHOP_DOOR_EMPTY_LINE};
use crate::trays;
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn today() -> String {
    db::local_date_today()
}

fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("groundtruth-shop-door-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn set_name(conn: &Connection) {
    invoice::set_farm_display_name(conn, "Groundtruth Farm").unwrap();
}

/// Two trays of one crop harvested today at 6.0 oz, listed at 2.0 oz.
fn listed(conn: &mut Connection, crop_id: &str) -> leftover::LeftoverListingView {
    let mut ids = Vec::new();
    for _ in 0..2 {
        let t = trays::sow_tray(conn, crop_id, 1).unwrap();
        trays::advance_tray(conn, &t.id).unwrap();
        ids.push(t.id);
    }
    trays::harvest_trays(conn, &ids, 6.0).unwrap();
    leftover::list_leftover(conn, crop_id, &today(), 2.0).unwrap()
}

/// A listed lot with a Payment Link minted at `cents` (LO-B, fake gateway).
fn minted(conn: &mut Connection, crop_id: &str, cents: i64) -> leftover::LeftoverListingView {
    let lot = listed(conn, crop_id);
    leftover::mint_payment_link_with(conn, &FakeGateway::new(), &lot.listing_id, cents).unwrap()
}

fn count_table(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn page(conn: &Connection, dir: &Path) -> (crate::models::ShopPage, String) {
    let written = shop::write_leftover_shop_page(conn, dir).unwrap();
    let html = fs::read_to_string(&written.file_path).unwrap();
    (written, html)
}

fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// Every .ts / .tsx under ../src, as (relative path, source).
fn ts_sources(dir: &Path, out: &mut Vec<(String, String)>) {
    for ent in fs::read_dir(dir).unwrap() {
        let path = ent.unwrap().path();
        if path.is_dir() {
            ts_sources(&path, out);
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.ends_with(".ts") || name.ends_with(".tsx") {
            out.push((
                path.to_string_lossy().into_owned(),
                fs::read_to_string(&path).unwrap(),
            ));
        }
    }
}

#[test]
fn shop_door_empty_leftover_says_nothing_for_sale() {
    let conn = mem();
    set_name(&conn);
    let dir = temp_dir("empty");
    let events = count_table(&conn, "event_log");
    let (written, html) = page(&conn, &dir);
    assert_eq!(
        written.file_path,
        shop::shop_page_path(&dir).to_str().unwrap(),
        "the cart page's path, reused"
    );
    assert!(written.harvest_dates.is_empty());
    assert_eq!(written.size_bytes as usize, html.len());
    assert_eq!(SHOP_DOOR_EMPTY_LINE, "Nothing for sale right now.");
    assert_eq!(html.matches(SHOP_DOOR_EMPTY_LINE).count(), 1, "{html}");
    assert_eq!(html.matches("<h1>Groundtruth Farm</h1>").count(), 1);
    assert_eq!(html.matches("<title>Groundtruth Farm</title>").count(), 1);
    assert_eq!(
        SHOP_DOOR_AVAILABILITY_TAIL,
        "Not a guarantee — if we sell out we'll refund you or offer a substitute."
    );
    assert_eq!(html.matches("Availability shown as of ").count(), 1);
    assert_eq!(html.matches(SHOP_DOOR_AVAILABILITY_TAIL).count(), 1);
    assert!(!html.contains("Pay online"));
    assert!(!html.contains("Prairie Roots"));
    assert!(!html.contains("<script"));
    assert_eq!(
        count_table(&conn, "event_log"),
        events,
        "a viewer writes no event"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn shop_door_missing_farm_name_refuses_with_the_inv_a_sentence_and_writes_no_file() {
    let mut conn = mem();
    minted(&mut conn, "kale", 700);
    let dir = temp_dir("noname");
    // NULL (a fresh farm), then an empty save, then a blank save.
    let err = shop::write_leftover_shop_page(&conn, &dir).unwrap_err();
    assert_eq!(err, INVOICE_NEEDS_FARM_NAME_LINE);
    for name in ["", "   "] {
        invoice::set_farm_display_name(&conn, name).unwrap();
        let err = shop::write_leftover_shop_page(&conn, &dir).unwrap_err();
        assert_eq!(err, INVOICE_NEEDS_FARM_NAME_LINE);
    }
    assert!(!shop::shop_page_path(&dir).exists(), "no file on a refusal");
    assert!(!shop::shop_dir(&dir).exists(), "no folder on a refusal");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn shop_door_one_unpaid_minted_lot_prints_the_money_row_and_the_stored_url() {
    let mut conn = mem();
    set_name(&conn);
    let lot = minted(&mut conn, "kale", 700);
    let url = lot.payment_link_url.clone().unwrap();
    assert!(
        url.ends_with(&format!("?client_reference_id=lo-{}", lot.listing_id)),
        "{url}"
    );
    let unminted = listed(&mut conn, "broccoli");
    assert!(unminted.payment_link_url.is_none());
    let events = count_table(&conn, "event_log");
    let dir = temp_dir("one");
    let (written, html) = page(&conn, &dir);
    assert_eq!(written.harvest_dates, vec![today()]);
    // The Money row's words: crop, harvested day, listed ounces, price.
    let row = format!("Kale · harvested {} · 2.0 oz · $7.00", today());
    assert_eq!(html.matches(&row).count(), 1, "{html}");
    // The stored URL, byte for byte, as the link and as its text.
    let pay = format!(r#"Pay online: <a href="{url}">{url}</a>"#);
    assert_eq!(html.matches(&pay).count(), 1, "{html}");
    assert_eq!(html.matches("Pay online").count(), 1);
    assert_eq!(html.matches(r#"<li class="lot">"#).count(), 1);
    assert!(!html.contains(SHOP_DOOR_EMPTY_LINE));
    // The unminted lot has no dollar and no link — it is not for sale.
    assert!(!html.contains("Broccoli"), "{html}");
    assert!(!html.contains(&unminted.listing_id));
    // Bytes law: no brand string, no script, no key, under the budget.
    assert!(!html.contains("Prairie Roots"));
    assert!(!html.contains("<script"));
    assert!(!html.to_ascii_lowercase().contains("rk_"));
    assert!(!html.to_ascii_lowercase().contains("sk_"));
    assert!(written.size_bytes < 100 * 1024, "{}", written.size_bytes);
    // The read wrote nothing: no event, and the listing is as Money left it.
    assert_eq!(count_table(&conn, "event_log"), events);
    let again = leftover::get_listing(&conn, &lot.listing_id).unwrap();
    assert_eq!(again.payment_link_url.as_deref(), Some(url.as_str()));
    assert_eq!(again.payment_link_id, lot.payment_link_id);
    assert_eq!(again.priced_total_cents, Some(700));
    assert!(again.paid_at.is_none());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn shop_door_two_lots_print_one_row_each_in_the_readers_order() {
    let mut conn = mem();
    set_name(&conn);
    let kale = minted(&mut conn, "kale", 700);
    let broccoli = minted(&mut conn, "broccoli", 500);
    let dir = temp_dir("two");
    let (written, html) = page(&conn, &dir);
    assert_eq!(written.harvest_dates, vec![today()], "one day, listed once");
    assert_eq!(html.matches(r#"<li class="lot">"#).count(), 2);
    assert_eq!(html.matches("Pay online").count(), 2);
    let b = html.find("Broccoli · harvested").unwrap();
    let k = html.find("Kale · harvested").unwrap();
    assert!(b < k, "the reader's crop order (sort_order), unchanged");
    assert_eq!(
        html.matches(broccoli.payment_link_url.as_deref().unwrap())
            .count(),
        2
    );
    assert_eq!(
        html.matches(kale.payment_link_url.as_deref().unwrap())
            .count(),
        2
    );
    assert!(html.contains("· 2.0 oz · $5.00"));
    assert!(html.contains("· 2.0 oz · $7.00"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn shop_door_a_paid_lot_drops_off_the_page() {
    let mut conn = mem();
    set_name(&conn);
    let lot = minted(&mut conn, "kale", 700);
    let url = lot.payment_link_url.clone().unwrap();
    let dir = temp_dir("paid");
    let (_, before) = page(&conn, &dir);
    assert!(before.contains(&url));
    leftover::pay_listing_cash(&mut conn, &lot.listing_id, 700, &today(), None).unwrap();
    // The same path, rewritten — the page reads the book as it is now.
    let (written, after) = page(&conn, &dir);
    assert!(!after.contains(&url), "{after}");
    assert!(!after.contains("Kale"));
    assert_eq!(after.matches(SHOP_DOOR_EMPTY_LINE).count(), 1);
    assert!(written.harvest_dates.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn shop_door_mints_nothing_and_leaves_the_stripe_side_alone() {
    let mut conn = mem();
    set_name(&conn);
    minted(&mut conn, "kale", 700);
    let dir = temp_dir("mint");
    let events = count_table(&conn, "event_log");
    let offers = count_table(&conn, "offers");
    assert_eq!(money::checkout_endpoint_url(&conn).unwrap(), None);
    let _ = page(&conn, &dir);
    let _ = page(&conn, &dir);
    assert_eq!(count_table(&conn, "event_log"), events, "no event");
    assert_eq!(count_table(&conn, "offers"), offers, "no offer");
    assert_eq!(
        money::checkout_endpoint_url(&conn).unwrap(),
        None,
        "no checkout address"
    );
    assert_eq!(
        count_table(&conn, "leftover_listings"),
        1,
        "the book is as Money left it"
    );
    // Structural: the composer takes a plain connection and a folder — no
    // gateway can reach it, and its source names none of the mint doors.
    let src = read("src/shop.rs");
    let marker = src.find("SHOP DOOR Job A").expect("the composer's marker");
    let door = &src[marker..];
    assert_eq!(
        door.matches("pub fn write_leftover_shop_page(conn: &Connection, folder_path: &Path)")
            .count(),
        1
    );
    for banned in [
        "gateway",
        "retire_harvest_links",
        "checkout_endpoint_url",
        "create_order_payment_link",
        "set_offer",
        "shop_listings",
        "insert_event",
        "apply_event",
        "transaction(",
    ] {
        assert_eq!(door.matches(banned).count(), 0, "the viewer names {banned}");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn shop_door_refuses_a_page_that_would_carry_a_key_and_writes_no_file() {
    let mut conn = mem();
    minted(&mut conn, "kale", 700);
    let dir = temp_dir("key");
    for name in ["Farm sk_test_do_not_leak", "RK_TEST_do_not_leak Farm"] {
        invoice::set_farm_display_name(&conn, name).unwrap();
        let err = shop::write_leftover_shop_page(&conn, &dir).unwrap_err();
        assert_eq!(err, shop::SHOP_DOOR_KEY_REFUSAL_LINE);
    }
    assert!(!shop::shop_page_path(&dir).exists(), "no file on a refusal");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn shop_door_desk_wiring_exists_once_and_the_sheet_stays_unmounted() {
    let lib = read("src/lib.rs");
    assert_eq!(
        lib.matches("commands::write_leftover_shop_page,").count(),
        2,
        "registered in both handler lists"
    );
    assert_eq!(lib.matches("commands::open_shop_page_folder,").count(), 2);
    assert!(lib.contains("mod shop_door_tests;"));
    let commands = read("src/commands.rs");
    assert_eq!(
        commands.matches("pub fn write_leftover_shop_page(").count(),
        1
    );
    let api = read("../src/farm/api.ts");
    assert_eq!(
        api.matches(r#"invoke("write_leftover_shop_page")"#).count(),
        1
    );
    assert_eq!(api.matches(r#"invoke("open_shop_page_folder")"#).count(), 1);
    let money_src = read("../src/screens/Money.tsx");
    assert_eq!(
        money_src
            .matches("onClick={() => void onWriteShopPage()}")
            .count(),
        1
    );
    assert_eq!(
        money_src
            .matches("onClick={() => void onOpenShopFolder()}")
            .count(),
        1
    );
    assert_eq!(
        money_src
            .matches("Shop page written: {shopPage.filePath}")
            .count(),
        1
    );
    assert!(money_src.contains("Test mode (GT-D13): only a Stripe test card can pay this link."));
    // GT-D23 holds on the desk: the Money door never reaches the cart
    // composer, an offer, the checkout address, or the sheet.
    for banned in [
        "generateShopPage",
        "setOffer",
        "setCheckoutEndpointUrl",
        "SellOnlineSheet",
    ] {
        assert_eq!(money_src.matches(banned).count(), 0, "Money names {banned}");
    }
    // GT-D24: SellOnlineSheet importers stay 0 — its only hits are its own.
    let mut sources = Vec::new();
    ts_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../src"),
        &mut sources,
    );
    assert!(sources.len() > 10, "walked the frontend tree");
    let importers: Vec<&String> = sources
        .iter()
        .filter(|(path, src)| {
            !path.ends_with("SellOnlineSheet.tsx") && src.contains("SellOnlineSheet")
        })
        .map(|(path, _)| path)
        .collect();
    assert!(
        importers.is_empty(),
        "SellOnlineSheet importers: {importers:?}"
    );
}
