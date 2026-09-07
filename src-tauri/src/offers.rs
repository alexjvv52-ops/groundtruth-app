//! Offers: Stripe Prices per crop. Cart checkout uses Price IDs on the shop page.
//!
//! Attribution of paid line items uses local `offers.stripe_price_id` — never
//! Stripe metadata. Metadata is still written for dashboard readability.

use crate::db;
use crate::models::OfferView;
use crate::money::{self, Offer, StripeGateway};
use crate::trays;
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

/// Crops with trays on `harvest_date`, left-joined to any offer row.
pub fn list_offers(conn: &Connection, harvest_date: &str) -> Result<Vec<OfferView>, String> {
    let mut stmt = conn
        .prepare(
            r#"
            WITH avail AS (
              SELECT t.crop_id AS crop_id,
                     SUM(t.quantity) AS available
              FROM trays t
              WHERE t.state NOT IN ('discarded', 'harvested')
                AND t.sown_on IS NOT NULL
                AND t.growth_days_at_sow IS NOT NULL
                AND date(t.sown_on, '+' || t.growth_days_at_sow || ' days') = ?1
              GROUP BY t.crop_id
            ),
            sold AS (
              SELECT crop_id, SUM(capacity_consumed) AS sold
              FROM orders
              WHERE harvest_date = ?1
              GROUP BY crop_id
            ),
            -- Same rule as trays::capacity_by_harvest_date, per (date, crop):
            -- only a non-voided promise reserves trays (D2(b)). These are
            -- still two queries — unifying them is B5 — but they must not
            -- disagree about what a delivered order means.
            wsold AS (
              SELECT l.crop_id, SUM(l.trays) AS w_sold
              FROM wholesale_orders o
              JOIN wholesale_order_lines l ON l.order_id = o.id
              WHERE o.harvest_date = ?1 AND o.state <> 'voided'
              GROUP BY l.crop_id
            )
            SELECT a.crop_id,
                   c.name,
                   a.available,
                   COALESCE(s.sold, 0) + COALESCE(w.w_sold, 0) AS sold,
                   o.id,
                   o.price_cents,
                   o.stripe_price_id
            FROM avail a
            JOIN crops c ON c.id = a.crop_id
            LEFT JOIN sold s ON s.crop_id = a.crop_id
            LEFT JOIN wsold w ON w.crop_id = a.crop_id
            LEFT JOIN offers o ON o.harvest_date = ?1 AND o.crop_id = a.crop_id
            ORDER BY c.sort_order ASC
            "#,
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([harvest_date], |row| {
            let available: i64 = row.get(2)?;
            let sold: i64 = row.get(3)?;
            Ok(OfferView {
                id: row.get(4)?,
                harvest_date: harvest_date.to_string(),
                crop_id: row.get(0)?,
                crop_name: row.get(1)?,
                price_cents: row.get(5)?,
                stripe_price_id: row.get(6)?,
                stripe_link_url: None,
                available,
                sold,
                remaining: available - sold,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for r in rows {
        let mut offer = r.map_err(|e| e.to_string())?;
        // Sale side, exact date, per (date, crop) — D21a. Same figure
        // trays::capacity_by_harvest_date publishes as remaining_trays.
        offer.remaining = trays::remaining_exact_for(conn, harvest_date, &offer.crop_id)?;
        out.push(offer);
    }
    Ok(out)
}

pub fn set_offer(
    conn: &mut Connection,
    harvest_date: &str,
    crop_id: &str,
    price_cents: i64,
) -> Result<OfferView, String> {
    let gw = money::gateway_from_db(conn)?;
    set_offer_with(conn, &gw, harvest_date, crop_id, price_cents)
}

pub fn set_offer_with<G: StripeGateway>(
    conn: &mut Connection,
    gateway: &G,
    harvest_date: &str,
    crop_id: &str,
    price_cents: i64,
) -> Result<OfferView, String> {
    if price_cents <= 0 {
        return Err("Price must be greater than zero.".to_string());
    }
    let rows = list_offers(conn, harvest_date)?;
    let row = rows
        .iter()
        .find(|r| r.crop_id == crop_id)
        .ok_or_else(|| "That crop has no trays for this harvest date.".to_string())?;

    let existing: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT id, stripe_price_id FROM offers WHERE harvest_date = ?1 AND crop_id = ?2",
            params![harvest_date, crop_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    let offer_id = existing
        .as_ref()
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let now = db::utc_now_rfc3339();
    let offer = Offer {
        id: offer_id.clone(),
        harvest_date: harvest_date.to_string(),
        crop_id: crop_id.to_string(),
        price_cents,
        stripe_price_id: None,
        stripe_link_id: None,
        stripe_link_url: None,
        created_at: now.clone(),
    };

    let price_id = gateway.create_price(&offer)?;

    if existing.is_some() {
        conn.execute(
            "UPDATE offers
             SET price_cents = ?1,
                 stripe_price_id = ?2,
                 stripe_link_id = NULL,
                 stripe_link_url = NULL
             WHERE id = ?3",
            params![price_cents, price_id, offer_id],
        )
        .map_err(|e| e.to_string())?;
    } else {
        conn.execute(
            "INSERT INTO offers
             (id, harvest_date, crop_id, price_cents, stripe_price_id,
              stripe_link_id, stripe_link_url, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6)",
            params![offer_id, harvest_date, crop_id, price_cents, price_id, now],
        )
        .map_err(|e| e.to_string())?;
    }

    // Stale harvest Payment Links must not remain buyable after a price change.
    conn.execute(
        "DELETE FROM harvest_links WHERE harvest_date = ?1",
        [harvest_date],
    )
    .map_err(|e| e.to_string())?;

    // J3 PRICE-GATE: the Price id this row just dropped is archived at Stripe
    // after the row write, best-effort like J1's deactivate_link. Read above,
    // before the overwrite; a Stripe error never rolls the offer back.
    if let Some((_, Some(old_price_id))) = &existing {
        if !old_price_id.is_empty() {
            let _ = gateway.archive_price(old_price_id);
        }
    }

    Ok(OfferView {
        id: Some(offer_id),
        harvest_date: harvest_date.to_string(),
        crop_id: crop_id.to_string(),
        crop_name: row.crop_name.clone(),
        price_cents: Some(price_cents),
        stripe_price_id: Some(price_id),
        stripe_link_url: None,
        available: row.available,
        sold: row.sold,
        remaining: row.remaining,
    })
}

pub fn remove_offer(conn: &mut Connection, offer_id: &str) -> Result<(), String> {
    let gw = money::gateway_from_db(conn)?;
    remove_offer_with(conn, &gw, offer_id)
}

pub fn remove_offer_with<G: StripeGateway>(
    conn: &mut Connection,
    gateway: &G,
    offer_id: &str,
) -> Result<(), String> {
    let held: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT harvest_date, stripe_price_id FROM offers WHERE id = ?1",
            [offer_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE offers
         SET stripe_price_id = NULL,
             stripe_link_id = NULL,
             stripe_link_url = NULL
         WHERE id = ?1",
        [offer_id],
    )
    .map_err(|e| e.to_string())?;

    if let Some((hd, _)) = &held {
        conn.execute("DELETE FROM harvest_links WHERE harvest_date = ?1", [hd])
            .map_err(|e| e.to_string())?;
    }

    // J3 PRICE-GATE: archive the Price this row held, after the row write.
    if let Some((_, Some(old_price_id))) = &held {
        if !old_price_id.is_empty() {
            let _ = gateway.archive_price(old_price_id);
        }
    }
    Ok(())
}

/// Live offers (have a price) with remaining capacity > 0, for the shop page.
pub fn shop_listings(conn: &Connection) -> Result<Vec<OfferView>, String> {
    let caps = trays::capacity_by_harvest_date(conn)?;
    let mut seen_dates = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for cap in &caps {
        if !seen_dates.insert(cap.harvest_date.clone()) {
            continue;
        }
        for offer in list_offers(conn, &cap.harvest_date)? {
            // Sale side, exact date, per (date, crop) — D21a.
            if trays::remaining_exact_for(conn, &offer.harvest_date, &offer.crop_id)? <= 0 {
                continue;
            }
            let has_price = offer
                .stripe_price_id
                .as_ref()
                .is_some_and(|s| !s.is_empty());
            if offer.remaining > 0 && has_price && offer.price_cents.is_some() {
                out.push(offer);
            }
        }
    }
    out.sort_by(|a, b| {
        a.harvest_date
            .cmp(&b.harvest_date)
            .then_with(|| a.crop_name.cmp(&b.crop_name))
    });
    Ok(out)
}

/// Deactivate every stored harvest Payment Link and clear `harvest_links`.
/// Called once on shop generate so no stale link can still take money.
pub fn retire_harvest_links<G: StripeGateway>(
    conn: &mut Connection,
    gateway: &G,
) -> Result<(), String> {
    let mut stmt = conn
        .prepare("SELECT stripe_link_id FROM harvest_links")
        .map_err(|e| e.to_string())?;
    let ids: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(stmt);

    for id in &ids {
        let _ = gateway.deactivate_link(id);
    }
    conn.execute("DELETE FROM harvest_links", [])
        .map_err(|e| e.to_string())?;
    Ok(())
}
