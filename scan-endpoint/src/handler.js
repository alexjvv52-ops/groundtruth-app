/**
 * Scan endpoint (GT-D16, narrowed and extended by GT-D18, GT-D21).
 *
 *   GET  /  and /s      record one scan, then redirect to REDIRECT_URL
 *                       (packs printed before tokens; unchanged)
 *   GET  /s/<token>     record one scan, then serve the customer page
 *   POST /s/<token>     the write door: store one standing-request candidate
 *   GET  /a/<token>     the field-terminal page (GT-D21). Records no scan.
 *   POST /a/<token>     store one phone-proposal candidate
 *   GET  /a/sw.js       the field page's service worker
 *   GET  /scans         serve the scan count since a cursor. Read-only. Token-gated.
 *   GET  /standing-requests   serve candidate rows after a seq cursor. Read-only. Token-gated.
 *   GET  /field-proposals     candidate rows after a seq cursor. Read-only. Token-gated.
 *
 * What it holds (GT-D18): scans (id + timestamp) and standing candidates
 * (endpoint-minted request_id, token, bags_per_cycle, requested_at). Nothing
 * else about the farm. It stores nothing about tokens: what the page shows —
 * the sampled varieties and the crop's cycle — rides in the link the farm
 * printed (?v=…&g=…) and is never persisted. It cannot tell a well-formed
 * forged token from a real one; the desktop is the authority and resolves
 * every candidate against its own samples when it pulls (Fence 5). Nothing
 * here writes farm truth. The pull is read-only by construction (SELECT only).
 * The pull is SELECT-only: the desktop can never write here through this door.
 */

const AFTER_RE = /^\d{1,15}$/;
const TOKEN_RE = /^[0-9a-f]{32}$/;
const MAX_BODY_BYTES = 1024;
const MAX_VARIETIES = 12;
const MAX_VARIETY_CHARS = 40;
const MAX_GROWTH_DAYS = 365;
const MAX_BAGS = 999;
const PULL_PAGE_LIMIT = 200;
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const CROP_ID_RE = /^[A-Za-z0-9._-]{1,64}$/;
const FIELD_VERBS = ["move_to_light", "harvest"];
const MAX_TRAYS = 999;
const MAX_NOTE_CHARS = 200;
const MAX_CAPTURED_AT_CHARS = 40;
const MAX_CROPS_IN_LINK = 40;
// Signed 2026-08-17 (rack-side fence 2). ONE set of bytes each.
export const FIELD_LINE = "Field terminal — captures go to the PC to confirm.";
export const FIELD_LABEL_CROP = "Crop";
export const FIELD_VERB_MOVE = "Move to light";
export const FIELD_VERB_HARVEST = "Harvest";
export const FIELD_LABEL_TRAYS = "Trays";
export const FIELD_LABEL_OZ = "oz";
export const FIELD_LABEL_NOTE = "Note (optional)";
export const FIELD_BUTTON = "Capture";
export const FIELD_STATUS_QUEUED = "Queued";
export const FIELD_STATUS_SENT = "Sent to the relay";
export const FIELD_HINT = "Queued captures send when the phone is online. Sent means the relay has it; the PC decides at the desk.";
export const FIELD_REFUSED = "The relay refused this capture. Nothing was sent.";
export const FIELD_NO_CROPS = "This link has no crops. Pair again from the PC.";
export const FIELD_SEND_QUEUED = "Send queued";
export const FIELD_NETWORK = "Could not reach the relay. Try again when you have a signal.";

// Signed 2026-08-17 (customer QR fence 4). ONE set of bytes each.
export const NOT_OURS = "This code isn't one of ours. Nothing was recorded.";
export const NOT_READY = "This page isn't ready yet. Nothing was recorded.";
export const BAD_QUANTITY =
  "Choose how many bags per cycle — a whole number, 1 or more.";
export const QUANTITY_LABEL = "Bags per cycle";
export const BUTTON = "Put this on standing";
export function cadenceSentence(days) {
  return `We grow this on a ${days}-day cycle. Standing means we plan for your trays every cycle.`;
}
export function successSentence(farm) {
  return `Request received. ${farm} will confirm before anything goes on standing.`;
}

