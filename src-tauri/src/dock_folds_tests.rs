//! Targeted coverage for the three folds. S2a signed 2026-08-21.

use crate::attention;
use crate::db;
use crate::dock_folds::{
    capture_endpoint, card_for_check, cards_from_rows, check_body, check_title, clash_cards,
    clashes_read_only, ids_for_card, phone_queue_facts, scope_for_check, severity_for_ids,
    today_attention_order, worst_clash, worst_clash_read_only, worst_of, STANDING_SHORTFALL_RANK,
};
use crate::health::{CheckStatus, Severity, REPORTED_CHECKS};
use crate::marketing;
use crate::scans;
use crate::trays;
use crate::wholesale::{self, OrderLine};
use rusqlite::Connection;
use std::collections::{BTreeMap, HashSet};

fn row(id: &str, severity: Severity, sentence: &str) -> CheckStatus {
    CheckStatus {
        check_id: id.into(),
        severity,
        sentence: sentence.into(),
        ran_at: None,
    }
}

/// FI-4b - any instant later than the test rows' `ran_at` (all None here), so
/// the age rule is exercised without a clock in the test.
const SERVED: &str = "2026-08-23T20:04:00+00:00";

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

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

#[test]
fn worst_of_unhealthy_beats_degraded_beats_healthy_empty_is_none() {
    assert_eq!(worst_of(&[]), None);
    let healthy = row("M1", Severity::Healthy, "M1 ok");
    let degraded = row("M2", Severity::Degraded, "M2 warn");
    let unhealthy = row("M3", Severity::Unhealthy, "M3 bad");
    assert_eq!(
        worst_of(std::slice::from_ref(&healthy)),
        Some(Severity::Healthy)
    );
    assert_eq!(
        worst_of(&[healthy.clone(), degraded.clone()]),
        Some(Severity::Degraded)
    );
    assert_eq!(
        worst_of(&[healthy, degraded, unhealthy]),
        Some(Severity::Unhealthy)
    );
}

/// One nine-row fixture, so the money and promise tests cannot drift apart.
fn nine_rows() -> Vec<CheckStatus> {
    vec![
        row(
            "M1",
            Severity::Unhealthy,
            "M1 Owed to you — two deliveries.",
        ),
        row("M2", Severity::Degraded, "M2 Cover — short."),
        row("M3", Severity::Healthy, "M3 Delivery — none late."),
        row("F1", Severity::Healthy, "F1 Harvest — none overdue."),
        row("F2", Severity::Healthy, "F2 Light — none overdue."),
        row("H2", Severity::Degraded, "H2 Flush — lagging."),
        row("H3", Severity::Healthy, "H3 Snapshot — taken."),
        row("H4", Severity::Healthy, "H4 Replay — clean."),
        row("M4", Severity::Healthy, "M4 Trail — complete."),
    ]
}

#[test]
fn severity_for_ids_m1_equals_money_card() {
    let rows = nine_rows();
    let cards = cards_from_rows(&rows, SERVED);
    let money = cards
        .iter()
        .find(|c| c.card == "money")
        .expect("money card");
    assert_eq!(severity_for_ids(&rows, &["M1"]), money.severity);
    assert_eq!(money.severity, Some(Severity::Unhealthy));
    assert_eq!(money.check_ids, vec!["M1"]);
}

#[test]
fn severity_for_ids_m3_equals_promise_card() {
    let rows = nine_rows();
    let cards = cards_from_rows(&rows, SERVED);
    let promise = cards
        .iter()
        .find(|c| c.card == "promise")
        .expect("promise card");
    assert_eq!(severity_for_ids(&rows, &["M3"]), promise.severity);
    assert_eq!(promise.severity, Some(Severity::Healthy));
    assert_eq!(promise.check_ids, vec!["M3"]);
}

