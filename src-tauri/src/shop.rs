//! Static customer shop page — cart on the farm page, Stripe collects payment only.

use crate::db;
use crate::models::{OfferView, ShopPage};
use crate::money;
use crate::offers;
use chrono::{Datelike, Local, NaiveDate, Timelike};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

pub fn shop_dir(folder_path: &Path) -> PathBuf {
    folder_path.join("shop")
}

pub fn shop_page_path(folder_path: &Path) -> PathBuf {
    shop_dir(folder_path).join("index.html")
}

pub fn generate_shop_page(conn: &mut Connection, folder_path: &Path) -> Result<ShopPage, String> {
    let gw = money::gateway_from_db(conn)?;
    generate_shop_page_with(conn, &gw, folder_path)
}

pub fn generate_shop_page_with<G: money::StripeGateway>(
    conn: &mut Connection,
    gateway: &G,
    folder_path: &Path,
) -> Result<ShopPage, String> {
    let checkout_url = money::checkout_endpoint_url(conn)?.ok_or_else(|| {
        "Add your checkout address in Sell online before publishing a shop page.".to_string()
    })?;

    // Retire every harvest Payment Link so nothing unpaid can still charge at an old price.
    offers::retire_harvest_links(conn, gateway)?;

    let listings = offers::shop_listings(conn)?;

    let mut dates: Vec<String> = Vec::new();
    for item in &listings {
        if !dates.contains(&item.harvest_date) {
            dates.push(item.harvest_date.clone());
        }
    }

    let generated_at = db::utc_now_rfc3339();
    let as_of = format_as_of_line(Local::now());
    let farm_name = crate::invoice::farm_display_name(conn)?
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    let title = farm_name.as_deref().unwrap_or(SHOP_UNNAMED_TITLE);
    let html = render_html(&listings, &as_of, &checkout_url, title);

    let bytes = html.as_bytes();
    if bytes.len() >= 100 * 1024 {
        return Err(format!(
            "Shop page is {} bytes — over the 100 KB budget. Remove content and try again.",
            bytes.len()
        ));
    }

    let lower = html.to_ascii_lowercase();
    if lower.contains("rk_") || lower.contains("sk_") {
        return Err(
            "Refusing to write a shop page that looks like it contains a Stripe key.".into(),
        );
    }

    let dir = shop_dir(folder_path);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = shop_page_path(folder_path);
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;

    let file_path = path
        .to_str()
        .ok_or_else(|| "Shop page path is not valid UTF-8".to_string())?
        .to_string();

    Ok(ShopPage {
        file_path,
        size_bytes: bytes.len() as i64,
        generated_at,
        harvest_dates: dates,
    })
}

fn format_as_of_line(now: chrono::DateTime<Local>) -> String {
    let time = {
        let h24 = now.hour();
        let m = now.minute();
        let (h12, ampm) = match h24 {
            0 => (12, "am"),
            1..=11 => (h24, "am"),
            12 => (12, "pm"),
            _ => (h24 - 12, "pm"),
        };
        if m == 0 {
            format!("{h12} {ampm}")
        } else {
            format!("{h12}:{m:02} {ampm}")
        }
    };
    let weekday = now.format("%A").to_string();
    format!("{time} {weekday}")
}

