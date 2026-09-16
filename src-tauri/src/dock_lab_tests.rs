//! LOOK A, the lab dock. Not a product test. Serves the real shell over the
//! real door on a seeded in-memory farm so the phone can be photographed
//! without opening the live farm. See docs/architecture/DESIGN-DOCK.md.

use crate::db;
use crate::dock_port;
use crate::field_devices;
use crate::marketing;
use crate::scans;
use crate::wholesale::{self, OrderLine};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const LAB_BIND: &str = "0.0.0.0:18766";

fn add_days(date: &str, n: i64) -> String {
    let d = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
    (d + chrono::Duration::days(n))
        .format("%Y-%m-%d")
        .to_string()
}

fn standing_shortfall(conn: &mut Connection) {
    let venue =
        marketing::record_venue(conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap();
    let mut targets = BTreeMap::new();
    targets.insert("Dun peas".into(), 3);
    marketing::change_stage(
        conn,
        &venue.venue_id,
        "standing",
        &db::local_date_today(),
        Some(3),
        Some(vec!["Dun peas".into()]),
        Some(targets),
        None,
    )
    .unwrap();
}

fn seed_collect(conn: &mut Connection, delivered_on: &str) -> (String, String) {
    let venue =
        marketing::record_venue(conn, "Collect Cafe", "cafe", None, None, None, None).unwrap();
    let harvest = add_days(&db::local_date_today(), -7);
    let order = wholesale::record_order(
        conn,
        &venue.venue_id,
        &harvest,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(900),
        }],
        true,
    )
    .unwrap();
    wholesale::deliver_order(conn, &order.id, Some(delivered_on.to_string())).unwrap();
    (order.id, venue.venue_id)
}

fn seed_lab() -> (Arc<Mutex<Connection>>, String) {
    let mut conn = db::open_in_memory().unwrap();
    scans::set_config(
        &conn,
        Some("https://scans.example"),
        Some("secret_pull_token_fixture"),
    )
    .unwrap();
    standing_shortfall(&mut conn);
    let today = db::local_date_today();
    let _ = seed_collect(&mut conn, &add_days(&today, -3));
    let pairing = field_devices::pair_admin(&mut conn).unwrap();
    (Arc::new(Mutex::new(conn)), pairing.token)
}

fn hold() -> Duration {
    let secs = std::env::var("GT_DOCK_LAB_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(600)
        .clamp(30, 3600);
    Duration::from_secs(secs)
}

#[test]
#[ignore = "lab dock: serves a seeded farm on 18766 until the hold ends"]
fn dock_lab() {
    let hold = hold();
    let (db, token) = seed_lab();
    let dir = std::env::temp_dir().join("groundtruth-dock-lab");
    let _ = std::fs::create_dir_all(&dir);
    let _ = dock_port::stop();
    let view = dock_port::start(db, dir.clone(), dir, LAB_BIND).unwrap();
    assert!(view.running);
    assert_eq!(view.port, Some(18766));
    match &view.reach_url {
        Some(url) => println!("reach URL: {url}"),
        None => println!("reach URL is not known; open http://<lan>:18766 on the phone"),
    }
    println!("fixture token: {token}");
    println!("phone: open the URL, paste the token into the token field, tap Remember");
    println!("hold: {} minutes", hold.as_secs() / 60);
    println!("this is a fixture farm; nothing here touches the real one");
    std::thread::sleep(hold);
    let _ = dock_port::stop();
    println!("the lab dock is closed");
}