#[test]
fn reported_checks_each_have_one_scope_and_one_card() {
    let mut seen_ids: HashSet<&str> = HashSet::new();
    let mut union: HashSet<String> = HashSet::new();
    for id in REPORTED_CHECKS {
        let scope = scope_for_check(id);
        assert!(scope == "farm" || scope == "system", "{id} scope={scope}");
        let card = card_for_check(id);
        assert_ne!(card, "phone_queue", "{id} must not map to phone_queue");
        assert!(
            ["money", "cover", "promise", "rack", "system"].contains(&card),
            "{id} card={card}"
        );
        seen_ids.insert(id);
    }
    assert_eq!(seen_ids.len(), 9);
    for card in ["money", "cover", "promise", "rack", "phone_queue", "system"] {
        for id in ids_for_card(card) {
            assert_ne!(card, "phone_queue");
            union.insert(id);
        }
    }
    let reported: HashSet<String> = REPORTED_CHECKS.iter().map(|s| (*s).to_string()).collect();
    assert!(ids_for_card("phone_queue").is_empty());
    assert_eq!(union, reported);
}

#[test]
fn check_title_strips_prefix_and_cuts_at_dash() {
    assert_eq!(
        check_title("M1", "M1 Owed to you — two deliveries still open."),
        "Owed to you"
    );
    assert_eq!(check_title("M1", "M1 no dash in this sentence"), "M1");
    assert_eq!(check_title("M1", "something else entirely"), "M1");
    assert_eq!(check_title("M1", " — leading dash is not a title"), "M1");
}

/// FI-4b - the complement of `check_title`, and total. B-1(b): a row that does
/// not carry "{id} {title} — {body}" yields no face sentence rather than a
/// headless clause or a raw check id.
///
/// The off-pattern strings below are grammar cases, not live text. FI-4d put
/// every reported check on the pattern, and f4f holds that line. These stay
/// because what is under test is the function's contract, not health.rs output.
#[test]
fn f4e_check_body_is_the_complement_of_check_title() {
    assert_eq!(
        check_body("M1", "M1 Owed to you — two deliveries still open."),
        Some("two deliveries still open.".to_string())
    );
    assert_eq!(
        check_title("M1", "M1 Owed to you — two deliveries still open."),
        "Owed to you"
    );
    assert_eq!(
        check_body("H2", "H2 evidence is timestamped in the future"),
        None
    );
    assert_eq!(
        check_body("H2", "Check H2 hasn't reported since never"),
        None
    );
    assert_eq!(
        check_body("H2", "Marketing log silent 9 days with 3 active venues."),
        None
    );
    assert_eq!(check_body("M1", "M1  — headless"), None);
    assert_eq!(check_body("M1", "M1 Owed to you — "), None);
    assert_eq!(check_body("M1", "something else entirely"), None);
}

/// FI-4d - the seven system-scope arms that used to fall back to a bare title
/// and a severity. Each now carries its own check's title and a body
/// `check_body` can lift. Zero new words entered: every clause below is the one
/// the arm already had, re-ordered onto the grammar FI-4b signed.
#[test]
fn f4f_every_reshaped_system_arm_yields_a_title_and_a_body() {
    for (id, title, sentence, body) in [
        (
            "H2",
            "Capture paths",
            "H2 Capture paths — evidence is timestamped in the future.",
            "evidence is timestamped in the future.",
        ),
        (
            "H2",
            "Capture paths",
            "H2 Capture paths — hasn't reported since never.",
            "hasn't reported since never.",
        ),
        (
            "H2",
            "Capture paths",
            "H2 Capture paths — marketing log silent 9 days with 3 active venues.",
            "marketing log silent 9 days with 3 active venues.",
        ),
        (
            "H3",
            "Critical jobs",
            "H3 Critical jobs — evidence is timestamped in the future.",
            "evidence is timestamped in the future.",
        ),
        (
            "H3",
            "Critical jobs",
            "H3 Critical jobs — hasn't reported since never.",
            "hasn't reported since never.",
        ),
        (
            "H4",
            "Core data intact",
            "H4 Core data intact — evidence is timestamped in the future.",
            "evidence is timestamped in the future.",
        ),
        (
            "H4",
            "Core data intact",
            "H4 Core data intact — hasn't reported since never.",
            "hasn't reported since never.",
        ),
    ] {
        assert_eq!(check_title(id, sentence), title, "title for: {sentence}");
        assert_eq!(
            check_body(id, sentence),
            Some(body.to_string()),
            "body for: {sentence}"
        );
        assert!(
            !body.contains(id),
            "a body must never carry a raw check id: {body}"
        );
    }
    // Source scan in the f1c / h2 style: the strings above are the ones health.rs
    // actually emits, not a copy of them that can drift.
    let health = include_str!("health.rs");
    for arm in [
        "\"H2 Capture paths — evidence is timestamped in the future.\"",
        "\"H2 Capture paths — hasn't reported since never.\"",
        "\"H2 Capture paths — marketing log silent {days_shown} days",
        "\"H3 Critical jobs — evidence is timestamped in the future.\"",
        "\"H3 Critical jobs — hasn't reported since never.\"",
        "\"H4 Core data intact — evidence is timestamped in the future.\"",
        "\"H4 Core data intact — hasn't reported since never.\"",
    ] {
        assert!(health.contains(arm), "health.rs no longer emits {arm}");
    }
    // D-2b - H1 is not a reported check and was signed out of scope. If this
    // fires, the fence reached past the seven arms it was allowed to touch.
    assert!(
        health.contains("\"H1 evidence is timestamped in the future\""),
        "H1 was carried forward, not reshaped"
    );
    assert!(
        health.contains("\"Check H1 hasn't reported since never\""),
        "H1 was carried forward, not reshaped"
    );
}