fn format_ready_date(yyyy_mm_dd: &str) -> String {
    let Ok(d) = NaiveDate::parse_from_str(yyyy_mm_dd, "%Y-%m-%d") else {
        return yyyy_mm_dd.to_string();
    };
    let months = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let weekday = d.format("%A").to_string();
    format!(
        "Ready {weekday} {} {}",
        d.day(),
        months[d.month0() as usize]
    )
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn format_price_cad(cents: i64) -> String {
    let dollars = cents / 100;
    let rem = (cents % 100).abs();
    format!("${dollars}.{rem:02}")
}

/// C1 — the cart page's title when Settings has no farm name yet. Never a
/// brand: the page carries the operator's own name or nothing at all.
const SHOP_UNNAMED_TITLE: &str = "Shop";

fn render_html(listings: &[OfferView], as_of: &str, checkout_url: &str, title: &str) -> String {
    let next_date = listings
        .first()
        .map(|o| o.harvest_date.as_str())
        .unwrap_or("");
    let ready = if next_date.is_empty() {
        "Nothing for sale right now".to_string()
    } else {
        format_ready_date(next_date)
    };

    let mut items_html = String::new();
    let mut idx = 0usize;
    for offer in listings {
        if offer.harvest_date != next_date {
            continue;
        }
        let Some(price_id) = offer.stripe_price_id.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        let cents = offer.price_cents.unwrap_or(0);
        let price = format_price_cad(cents);
        items_html.push_str(&format!(
            r#"<div class="item" data-price-id="{price_id}" data-price-cents="{cents}" data-remaining="{rem}" data-available="{avail}" data-sold="{sold}">
  <div class="row">
    <div>
      <div class="name">{name}</div>
      <div class="price">{price} · {rem} left</div>
    </div>
    <div class="stepper" data-stepper="{idx}">
      <button type="button" class="step" data-dir="-1" aria-label="Fewer">−</button>
      <span class="qty" data-qty="{idx}">0</span>
      <button type="button" class="step" data-dir="1" aria-label="More">+</button>
    </div>
  </div>
</div>
"#,
            price_id = esc(price_id),
            cents = cents,
            rem = offer.remaining,
            avail = offer.available,
            sold = offer.sold,
            name = esc(&offer.crop_name),
            price = price,
            idx = idx,
        ));
        idx += 1;
    }

    if items_html.is_empty() {
        items_html = r#"<p class="muted">Nothing available to buy right now.</p>"#.to_string();
    }

    let harvest_attr = esc(next_date);
    let checkout_attr = esc(checkout_url);
    let pay_block = if next_date.is_empty() || idx == 0 {
        r#"<p class="muted">Nothing available to buy right now.</p>"#.to_string()
    } else {
        r#"<div class="total-row">Total <strong id="total">$0.00</strong></div>
<button type="button" class="pay" id="pay" disabled>Pay</button>
<p class="err" id="err" hidden></p>"#
            .to_string()
    };

    // Vanilla JS only. Reference minted per Pay attempt so cancel-and-retry gets a new Session.
    let script = r#"<script>
(function(){
  var root=document.getElementById('cart');
  if(!root)return;
  var endpoint=root.getAttribute('data-checkout');
  var harvest=root.getAttribute('data-harvest');
  var items=[].slice.call(document.querySelectorAll('.item[data-price-id]'));
  var totalEl=document.getElementById('total');
  var pay=document.getElementById('pay');
  var err=document.getElementById('err');
  var busy=false;
  function money(c){return '$'+(c/100).toFixed(2);}
  function readCart(){
    var lines=[],total=0;
    items.forEach(function(el){
      var qty=parseInt(el.querySelector('.qty').textContent,10)||0;
      var max=parseInt(el.getAttribute('data-remaining'),10)||0;
      if(qty<0)qty=0;if(qty>max)qty=max;
      el.querySelector('.qty').textContent=String(qty);
      if(qty>=1){
        var cents=parseInt(el.getAttribute('data-price-cents'),10)||0;
        lines.push({priceId:el.getAttribute('data-price-id'),quantity:qty});
        total+=cents*qty;
      }
    });
    return {lines:lines,total:total};
  }
  function paint(){
    var c=readCart();
    if(totalEl)totalEl.textContent=money(c.total);
    if(pay)pay.disabled=busy||c.lines.length===0;
  }
  function showErr(msg){if(!err)return;err.hidden=!msg;err.textContent=msg||'';}
  items.forEach(function(el){
    el.querySelectorAll('.step').forEach(function(btn){
      btn.addEventListener('click',function(){
        var qtyEl=el.querySelector('.qty');
        var qty=parseInt(qtyEl.textContent,10)||0;
        var max=parseInt(el.getAttribute('data-remaining'),10)||0;
        var dir=parseInt(btn.getAttribute('data-dir'),10)||0;
        qty=Math.max(0,Math.min(max,qty+dir));
        qtyEl.textContent=String(qty);
        showErr('');
        paint();
      });
    });
  });
  if(pay){
    pay.addEventListener('click',function(){
      if(busy||pay.disabled)return;
      var c=readCart();
      if(!c.lines.length)return;
      var reference=(crypto.randomUUID&&crypto.randomUUID())||(Date.now()+'-'+Math.random().toString(16).slice(2));
      busy=true;paint();showErr('');
      fetch(endpoint,{
        method:'POST',
        headers:{'Content-Type':'application/json'},
        body:JSON.stringify({
          reference:reference,
          harvestDate:harvest,
          currency:'cad',
          total:c.total,
          lines:c.lines
        })
      }).then(function(res){
        if(res.status===200)return res.json().then(function(body){
          if(body&&body.url){window.location=body.url;return;}
          throw new Error('no url');
        });
        if(res.status===409){
          showErr('Prices have changed. Reload the page and try again.');
          return;
        }
        showErr("Couldn't start checkout. Try again in a moment.");
      }).catch(function(){
        showErr("Couldn't start checkout. Try again in a moment.");
      }).then(function(){
        busy=false;paint();
      });
    });
  }
  paint();
})();
</script>"#;

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title}</title>
<style>
:root{{--fg:#14201a;--muted:#5c6b63;--line:#d7ddd9;--bg:#f7f5f0;--btn:#1f3d2f;--btnfg:#fff;--btn-dis:#9aa89f}}
*{{box-sizing:border-box}}
body{{margin:0;font:16px/1.45 system-ui,-apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif;color:var(--fg);background:var(--bg)}}
main{{max-width:28rem;margin:0 auto;padding:1.25rem 1.1rem 3rem}}
h1{{font-size:1.55rem;font-weight:650;margin:0 0 .75rem;letter-spacing:-.02em}}
.item{{border-top:1px solid var(--line);padding:.9rem 0}}
.item:first-of-type{{border-top:0;padding-top:0}}
.row{{display:flex;align-items:center;justify-content:space-between;gap:1rem}}
.name{{font-weight:600}}
.price,.muted{{color:var(--muted);font-size:.92rem}}
.stepper{{display:flex;align-items:center;gap:.35rem}}
.step{{width:2.4rem;height:2.4rem;border:1px solid var(--line);border-radius:.5rem;background:#fff;font-size:1.25rem;line-height:1;color:var(--fg)}}
.qty{{min-width:1.5rem;text-align:center;font-weight:600}}
.total-row{{display:flex;justify-content:space-between;align-items:baseline;margin:1.1rem 0 .5rem;font-size:1.1rem}}
.pay{{display:block;width:100%;margin:.35rem 0 .75rem;padding:.95rem 1rem;border:0;border-radius:.65rem;background:var(--btn);color:var(--btnfg);font-size:1.1rem;font-weight:600}}
.pay:disabled{{background:var(--btn-dis);color:#eef2ef}}
.err{{color:#8a2b2b;font-size:.92rem;margin:.25rem 0 .75rem}}
.note{{font-size:.88rem;color:var(--muted);margin:0 0 2.5rem}}
.below{{border-top:1px solid var(--line);padding-top:1.5rem;color:var(--muted);font-size:.95rem}}
.below h2{{color:var(--fg);font-size:1.05rem;margin:0 0 .5rem}}
.below p{{margin:.4rem 0}}
</style>
</head>
<body>
<main id="cart" data-checkout="{checkout}" data-harvest="{harvest}">
<h1>{ready}</h1>
<div id="items">
{items}
</div>
{pay}
<p class="note">Availability shown as of {as_of}. Not a guarantee — if we sell out we'll refund you or offer a substitute.</p>
<section class="below">
<h2>Who we are</h2>
<p>Small-batch microgreens, grown locally. Collection details are on your Stripe receipt after you pay.</p>
<p>Questions? Reply to the receipt email — Stripe's receipt is your confirmation.</p>
</section>
</main>
{script}
</body>
</html>
"#,
        ready = esc(&ready),
        items = items_html,
        pay = pay_block,
        as_of = esc(as_of),
        checkout = checkout_attr,
        harvest = harvest_attr,
        script = if idx == 0 { "" } else { script },
        title = esc(title),
    )
}

// ---------------------------------------------------------------------------
// SHOP DOOR Job A (signed WHAT A · WHERE A · WRITE A · PAY A · CAP A · GT-D24
// VIEWER OK). A read-only page of the leftover lots Money already minted a
// Payment Link for and nobody has paid yet. It composes from
// leftover::listings, writes the same <farm folder>/shop/index.html the cart
// page used, and mints nothing: no offer, no harvest link, no checkout
// address, no Stripe call, no poll, no event, no flush. The cart composer
// above is left byte for byte; hosting stays by hand (write file + Open
// folder on Money).
// ---------------------------------------------------------------------------

/// The empty page's one sentence — the cart page's "Nothing for sale right
/// now" precedent (render_html), as the signed brief spells it.
pub const SHOP_DOOR_EMPTY_LINE: &str = "Nothing for sale right now.";

/// The cart page's availability sentence (render_html), reused verbatim after
/// "Availability shown as of {as_of}."
pub const SHOP_DOOR_AVAILABILITY_TAIL: &str =
    "Not a guarantee — if we sell out we'll refund you or offer a substitute.";

/// The cart page's key refusal (generate_shop_page_with), reused verbatim.
pub const SHOP_DOOR_KEY_REFUSAL_LINE: &str =
    "Refusing to write a shop page that looks like it contains a Stripe key.";

/// The lots the page prints: listings that are minted (payment_link_url and
/// priced_total_cents stored) and unpaid (paid_at NULL). The reader's own
/// order is kept — newest harvest first, then the crop order.
fn shop_door_lots(conn: &Connection) -> Result<Vec<crate::leftover::LeftoverListingView>, String> {
    Ok(crate::leftover::listings(conn)?
        .into_iter()
        .filter(|l| {
            l.paid_at.is_none() && l.payment_link_url.is_some() && l.priced_total_cents.is_some()
        })
        .collect())
}

/// The composer. Gate order: the farm display name (INV-A's gate and
/// sentence, invoice.rs — the name is the page's identity line), then the
/// 100 KB budget and the key refusal the cart page already applies. Nothing
/// is written on any refusal. Two clock reads (as-of line, generated_at):
/// this is a handler, never an apply_*.
pub fn write_leftover_shop_page(conn: &Connection, folder_path: &Path) -> Result<ShopPage, String> {
    let farm_name = crate::invoice::farm_display_name(conn)?
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| crate::invoice::INVOICE_NEEDS_FARM_NAME_LINE.to_string())?;
    let lots = shop_door_lots(conn)?;
    let mut harvest_dates: Vec<String> = Vec::new();
    for lot in &lots {
        if !harvest_dates.contains(&lot.harvested_on) {
            harvest_dates.push(lot.harvested_on.clone());
        }
    }
    let generated_at = db::utc_now_rfc3339();
    let as_of = format_as_of_line(Local::now());
    let html = render_leftover_html(&farm_name, &lots, &as_of);
    let bytes = html.as_bytes();
    if bytes.len() >= 100 * 1024 {
        return Err(format!(
            "Shop page is {} bytes — over the 100 KB budget. Remove content and try again.",
            bytes.len()
        ));
    }
    let lower = html.to_ascii_lowercase();
    if lower.contains("rk_") || lower.contains("sk_") {
        return Err(SHOP_DOOR_KEY_REFUSAL_LINE.to_string());
    }
    let dir = shop_dir(folder_path);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = shop_page_path(folder_path);
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let file_path = path
        .to_str()
        .ok_or_else(|| "Shop page path is not valid UTF-8".to_string())?
        .to_string();
    Ok(ShopPage {
        file_path,
        size_bytes: bytes.len() as i64,
        generated_at,
        harvest_dates,
    })
}

/// Page bytes. Identity line = the farm display name (no brand string). One
/// row per lot: crop · harvested day · listed oz · price, then the stored
/// payment_link_url — verbatim, as the link and as its text — behind the
/// bill's own words "Pay online:". No script, no cart, no third-party origin,
/// and no code image: one 49-module symbol drawn as rects costs more bytes
/// than this whole page, so URL-only keeps the budget (CAP A).
fn render_leftover_html(
    farm_name: &str,
    lots: &[crate::leftover::LeftoverListingView],
    as_of: &str,
) -> String {
    let mut lots_html = String::new();
    for lot in lots {
        let (Some(url), Some(cents)) = (lot.payment_link_url.as_deref(), lot.priced_total_cents)
        else {
            continue;
        };
        lots_html.push_str(&format!(
            r#"<li class="lot">
<p class="line">{crop} · harvested {day} · {oz:.1} oz · {price}</p>
<p class="pay">Pay online: <a href="{url}">{url}</a></p>
</li>
"#,
            crop = esc(&lot.crop_name),
            day = esc(&lot.harvested_on),
            oz = lot.listed_oz,
            price = format_price_cad(cents),
            url = esc(url),
        ));
    }
    let body = if lots_html.is_empty() {
        format!(r#"<p class="muted">{SHOP_DOOR_EMPTY_LINE}</p>"#)
    } else {
        format!("<ul class=\"lots\">\n{lots_html}</ul>")
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{name}</title>
<style>
:root{{--fg:#14201a;--muted:#5c6b63;--line:#d7ddd9;--bg:#f7f5f0}}
*{{box-sizing:border-box}}
body{{margin:0;font:16px/1.45 system-ui,-apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif;color:var(--fg);background:var(--bg)}}
main{{max-width:28rem;margin:0 auto;padding:1.25rem 1.1rem 3rem}}
h1{{font-size:1.55rem;font-weight:650;margin:0 0 .75rem;letter-spacing:-.02em}}
.lots{{list-style:none;margin:0;padding:0}}
.lot{{border-top:1px solid var(--line);padding:.9rem 0}}
.lot:first-child{{border-top:0;padding-top:0}}
.line{{margin:0 0 .35rem;font-weight:600}}
.pay{{margin:0;font-size:.92rem;overflow-wrap:anywhere}}
.pay a{{color:var(--fg)}}
.muted{{color:var(--muted);font-size:.92rem}}
.note{{font-size:.88rem;color:var(--muted);margin:1.5rem 0 0}}
</style>
</head>
<body>
<main>
<h1>{name}</h1>
{body}
<p class="note">Availability shown as of {as_of}. {tail}</p>
</main>
</body>
</html>
"#,
        name = esc(farm_name),
        body = body,
        as_of = esc(as_of),
        tail = SHOP_DOOR_AVAILABILITY_TAIL,
    )
}