/**
 * @param {Request} request
 * @param {Record<string, any>} env
 * @param {() => string} nowIso
 * @param {() => string} newId
 * @returns {Promise<Response>}
 */
export async function handleScan(
  request,
  env,
  nowIso = () => new Date().toISOString(),
  newId = () => crypto.randomUUID(),
) {
  const url = new URL(request.url);
  const path = url.pathname;
  if (request.method === "GET") {
    if (path === "/" || path === "/s") return recordAndRedirect(env, nowIso);
    if (path === "/a/sw.js") return serveFieldSw();
    if (path.startsWith("/a/")) return serveFieldPage(url, env);
    if (path.startsWith("/s/")) return servePage(url, env, nowIso);
    if (path === "/scans") return servePull(request, url, env, nowIso);
    if (path === "/standing-requests") return serveStandingPull(request, url, env, nowIso);
    if (path === "/field-proposals") return serveFieldPull(request, url, env, nowIso);
    return json(404, { error: "Not found." });
  }
  if (request.method === "POST" && path.startsWith("/a/")) return writeFieldProposal(request, url, env, nowIso);
  if (request.method === "POST" && path.startsWith("/s/")) {
    return writeStanding(request, url, env, nowIso, newId);
  }
  return json(405, { error: "Method not allowed." });
}

async function recordAndRedirect(env, nowIso) {
  const destination = env.REDIRECT_URL;
  if (!destination) {
    return json(503, { error: "Scan endpoint is not configured." });
  }
  await recordScanBestEffort(env, nowIso);
  return new Response(null, {
    status: 302,
    headers: { Location: destination, "Cache-Control": "no-store" },
  });
}

async function recordScanBestEffort(env, nowIso) {
  try {
    await env.DB.prepare("INSERT INTO scans (scanned_at) VALUES (?1)")
      .bind(nowIso())
      .run();
  } catch (e) {
    // The person holding the pack matters more than the number. Serve
    // anyway. This scan is then invisible downstream, which is why the count
    // is documented as a floor and never as a total.
    console.log("scan insert failed:", e && e.message ? e.message : e);
  }
}

// ---- customer page (GT-D18) ----

async function servePage(url, env, nowIso) {
  const farm = farmName(env);
  if (!farm) {
    return html(503, refusalPage(null, NOT_READY));
  }
  const token = tokenFromPath(url.pathname);
  const varieties = parseVarieties(url.searchParams.getAll("v"));
  const growth = parseGrowth(url.searchParams.getAll("g"));
  if (!token || varieties === null || growth === null) {
    return html(404, refusalPage(farm, NOT_OURS));
  }
  await recordScanBestEffort(env, nowIso);
  const days = Math.max(...growth);
  return html(200, standingPage({ farm, token, varieties, days }));
}

async function writeStanding(request, url, env, nowIso, newId) {
  const farm = farmName(env);
  if (!farm) {
    return json(503, { error: NOT_READY });
  }
  const token = tokenFromPath(url.pathname);
  if (!token) {
    return json(404, { error: NOT_OURS });
  }
  const raw = await readBodyLimited(request, MAX_BODY_BYTES);
  if (!raw.ok) {
    return json(400, { error: BAD_QUANTITY });
  }
  let body;
  try {
    body = JSON.parse(raw.text);
  } catch {
    return json(400, { error: BAD_QUANTITY });
  }
  const bags = body && body.bagsPerCycle;
  if (!Number.isInteger(bags) || bags < 1 || bags > MAX_BAGS) {
    return json(400, { error: BAD_QUANTITY });
  }
  const requestId = newId();
  const requestedAt = nowIso();
  try {
    await env.DB.prepare(
      "INSERT INTO standing_requests (request_id, token, bags_per_cycle, requested_at) VALUES (?1, ?2, ?3, ?4)",
    )
      .bind(requestId, token, bags, requestedAt)
      .run();
  } catch (e) {
    console.log("standing insert failed:", e && e.message ? e.message : e);
    return json(503, { error: NOT_READY });
  }
  return json(200, { requestId, message: successSentence(farm) });
}