#[test]
fn today_attention_order_debt_age_then_cover() {
    let mut conn = mem();
    let today = db::local_date_today();
    let older = add_days(&today, -5);
    let newer = add_days(&today, -1);
    let cover_later = add_days(&today, 21);
    let cover_earlier = add_days(&today, 14);

    let past_harvest = add_days(&today, -7);
    let venue_n =
        marketing::record_venue(&mut conn, "Newer Cafe", "cafe", None, None, None, None).unwrap();
    let newer_order = wholesale::record_order(
        &mut conn,
        &venue_n.venue_id,
        &past_harvest,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(900),
        }],
        true,
    )
    .unwrap();
    wholesale::deliver_order(&mut conn, &newer_order.id, Some(newer.clone())).unwrap();

    let venue_o =
        marketing::record_venue(&mut conn, "Older Cafe", "cafe", None, None, None, None).unwrap();
    let older_order = wholesale::record_order(
        &mut conn,
        &venue_o.venue_id,
        &past_harvest,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(900),
        }],
        true,
    )
    .unwrap();
    wholesale::deliver_order(&mut conn, &older_order.id, Some(older.clone())).unwrap();

    let cover_venue =
        marketing::record_venue(&mut conn, "Cover Cafe", "cafe", None, None, None, None).unwrap();
    wholesale::record_order(
        &mut conn,
        &cover_venue.venue_id,
        &cover_later,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 5,
            price_cents_per_tray: Some(900),
        }],
        true,
    )
    .unwrap();
    wholesale::record_order(
        &mut conn,
        &cover_venue.venue_id,
        &cover_earlier,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 5,
            price_cents_per_tray: Some(900),
        }],
        true,
    )
    .unwrap();

    let now = db::utc_now_rfc3339();
    conn.execute(
        "INSERT INTO attention
         (id, kind, entity_type, entity_id, message, actions, created_at, resolved_at, resolved_by)
         VALUES ('unrec-later', 'order.unrecorded', 'harvest_date', ?1,
                 'later cover date, inserted first', '[]', ?2, NULL, NULL)",
        rusqlite::params![&cover_later, &now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO attention
         (id, kind, entity_type, entity_id, message, actions, created_at, resolved_at, resolved_by)
         VALUES ('unrec-earlier', 'order.unrecorded', 'harvest_date', ?1,
                 'earlier cover date, inserted second', '[]', ?2, NULL, NULL)",
        rusqlite::params![&cover_earlier, &now],
    )
    .unwrap();

    let ids = today_attention_order(&conn).unwrap();
    let items = attention::check_attention(&conn).unwrap();
    let by_id: BTreeMap<String, &crate::models::AttentionItem> =
        items.iter().map(|a| (a.id.clone(), a)).collect();
    let ordered: Vec<&crate::models::AttentionItem> = ids
        .iter()
        .map(|id| *by_id.get(id).expect("id from today_attention_order"))
        .collect();

    let collect: Vec<&str> = ordered
        .iter()
        .filter(|a| a.kind == "money.delivered_unpaid")
        .filter_map(|a| a.entity_id.as_deref())
        .collect();
    assert_eq!(
        collect,
        vec![older_order.id.as_str(), newer_order.id.as_str()],
        "same-rank money rows order by delivered_on"
    );

    let cover_ids: Vec<String> = ordered
        .iter()
        .filter(|a| a.kind == "money.capacity_short")
        .filter_map(|a| a.entity_id.clone())
        .collect();
    assert_eq!(
        cover_ids,
        vec![
            format!("{cover_earlier}|dun-peas"),
            format!("{cover_later}|dun-peas")
        ],
        "cover-plan order, not insert order"
    );

    let unrec: Vec<&str> = ordered
        .iter()
        .filter(|a| a.kind == "order.unrecorded")
        .map(|a| a.id.as_str())
        .collect();
    assert_eq!(
        unrec,
        vec!["unrec-earlier", "unrec-later"],
        "unreadable age falls through to cover order instead of guessing"
    );
}

#[test]
fn worst_clash_attention_at_rank_0_beats_standing_shortfall() {
    let mut conn = mem();
    standing_shortfall(&mut conn);
    let today = db::local_date_today();
    let (order_id, _) = seed_collect(&mut conn, &add_days(&today, -3));
    let clash = worst_clash(&conn).unwrap().expect("a clash");
    assert_eq!(clash.source, "attention");
    assert_eq!(clash.kind, "money.delivered_unpaid");
    assert_eq!(clash.entity_id.as_deref(), Some(order_id.as_str()));
    assert_eq!(clash.rank, 0);
    let stored = clash.sentence.expect("attention carries its own message");
    assert!(!stored.is_empty());
    assert!(
        STANDING_SHORTFALL_RANK > clash.rank,
        "rank 0 beats standing shortfall at 3"
    );
}

#[test]
fn worst_clash_shortfall_wins_with_no_today_attention() {
    let mut conn = mem();
    standing_shortfall(&mut conn);
    let clash = worst_clash(&conn).unwrap().expect("a clash");
    assert_eq!(clash.source, "standing_shortfall");
    assert_eq!(clash.rank, STANDING_SHORTFALL_RANK);
    let s = clash
        .sentence
        .expect("FI-5 - standing shortfall carries a sentence");
    assert!(s.starts_with("Standing orders are short "), "{s}");
    assert!(s.ends_with(" this week."), "{s}");
    assert_eq!(
        clash.cards,
        Some(vec!["cover".to_string(), "promise".to_string()])
    );
}

#[test]
fn worst_clash_move_due_beats_harvest_due_when_neither_attention_nor_shortfall() {
    let mut conn = mem();
    let mtl = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    trays::test_shift_dates(&mut conn, &mtl.id, -4).unwrap();
    let harv = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    trays::advance_trays(&mut conn, std::slice::from_ref(&harv.id)).unwrap();
    trays::test_shift_dates(&mut conn, &harv.id, -10).unwrap();

    let view = trays::today_view(&conn).unwrap();
    assert!(view.move_to_light.is_some());
    assert!(view.harvest_summary.is_some());

    let clash = worst_clash(&conn).unwrap().expect("a clash");
    assert_eq!(clash.source, "move_due");
    assert_eq!(clash.rank, 8);
    let s = clash.sentence.expect("FI-5 - move due carries a sentence");
    assert!(s.ends_with(" are due to move to light."), "{s}");
    assert_eq!(clash.cards, None, "work due on the rack draws no line");
}

#[test]
fn f1_worst_clash_read_only_agrees_when_already_evaluated() {
    let mut conn = mem();
    standing_shortfall(&mut conn);
    let today = db::local_date_today();
    let _ = seed_collect(&mut conn, &add_days(&today, -3));
    let _ = attention::check_attention(&conn).unwrap();
    let evaluating = worst_clash(&conn).unwrap();
    let read_only = worst_clash_read_only(&conn).unwrap();
    assert_eq!(evaluating, read_only);
}

#[test]
fn f2_worst_clash_read_only_does_not_raise_rows() {
    let mut conn = mem();
    let today = db::local_date_today();
    let _ = seed_collect(&mut conn, &add_days(&today, -3));
    let before: i64 = conn
        .query_row("SELECT COUNT(*) FROM attention", [], |r| r.get(0))
        .unwrap();
    let _ = worst_clash_read_only(&conn).unwrap();
    let after: i64 = conn
        .query_row("SELECT COUNT(*) FROM attention", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, before);
}