function farmName(env) {
  const f = typeof env.FARM_NAME === "string" ? env.FARM_NAME.trim() : "";
  return f.length > 0 ? f : null;
}

function tokenFromPath(pathname) {
  const rest = pathname.slice("/s/".length);
  return TOKEN_RE.test(rest) ? rest : null;
}

function parseVarieties(list) {
  const names = list.map((s) => s.trim()).filter((s) => s.length > 0);
  if (names.length === 0 || names.length > MAX_VARIETIES) return null;
  if (names.some((n) => n.length > MAX_VARIETY_CHARS)) return null;
  return names;
}

function parseGrowth(list) {
  if (list.length === 0 || list.length > MAX_VARIETIES) return null;
  const days = list.map((s) => (/^\d{1,3}$/.test(s) ? Number(s) : NaN));
  if (days.some((d) => !Number.isInteger(d) || d < 1 || d > MAX_GROWTH_DAYS)) return null;
  return days;
}

async function readBodyLimited(request, max) {
  let text;
  try {
    text = await request.text();
  } catch {
    return { ok: false };
  }
  if (text.length > max) return { ok: false };
  return { ok: true, text };
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => (
    { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]
  ));
}

const CSS = `
:root{color-scheme:light}
body{margin:0;font-family:system-ui,-apple-system,Segoe UI,Roboto,sans-serif;color:#111;background:#fff}
main{max-width:28rem;margin:0 auto;padding:1.5rem 1.25rem;display:flex;flex-direction:column;gap:1rem}
h1{font-size:1.25rem;font-weight:600;margin:0}
p{margin:0;font-size:1rem;line-height:1.4}
label{font-size:.95rem}
.qty{display:flex;align-items:center;gap:.5rem}
.qty button{width:3rem;height:3rem;font-size:1.5rem;border:1px solid #bbb;background:#fff;border-radius:.375rem}
.qty input{width:5rem;height:3rem;font-size:1.25rem;text-align:center;border:1px solid #bbb;border-radius:.375rem}
#go{height:3.25rem;font-size:1.1rem;font-weight:600;border:0;border-radius:.375rem;background:#111;color:#fff;width:100%}
#go[disabled]{opacity:.6}
.note{font-size:.95rem;min-height:1.4em}
.line{color:#333}
.verbs{display:flex;gap:.5rem}
.verb{height:3rem;padding:0 1rem;font-size:1rem;border:1px solid #bbb;background:#fff;border-radius:.375rem}
.verb.on{background:#111;color:#fff;border-color:#111}
#note,#oz{height:3rem;font-size:1rem;border:1px solid #bbb;border-radius:.375rem;padding:0 .5rem;width:100%;box-sizing:border-box}
#oz{width:7rem}
.queue{list-style:none;padding:0;margin:0;display:flex;flex-direction:column;gap:.75rem}
.queue li{border-bottom:1px solid #ddd;padding-bottom:.5rem}
.status,.hint{font-size:.9rem;color:#555}
#sendq{height:3rem;font-size:1rem;border:1px solid #bbb;background:#fff;border-radius:.375rem;width:100%}
#sendq[disabled]{opacity:.6}
.err{font-size:.9rem;color:#8a5a00}
`;

function pageJs() {
  return `
(function () {
  var form = document.getElementById("standing");
  var bags = document.getElementById("bags");
  var note = document.getElementById("note");
  var go = document.getElementById("go");
  var notReady = ${JSON.stringify(NOT_READY)};
  function clamp() {
    var n = parseInt(bags.value, 10);
    if (!(n >= 1)) n = 1;
    bags.value = String(n);
    return n;
  }
  document.getElementById("minus").addEventListener("click", function () {
    bags.value = String(Math.max(1, clamp() - 1));
  });
  document.getElementById("plus").addEventListener("click", function () {
    bags.value = String(clamp() + 1);
  });
  form.addEventListener("submit", function (ev) {
    ev.preventDefault();
    var n = clamp();
    go.disabled = true;
    note.textContent = "";
    fetch(window.location.pathname, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ bagsPerCycle: n })
    })
      .then(function (r) { return r.json().then(function (b) { return { ok: r.ok, body: b }; }); })
      .then(function (res) {
        if (res.ok) { go.hidden = true; note.textContent = res.body.message; }
        else { go.disabled = false; note.textContent = res.body.error || notReady; }
      })
      .catch(function () { go.disabled = false; note.textContent = notReady; });
  });
})();`;
}