/// FI-3 - the six nodes, in the signed order. Asserted through the observable
/// document rather than the private constant.
#[test]
fn f3a_cards_are_the_signed_six_in_order() {
    let cards = cards_from_rows(&nine_rows(), SERVED);
    let names: Vec<&str> = cards.iter().map(|c| c.card.as_str()).collect();
    assert_eq!(
        names,
        vec!["money", "cover", "promise", "rack", "phone_queue", "system"]
    );
}

/// FI-3 - the two hand-mirrored tables agree. This is the proof that would have
/// caught them drifting: every reported check is listed by the card it claims,
/// and the union over the six cards is exactly the reported set.
#[test]
fn f3b_card_for_check_and_ids_for_card_agree() {
    let mut union: HashSet<String> = HashSet::new();
    for id in REPORTED_CHECKS {
        let card = card_for_check(id);
        assert!(
            ids_for_card(card).iter().any(|x| x == id),
            "{id} maps to {card}, which does not list it"
        );
    }
    for card in ["money", "cover", "promise", "rack", "phone_queue", "system"] {
        for id in ids_for_card(card) {
            assert_eq!(
                card_for_check(&id),
                card,
                "{card} lists {id}, which maps elsewhere"
            );
            union.insert(id);
        }
    }
    let reported: HashSet<String> = REPORTED_CHECKS.iter().map(|s| (*s).to_string()).collect();
    assert_eq!(union, reported);
}

/// FI-3 - no card invents a severity. Every card is worst_of its own ids, and
/// the sourceless card reports nothing rather than something.
#[test]
fn f3c_no_card_carries_an_invented_severity() {
    let rows = nine_rows();
    let cards = cards_from_rows(&rows, SERVED);
    for c in &cards {
        let ids: Vec<&str> = c.check_ids.iter().map(String::as_str).collect();
        assert_eq!(
            c.severity,
            severity_for_ids(&rows, &ids),
            "{} severity is not worst_of its own ids",
            c.card
        );
    }
    let queue = cards
        .iter()
        .find(|c| c.card == "phone_queue")
        .expect("phone_queue card");
    assert!(queue.check_ids.is_empty(), "still sourceless");
    assert_eq!(queue.severity, None, "S5 - no invented severity");
}

/// FI-3 - the split is real: three distinct cards, one check each, and M3 is no
/// longer filed under money.
#[test]
fn f3d_money_cover_and_promise_are_distinct_single_check_cards() {
    let cards = cards_from_rows(&nine_rows(), SERVED);
    let find = |name: &str| {
        cards
            .iter()
            .find(|c| c.card == name)
            .unwrap_or_else(|| panic!("{name} card"))
            .check_ids
            .clone()
    };
    assert_eq!(find("money"), vec!["M1"]);
    assert_eq!(find("cover"), vec!["M2"]);
    assert_eq!(find("promise"), vec!["M3"]);
    assert_eq!(card_for_check("M3"), "promise");
    assert_ne!(card_for_check("M3"), "money");
}

/// FI-5 - the signed edge table, row by row, including the closed-list rule.
#[test]
fn f5a_clash_cards_is_the_signed_table() {
    assert_eq!(clash_cards("attention", "money.delivered_unpaid"), None);
    assert_eq!(
        clash_cards("attention", "money.delivery_due"),
        Some(("promise", "money"))
    );
    assert_eq!(
        clash_cards("attention", "money.capacity_short"),
        Some(("cover", "promise"))
    );
    for k in ["order.unrecorded", "order.refunded", "order.disputed"] {
        assert_eq!(
            clash_cards("attention", k),
            Some(("money", "system")),
            "{k}"
        );
    }
    assert_eq!(
        clash_cards("attention", "order.oversold"),
        Some(("promise", "cover"))
    );
    for k in ["stripe.account_mismatch", "stripe.unrecognised_session"] {
        assert_eq!(
            clash_cards("attention", k),
            Some(("money", "system")),
            "{k}"
        );
    }
    assert_eq!(
        clash_cards("attention", "wholesale.overcommitted"),
        Some(("promise", "cover"))
    );
    assert_eq!(
        clash_cards("attention", "tray.overdue_harvest"),
        Some(("rack", "promise"))
    );
    assert_eq!(
        clash_cards("attention", "tray.overdue_light"),
        Some(("rack", "cover"))
    );
    assert_eq!(
        clash_cards("standing_shortfall", ""),
        Some(("cover", "promise"))
    );
    assert_eq!(clash_cards("move_due", ""), None);
    assert_eq!(clash_cards("harvest_due", ""), None);
    // Closed list: never guess.
    assert_eq!(clash_cards("attention", "some.future.kind"), None);
    assert_eq!(clash_cards("no_such_source", "money.delivery_due"), None);
}

/// FI-5 - no clash reaches the wire without a sentence. This is the test the
/// bare-token defect would have failed.
#[test]
fn f5b_every_clash_carries_a_sentence() {
    let mut conn = mem();
    standing_shortfall(&mut conn);
    let today = db::local_date_today();
    let _ = seed_collect(&mut conn, &add_days(&today, -3));
    let _ = attention::check_attention(&conn).unwrap();
    let clashes = clashes_read_only(&conn).unwrap();
    assert!(!clashes.is_empty(), "the fixture must produce clashes");
    for c in &clashes {
        let s = c.sentence.as_deref().unwrap_or("");
        assert!(!s.is_empty(), "{} has no sentence", c.source);
    }
}

/// FI-5 - an edge is either absent or an ordered pair of two distinct known
/// cards. Never a self-loop, never an unknown node.
#[test]
fn f5c_edges_are_ordered_pairs_of_distinct_known_cards() {
    let mut conn = mem();
    standing_shortfall(&mut conn);
    let today = db::local_date_today();
    let _ = seed_collect(&mut conn, &add_days(&today, -3));
    let _ = attention::check_attention(&conn).unwrap();
    let known = ["money", "cover", "promise", "rack", "phone_queue", "system"];
    for c in clashes_read_only(&conn).unwrap() {
        if let Some(pair) = c.cards {
            assert_eq!(pair.len(), 2, "{} edge is not a pair", c.source);
            assert_ne!(pair[0], pair[1], "{} edge is a self-loop", c.source);
            for node in &pair {
                assert!(known.contains(&node.as_str()), "unknown node {node}");
            }
        }
    }
}

/// FI-5 - the clash set is read-only, like its worst-of sibling. Companion to
/// f2: the port must never evaluate to build a line.
#[test]
fn f5f_clashes_read_only_does_not_raise_rows() {
    let mut conn = mem();
    let today = db::local_date_today();
    let _ = seed_collect(&mut conn, &add_days(&today, -3));
    let before: i64 = conn
        .query_row("SELECT COUNT(*) FROM attention", [], |r| r.get(0))
        .unwrap();
    let _ = clashes_read_only(&conn).unwrap();
    let after: i64 = conn
        .query_row("SELECT COUNT(*) FROM attention", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, before);
}

/// FI-7 - the fold is a read, and its count is its rows.
#[test]
fn f7f_phone_queue_facts_is_a_read_and_counts_its_own_rows() {
    let conn = mem();
    let before: i64 = conn
        .query_row("SELECT COUNT(*) FROM phone_proposals", [], |r| r.get(0))
        .unwrap();
    let q = phone_queue_facts(&conn).unwrap();
    let after: i64 = conn
        .query_row("SELECT COUNT(*) FROM phone_proposals", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, before, "reading the queue must not write to it");
    assert_eq!(q.pending_count as usize, q.rows.len());
    assert_eq!(q.pending_count, 0, "empty fixture");
    assert_eq!(q.sentence, "No captures are waiting on the PC.");
}