function shell(title, bodyHtml) {
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="robots" content="noindex"><title>${esc(title)}</title><style>${CSS}</style></head><body><main>${bodyHtml}</main></body></html>`;
}

function refusalPage(farm, sentence) {
  const head = farm ? `<h1>${esc(farm)}</h1>` : "";
  // Signed sentences are one set of bytes (GT-D18). They are constants, not
  // link-carried input; escaping the apostrophe would break that identity.
  return shell(farm || "", `${head}<p class="note">${sentence}</p>`);
}

function standingPage({ farm, token, varieties, days }) {
  const body = `<h1>${esc(farm)}</h1>
<p class="varieties">${esc(varieties.join(", "))}</p>
<p class="cadence">${esc(cadenceSentence(days))}</p>
<form id="standing" data-token="${esc(token)}">
<label for="bags">${esc(QUANTITY_LABEL)}</label>
<div class="qty"><button type="button" id="minus" aria-label="Fewer">−</button><input id="bags" name="bagsPerCycle" type="number" inputmode="numeric" min="1" step="1" value="1"><button type="button" id="plus" aria-label="More">+</button></div>
<button type="submit" id="go">${esc(BUTTON)}</button>
<p id="note" class="note" role="status" aria-live="polite"></p>
</form>
<script>${pageJs()}</script>`;
  return shell(farm, body);
}

// ---- pull (GT-D16, unchanged) ----

async function servePull(request, url, env, nowIso) {
  const token = env.PULL_TOKEN;
  if (!token || request.headers.get("X-Pull-Token") !== token) {
    return json(401, { error: "Unauthorized." });
  }
  const raw = url.searchParams.get("after") ?? "0";
  if (!AFTER_RE.test(raw)) {
    return json(400, { error: "after must be a non-negative integer." });
  }
  const after = Number(raw);
  const bounds = await env.DB.prepare(
    "SELECT MIN(scan_id) AS first_id, MAX(scan_id) AS max_id FROM scans",
  ).first();
  const counted = await env.DB.prepare(
    "SELECT COUNT(*) AS n FROM scans WHERE scan_id > ?1",
  )
    .bind(after)
    .first();
  return json(200, {
    newCount: counted && counted.n != null ? counted.n : 0,
    firstAvailableId: bounds ? bounds.first_id : null,
    maxScanId: bounds && bounds.max_id != null ? bounds.max_id : 0,
    servedAt: nowIso(),
  });
}

// ---- standing-request pull (GT-D18 → desktop, fence 5). SELECT only. ----
async function serveStandingPull(request, url, env, nowIso) {
  const token = env.PULL_TOKEN;
  if (!token || request.headers.get("X-Pull-Token") !== token) {
    return json(401, { error: "Unauthorized." });
  }
  const raw = url.searchParams.get("after") ?? "0";
  if (!AFTER_RE.test(raw)) {
    return json(400, { error: "after must be a non-negative integer." });
  }
  const after = Number(raw);
  const bounds = await env.DB.prepare(
    "SELECT MIN(seq) AS first_seq, MAX(seq) AS max_seq FROM standing_requests",
  ).first();
  const page = await env.DB.prepare(
    "SELECT seq, request_id, token, bags_per_cycle, requested_at FROM standing_requests WHERE seq > ?1 ORDER BY seq ASC LIMIT ?2",
  )
    .bind(after, PULL_PAGE_LIMIT)
    .all();
  const rows = (page && page.results ? page.results : []).map((r) => ({
    seq: r.seq,
    requestId: r.request_id,
    token: r.token,
    bagsPerCycle: r.bags_per_cycle,
    requestedAt: r.requested_at,
  }));
  return json(200, {
    rows,
    firstAvailableSeq: bounds ? bounds.first_seq : null,
    maxSeq: bounds && bounds.max_seq != null ? bounds.max_seq : 0,
    servedAt: nowIso(),
  });
}

// ---- field terminal (GT-D21) ----

function parseCrops(list) {
  const out = [];
  for (const raw of list) {
    const i = raw.indexOf(":");
    if (i <= 0) continue;
    const id = raw.slice(0, i), name = raw.slice(i + 1).trim();
    if (!CROP_ID_RE.test(id) || name.length === 0 || name.length > MAX_VARIETY_CHARS) continue;
    out.push({ id, name });
    if (out.length >= MAX_CROPS_IN_LINK) break;
  }
  return out;
}

async function serveFieldPage(url, env) {
  const farm = farmName(env);
  if (!farm) return html(503, refusalPage(null, NOT_READY));
  const token = tokenFromPath(url.pathname);
  if (!token) return html(404, refusalPage(farm, NOT_OURS));
  const crops = parseCrops(url.searchParams.getAll("c"));
  // The field page is not a pack scan: no scans row.
  return html(200, fieldPage({ farm, token, crops }));
}

export function validateFieldProposal(body) {
  if (!body || typeof body !== "object") return null;
  const proposalId = typeof body.proposalId === "string" && UUID_RE.test(body.proposalId) ? body.proposalId.toLowerCase() : null;
  const verb = FIELD_VERBS.includes(body.verb) ? body.verb : null;
  const cropId = typeof body.cropId === "string" && CROP_ID_RE.test(body.cropId) ? body.cropId : null;
  const quantity = Number.isInteger(body.quantity) && body.quantity >= 1 && body.quantity <= MAX_TRAYS ? body.quantity : null;
  const at = typeof body.phoneCapturedAt === "string" && body.phoneCapturedAt.length <= MAX_CAPTURED_AT_CHARS
    && !Number.isNaN(Date.parse(body.phoneCapturedAt)) ? body.phoneCapturedAt : null;
  let note = null;
  if (body.note != null) {
    if (typeof body.note !== "string" || body.note.length > MAX_NOTE_CHARS) return null;
    note = body.note.trim() || null;
  }
  let actualYieldOz = null;
  if (verb === "harvest") {
    if (typeof body.actualYieldOz !== "number" || !Number.isFinite(body.actualYieldOz) || body.actualYieldOz <= 0) return null;
    actualYieldOz = body.actualYieldOz;
  } else if (body.actualYieldOz != null) {
    return null;
  }
  if (!proposalId || !verb || !cropId || !quantity || !at) return null;
  return { proposalId, verb, cropId, quantity, actualYieldOz, phoneCapturedAt: at, note };
}

async function writeFieldProposal(request, url, env, nowIso) {
  const farm = farmName(env);
  if (!farm) return json(503, { error: NOT_READY });
  const token = tokenFromPath(url.pathname);
  if (!token) return json(404, { error: NOT_OURS });
  const raw = await readBodyLimited(request, MAX_BODY_BYTES);
  if (!raw.ok) return json(400, { error: FIELD_REFUSED });
  let body;
  try { body = JSON.parse(raw.text); } catch { return json(400, { error: FIELD_REFUSED }); }
  const p = validateFieldProposal(body);
  if (!p) return json(400, { error: FIELD_REFUSED });
  try {
    await env.DB.prepare(
      "INSERT OR IGNORE INTO field_proposals (proposal_id, device_token, verb, crop_id, quantity, actual_yield_oz, phone_captured_at, note, received_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    ).bind(p.proposalId, token, p.verb, p.cropId, p.quantity, p.actualYieldOz, p.phoneCapturedAt, p.note, nowIso()).run();
  } catch (e) {
    console.log("field insert failed:", e && e.message ? e.message : e);
    return json(503, { error: NOT_READY });
  }
  return json(200, { proposalId: p.proposalId });
}

// SELECT only. Same PULL_TOKEN, same shape as /standing-requests.
async function serveFieldPull(request, url, env, nowIso) {
  const token = env.PULL_TOKEN;
  if (!token || request.headers.get("X-Pull-Token") !== token) return json(401, { error: "Unauthorized." });
  const raw = url.searchParams.get("after") ?? "0";
  if (!AFTER_RE.test(raw)) return json(400, { error: "after must be a non-negative integer." });
  const after = Number(raw);
  const bounds = await env.DB.prepare("SELECT MIN(seq) AS first_seq, MAX(seq) AS max_seq FROM field_proposals").first();
  const page = await env.DB.prepare(
    "SELECT seq, proposal_id, device_token, verb, crop_id, quantity, actual_yield_oz, phone_captured_at, note, received_at FROM field_proposals WHERE seq > ?1 ORDER BY seq ASC LIMIT ?2",
  ).bind(after, PULL_PAGE_LIMIT).all();
  const rows = (page && page.results ? page.results : []).map((r) => ({
    seq: r.seq, proposalId: r.proposal_id, deviceToken: r.device_token, verb: r.verb, cropId: r.crop_id,
    quantity: r.quantity, actualYieldOz: r.actual_yield_oz, phoneCapturedAt: r.phone_captured_at,
    note: r.note, receivedAt: r.received_at,
  }));
  return json(200, { rows, firstAvailableSeq: bounds ? bounds.first_seq : null,
    maxSeq: bounds && bounds.max_seq != null ? bounds.max_seq : 0, servedAt: nowIso() });
}

function serveFieldSw() {
  return new Response(FIELD_SW, { status: 200, headers: { "Content-Type": "application/javascript; charset=utf-8", "Cache-Control": "no-store" } });
}

// Network-first with cache fallback for GETs under /a/: a re-pair or redeploy shows on
// the next online open; with no signal the last good page shell opens. POSTs never touch it.
const FIELD_SW = `
const CACHE = "gt-field-v1";
self.addEventListener("install", () => { self.skipWaiting(); });
self.addEventListener("activate", (e) => { e.waitUntil(self.clients.claim()); });
self.addEventListener("fetch", (e) => {
  const req = e.request;
  if (req.method !== "GET") return;
  const url = new URL(req.url);
  if (!url.pathname.startsWith("/a/")) return;
  e.respondWith((async () => {
    const cache = await caches.open(CACHE);
    try {
      const fresh = await fetch(req);
      if (fresh && fresh.ok) await cache.put(req, fresh.clone());
      return fresh;
    } catch (err) {
      const hit = await cache.match(req);
      if (hit) return hit;
      throw err;
    }
  })());
});
`;

function fieldPage({ farm, token, crops }) {
  const cropsJson = JSON.stringify(crops).replace(/</g, "\\u003c");
  const body = `<h1>${esc(farm)}</h1>
<p class="line">${esc(FIELD_LINE)}</p>
<form id="cap" data-token="${esc(token)}">
<label for="crop">${esc(FIELD_LABEL_CROP)}</label>
<select id="crop"></select>
<div class="verbs" role="group"><button type="button" id="v-move" class="verb on" aria-pressed="true">${esc(FIELD_VERB_MOVE)}</button><button type="button" id="v-harvest" class="verb" aria-pressed="false">${esc(FIELD_VERB_HARVEST)}</button></div>
<label for="trays">${esc(FIELD_LABEL_TRAYS)}</label>
<div class="qty"><button type="button" id="minus" aria-label="Fewer trays">−</button><input id="trays" type="number" inputmode="numeric" min="1" max="999" step="1" value="1"><button type="button" id="plus" aria-label="More trays">+</button></div>
<div id="ozrow" hidden><label for="oz">${esc(FIELD_LABEL_OZ)}</label> <input id="oz" type="number" inputmode="decimal" min="0" step="0.1"></div>
<label for="note">${esc(FIELD_LABEL_NOTE)}</label>
<input id="note" type="text" maxlength="200">
<button type="submit" id="go">${esc(FIELD_BUTTON)}</button>
<p id="line" class="note" role="status" aria-live="polite"></p>
</form>
<ul id="queue" class="queue"></ul>
<button type="button" id="sendq" disabled>${esc(FIELD_SEND_QUEUED)}</button>
<p class="hint">${esc(FIELD_HINT)}</p>
<script id="crops" type="application/json">${cropsJson}</script>
<script>${fieldJs()}</script>`;
  return shell(farm, body);
}

function fieldJs() {
  return `
(function () {
  var crops = JSON.parse(document.getElementById("crops").textContent || "[]");
  var form = document.getElementById("cap"), token = form.getAttribute("data-token");
  var KEY = "gt-field-queue:" + token;
  var NO_CROPS = ${JSON.stringify(FIELD_NO_CROPS)}, NETWORK = ${JSON.stringify(FIELD_NETWORK)};
  var QUEUED = ${JSON.stringify(FIELD_STATUS_QUEUED)}, SENT = ${JSON.stringify(FIELD_STATUS_SENT)};
  var sel = document.getElementById("crop"), trays = document.getElementById("trays"), oz = document.getElementById("oz");
  var ozrow = document.getElementById("ozrow"), note = document.getElementById("note"), go = document.getElementById("go");
  var line = document.getElementById("line"), list = document.getElementById("queue"), sendq = document.getElementById("sendq");
  var vMove = document.getElementById("v-move"), vHarvest = document.getElementById("v-harvest");
  var verb = "move_to_light";
  crops.forEach(function (c) { var o = document.createElement("option"); o.value = c.id; o.textContent = c.name; sel.appendChild(o); });
  if (crops.length === 0) { line.textContent = NO_CROPS; go.disabled = true; sel.disabled = true; }
  function setVerb(v) {
    verb = v;
    vMove.classList.toggle("on", v === "move_to_light"); vHarvest.classList.toggle("on", v === "harvest");
    vMove.setAttribute("aria-pressed", String(v === "move_to_light")); vHarvest.setAttribute("aria-pressed", String(v === "harvest"));
    ozrow.hidden = v !== "harvest";
  }
  vMove.addEventListener("click", function () { setVerb("move_to_light"); });
  vHarvest.addEventListener("click", function () { setVerb("harvest"); });
  function clampTrays() { var n = parseInt(trays.value, 10); if (!(n >= 1)) n = 1; if (n > 999) n = 999; trays.value = String(n); return n; }
  document.getElementById("minus").addEventListener("click", function () { trays.value = String(Math.max(1, clampTrays() - 1)); });
  document.getElementById("plus").addEventListener("click", function () { trays.value = String(Math.min(999, clampTrays() + 1)); });
  function load() { try { return JSON.parse(localStorage.getItem(KEY) || "[]"); } catch (e) { return []; } }
  function save(items) { localStorage.setItem(KEY, JSON.stringify(items)); }
  // Per-row write: re-read, patch by proposalId, write back — a capture added while a send is
  // in flight is never overwritten by a stale array.
  function update(id, patch) {
    var items = load();
    for (var k = 0; k < items.length; k++) { if (items[k].proposalId === id) { for (var key in patch) items[k][key] = patch[key]; } }
    save(items);
  }
  function isPending(it) { return it.status !== "sent"; }
  function trayWord(n) { return n === 1 ? "1 tray" : n + " trays"; }
  function pad2(n) { return n < 10 ? "0" + n : String(n); }
  function clock(d) { var h = d.getHours(), m = d.getMinutes(), ap = h >= 12 ? "pm" : "am", h12 = h % 12; if (h12 === 0) h12 = 12; return h12 + ":" + pad2(m) + " " + ap; }
  var DAYS = ["Sun","Mon","Tue","Wed","Thu","Fri","Sat"], MONTHS = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
  function monDay(d) { return DAYS[d.getDay()] + " " + MONTHS[d.getMonth()] + " " + d.getDate(); }
  function age(iso) {
    var d = new Date(iso), n = new Date();
    var a = new Date(d.getFullYear(), d.getMonth(), d.getDate()), b = new Date(n.getFullYear(), n.getMonth(), n.getDate());
    var diff = Math.round((b - a) / 86400000), t = clock(d);
    if (diff === 0) return "today at " + t; if (diff === 1) return "yesterday at " + t; return monDay(d) + " at " + t;
  }
  function card(it) {
    var t = trayWord(it.quantity), m;
    if (it.verb === "harvest") m = "Harvest " + t + " of " + it.cropName + ", " + Number(it.actualYieldOz).toFixed(1) + " oz — captured " + age(it.phoneCapturedAt) + ".";
    else m = "Move " + t + " of " + it.cropName + " to light — captured " + age(it.phoneCapturedAt) + ".";
    if (it.note) m += " Note: " + it.note + ".";
    return m;
  }
  function render() {
    var items = load(); list.textContent = "";
    items.slice().reverse().forEach(function (it) {
      var li = document.createElement("li"), p = document.createElement("p"), s = document.createElement("p");
      p.textContent = card(it); s.className = "status"; s.textContent = isPending(it) ? QUEUED : SENT;
      li.appendChild(p); li.appendChild(s);
      if (isPending(it) && it.error) { var e = document.createElement("p"); e.className = "err"; e.textContent = it.error; li.appendChild(e); }
      list.appendChild(li);
    });
    sendq.disabled = sending || load().filter(isPending).length === 0;
  }
  var sending = false;
  // One attempt per queued row, in order, same endpoint and body as before; a failure on one row
  // does not stop the next. On success the row is Sent; on an error body the row stays Queued
  // and shows the relay's own sentence; with no answer it shows NETWORK.
  function send() {
    if (sending) return; sending = true; render();
    var pending = load().filter(isPending).map(function (it) { return it.proposalId; });
    var i = 0;
    function finish() { sending = false; render(); }
    function next() {
      if (i >= pending.length) { finish(); return; }
      var it = load().filter(function (x) { return x.proposalId === pending[i]; })[0];
      if (!it || !isPending(it)) { i++; next(); return; }
      fetch(window.location.pathname, { method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ proposalId: it.proposalId, verb: it.verb, cropId: it.cropId, quantity: it.quantity,
          actualYieldOz: it.verb === "harvest" ? it.actualYieldOz : null, phoneCapturedAt: it.phoneCapturedAt, note: it.note || null }) })
        .then(function (r) {
          if (r.ok) { update(it.proposalId, { status: "sent", error: null }); i++; render(); next(); return; }
          return r.json().then(function (b) { return b && typeof b.error === "string" ? b.error : NETWORK; }, function () { return NETWORK; })
            .then(function (msg) { update(it.proposalId, { status: "queued", error: msg }); i++; render(); next(); });
        })
        .catch(function () { update(it.proposalId, { status: "queued", error: NETWORK }); i++; render(); next(); });
    }
    next();
  }
  form.addEventListener("submit", function (ev) {
    ev.preventDefault();
    if (crops.length === 0) return;
    var n = clampTrays();
    var it = { proposalId: crypto.randomUUID(), verb: verb, cropId: sel.value, cropName: sel.options[sel.selectedIndex].textContent,
      quantity: n, actualYieldOz: verb === "harvest" ? Number(oz.value) : null, phoneCapturedAt: new Date().toISOString(),
      note: (note.value || "").trim().slice(0, 200), status: "queued", error: null };
    var items = load(); items.push(it); save(items); render();
    note.value = ""; oz.value = "";
    send();
  });
  sendq.addEventListener("click", send);
  window.addEventListener("online", send);
  render(); send();
  if ("serviceWorker" in navigator) { navigator.serviceWorker.register("/a/sw.js").catch(function () {}); }
})();`;
}

function json(status, body) {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": "no-store",
    },
  });
}

function html(status, body) {
  return new Response(body, {
    status,
    headers: {
      "Content-Type": "text/html; charset=utf-8",
      "Cache-Control": "no-store",
    },
  });
}