/// FI-9 - the empty-farm block, line by line, reusing the signed sentences.
#[test]
fn f9e_diagnosis_block_on_an_empty_farm_reuses_the_signed_sentences() {
    let conn = mem();
    // db::local_date_today_naive does not exist. Same clock pair dock_port.rs
    // uses: one utc instant, then local_date_of so the two cannot straddle midnight.
    let now = db::utc_now_rfc3339();
    let today = crate::dock_folds::local_date_of(&now).expect("today from now");
    let doc = crate::dock_folds::port_document(
        &conn,
        std::path::Path::new("."),
        std::path::Path::new("."),
        &now,
        today,
    );
    // If the fixture cannot build a document, STOP and report - do not weaken.
    let doc = doc.expect("port_document on an empty farm");
    let d = doc.diagnosis;
    assert!(d.contains("Worst now: nothing open."));
    assert!(d.contains("Today's queue is clear."));
    assert!(d.contains("No captures are waiting on the PC."));
    assert!(d.contains("AUTHORITY: PC sole writer. Snapshot only. Do not invent numbers."));
}

/// FI-10 - unconfigured means nothing, not an empty string, and never a path.
#[test]
fn f10e_capture_endpoint_is_none_when_unconfigured_and_is_never_a_path() {
    let conn = mem();
    assert_eq!(capture_endpoint(&conn).unwrap(), None);
    scans::set_config(&conn, Some("https://scans.example/"), Some("fixture_token")).unwrap();
    let e = capture_endpoint(&conn).unwrap().expect("configured");
    assert_eq!(e, "https://scans.example", "trailing slash trimmed");
    assert!(!e.contains("/a/"));
    assert!(
        !e.contains("fixture_token"),
        "the pull token is not the endpoint"
    );
}

/// C1 (INT-001, D2). A failed standing-demand read propagates instead of
/// vanishing: the clash set and the port document both fail, and neither can
/// say "Today's queue is clear." The phone then shows its stale banner.
#[test]
fn int001_failed_standing_demand_read_fails_the_document_instead_of_a_clear_queue() {
    let conn = mem();
    conn.execute_batch("ALTER TABLE mkt_stages RENAME TO mkt_stages_off;")
        .unwrap();
    let err = clashes_read_only(&conn).unwrap_err();
    assert!(err.contains("mkt_stages"), "{err}");
    assert!(worst_clash_read_only(&conn).is_err());
    let now = db::utc_now_rfc3339();
    let today = crate::dock_folds::local_date_of(&now).expect("today from now");
    let err = crate::dock_folds::port_document(
        &conn,
        std::path::Path::new("."),
        std::path::Path::new("."),
        &now,
        today,
    )
    .unwrap_err();
    assert!(err.contains("mkt_stages"), "{err}");
    assert!(!err.contains("Today's queue is clear."));
}

/// C-1 (SOP-1, 2026-08-30) - the signed Today queue order, as integers.
/// Six beats and no other order: Collect, unmatched payment, a short the
/// desk cannot sow, standing short to sow, cover/harvest, Deliver.
#[test]
fn c1_today_rank_holds_the_signed_queue_order() {
    use crate::dock_folds::today_rank;
    // beat 1 - Collect
    assert_eq!(today_rank("money.delivered_unpaid"), 0);
    // beat 2 - unmatched payment, directly under Collect
    for kind in [
        "order.unrecorded",
        "order.refunded",
        "order.disputed",
        "order.oversold",
        "stripe.account_mismatch",
        "stripe.unrecognised_session",
        "wholesale.overcommitted",
    ] {
        assert_eq!(today_rank(kind), 1, "{kind}");
    }
    // beat 3 - a short the desk cannot sow
    assert_eq!(today_rank("money.capacity_short"), 2);
    // beat 4 - standing short to sow
    assert_eq!(STANDING_SHORTFALL_RANK, 3);
    // beat 5 - cover / harvest
    assert_eq!(today_rank("tray.overdue_harvest"), 4);
    // beat 6 - Deliver
    assert_eq!(today_rank("money.delivery_due"), 5);
    // not a beat; unchanged, still below the six
    assert_eq!(today_rank("tray.overdue_light"), 6);
}

/// C-1 - the law the integers serve: Deliver does not stay above a short the
/// desk cannot sow, and the unmatched payment sits directly under Collect.
#[test]
fn c1_deliver_sits_below_both_shorts() {
    use crate::dock_folds::today_rank;
    let deliver = today_rank("money.delivery_due");
    assert!(deliver > today_rank("money.capacity_short"));
    assert!(deliver > STANDING_SHORTFALL_RANK);
    let collect = today_rank("money.delivered_unpaid");
    assert_eq!(today_rank("order.unrecorded"), collect + 1);
}
