import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  handleScan, NOT_OURS, NOT_READY, BAD_QUANTITY, QUANTITY_LABEL, BUTTON,
  cadenceSentence, successSentence,
} from "../src/handler.js";

const NOW = "2026-08-17T18:00:00.000Z";
const TOKEN = "0123456789abcdef0123456789abcdef";
const FARM = "Test Farm";
const ORIGIN = "https://scans.example";

function fakeDb({ failInsert = false } = {}) {
  const sql = [];
  const scans = [];
  const requests = [];
  let nextScan = 1;
  let nextSeq = 1;
  const db = {
    sql, scans, requests,
    prepare(text) {
      sql.push(text);
      let bound = [];
      const stmt = {
        bind(...args) { bound = args; return stmt; },
        async run() {
          if (failInsert) throw new Error("D1 unavailable");
          if (/INSERT INTO scans/i.test(text)) { scans.push(nextScan++); return { success: true }; }
          if (/INSERT INTO standing_requests/i.test(text)) {
            requests.push({ seq: nextSeq++, request_id: bound[0], token: bound[1], bags_per_cycle: bound[2], requested_at: bound[3] });
            return { success: true };
          }
          throw new Error("unexpected write: " + text);
        },
        async first() {
          if (/MIN\(seq\)/i.test(text)) {
            return requests.length
              ? { first_seq: Math.min(...requests.map((r) => r.seq)), max_seq: Math.max(...requests.map((r) => r.seq)) }
              : { first_seq: null, max_seq: null };
          }
          throw new Error("unexpected read: " + text);
        },
        async all() {
          if (/FROM standing_requests WHERE seq > \?1/i.test(text)) {
            const after = Number(bound[0] ?? 0);
            const limit = Number(bound[1] ?? 200);
            const results = requests.filter((r) => r.seq > after).sort((a, b) => a.seq - b.seq).slice(0, limit)
              .map((r) => ({ seq: r.seq, request_id: r.request_id, token: r.token, bags_per_cycle: r.bags_per_cycle, requested_at: r.requested_at }));
            return { results };
          }
          throw new Error("unexpected read: " + text);
        },
      };
      return stmt;
    },
  };
  return db;
}
function envFor(db, extras = {}) {
  return { REDIRECT_URL: "https://example.com/shop", PULL_TOKEN: "pull_fixture", FARM_NAME: FARM, DB: db, ...extras };
}
function pageUrl(token = TOKEN, query = "?v=Dun%20peas&v=Red%20arrow%20radish&g=9&g=7") {
  return `${ORIGIN}/s/${token}${query}`;
}
function get(url) { return new Request(url, { method: "GET" }); }
function post(url, body, raw = false) {
  return new Request(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: raw ? body : JSON.stringify(body),
  });
}
const NEW_ID = () => "req-fixture-1";

describe("customer page (GT-D18)", () => {
  it("gt18w1 GET /s/<token> counts one scan and serves exactly the signed elements", async () => {
    const db = fakeDb();
    const res = await handleScan(get(pageUrl()), envFor(db), () => NOW, NEW_ID);
    assert.equal(res.status, 200);
    assert.match(res.headers.get("Content-Type"), /text\/html/);
    assert.equal(res.headers.get("Cache-Control"), "no-store");
    const body = await res.text();
    assert.ok(body.includes(`<h1>${FARM}</h1>`));
    assert.ok(body.includes("Dun peas, Red arrow radish"));
    assert.ok(body.includes(cadenceSentence(9)), "longest growth_days wins");
    assert.ok(!body.includes(cadenceSentence(7)));
    assert.ok(body.includes(`>${QUANTITY_LABEL}<`));
    assert.ok(body.includes('value="1"'));
    assert.ok(body.includes(`>${BUTTON}<`));
    assert.ok(!/https?:\/\//.test(body), "no external URL or asset on the page");
    assert.equal(db.scans.length, 1, "a scan is a scan");
    assert.equal(db.requests.length, 0, "GET never writes a candidate");
    assert.ok(db.sql.every((s) => !/standing_requests/i.test(s)));
  });

  it("gt18w2 malformed or missing token → not-ours page, nothing counted, nothing written", async () => {
    for (const bad of ["/s/abc", "/s/" + TOKEN.toUpperCase(), "/s/", "/s/" + TOKEN + "x"]) {
      const db = fakeDb();
      const res = await handleScan(get(`${ORIGIN}${bad}?v=Kale&g=9`), envFor(db), () => NOW, NEW_ID);
      assert.equal(res.status, 404, bad);
      const body = await res.text();
      assert.ok(body.includes(NOT_OURS), bad);
      assert.ok(!body.includes(BUTTON), bad);
      assert.equal(db.scans.length, 0, bad);
      assert.equal(db.requests.length, 0, bad);
    }
  });

  it("gt18w3 a link without its context (v or g) is not ours", async () => {
    for (const q of ["", "?v=Kale", "?g=9", "?v=&g=9", "?v=Kale&g=0", "?v=Kale&g=abc", "?v=Kale&g=400"]) {
      const db = fakeDb();
      const res = await handleScan(get(pageUrl(TOKEN, q)), envFor(db), () => NOW, NEW_ID);
      assert.equal(res.status, 404, q);
      assert.ok((await res.text()).includes(NOT_OURS), q);
      assert.equal(db.scans.length, 0, q);
    }
  });

  it("gt18w4 FARM_NAME unset → page and door refuse with the not-ready sentence, nothing written", async () => {
    for (const farm of [undefined, "", "   "]) {
      const db = fakeDb();
      const page = await handleScan(get(pageUrl()), envFor(db, { FARM_NAME: farm }), () => NOW, NEW_ID);
      assert.equal(page.status, 503);
      assert.ok((await page.text()).includes(NOT_READY));
      const door = await handleScan(post(pageUrl(TOKEN, ""), { bagsPerCycle: 2 }), envFor(db, { FARM_NAME: farm }), () => NOW, NEW_ID);
      assert.equal(door.status, 503);
      assert.equal((await door.json()).error, NOT_READY);
      assert.equal(db.scans.length, 0);
      assert.equal(db.requests.length, 0);
    }
  });

  it("gt18w5 POST /s/<token> stores exactly the candidate and answers the success sentence", async () => {
    const db = fakeDb();
    const res = await handleScan(post(pageUrl(TOKEN, ""), { bagsPerCycle: 2 }), envFor(db), () => NOW, NEW_ID);
    assert.equal(res.status, 200);
    const body = await res.json();
    assert.equal(body.requestId, "req-fixture-1");
    assert.equal(body.message, successSentence(FARM));
    assert.deepEqual(db.requests, [{ seq: 1, request_id: "req-fixture-1", token: TOKEN, bags_per_cycle: 2, requested_at: NOW }]);
    assert.equal(db.scans.length, 0, "the door never counts a scan");
    // Twice is two candidates (GT-D17: several per token).
    await handleScan(post(pageUrl(TOKEN, ""), { bagsPerCycle: 3 }), envFor(db), () => NOW, () => "req-fixture-2");
    assert.equal(db.requests.length, 2);
  });

  it("gt18w6 the door refuses anything but a whole quantity ≥ 1", async () => {
    const cases = [
      { bagsPerCycle: 0 }, { bagsPerCycle: 1.5 }, { bagsPerCycle: "2" }, { bagsPerCycle: -1 },
      { bagsPerCycle: 1000 }, {}, { bags: 2 },
    ];
    for (const c of cases) {
      const db = fakeDb();
      const res = await handleScan(post(pageUrl(TOKEN, ""), c), envFor(db), () => NOW, NEW_ID);
      assert.equal(res.status, 400, JSON.stringify(c));
      assert.equal((await res.json()).error, BAD_QUANTITY);
      assert.equal(db.requests.length, 0);
    }
    for (const raw of ["not json", "", "x".repeat(2000)]) {
      const db = fakeDb();
      const res = await handleScan(post(pageUrl(TOKEN, ""), raw, true), envFor(db), () => NOW, NEW_ID);
      assert.equal(res.status, 400);
      assert.equal((await res.json()).error, BAD_QUANTITY);
      assert.equal(db.requests.length, 0);
    }
  });

  it("gt18w7 the door refuses a bad token; POST elsewhere is 405; legacy /s still redirects", async () => {
    const db = fakeDb();
    const bad = await handleScan(post(`${ORIGIN}/s/abc`, { bagsPerCycle: 1 }), envFor(db), () => NOW, NEW_ID);
    assert.equal(bad.status, 404);
    assert.equal((await bad.json()).error, NOT_OURS);
    const other = await handleScan(post(`${ORIGIN}/scans`, { bagsPerCycle: 1 }), envFor(db), () => NOW, NEW_ID);
    assert.equal(other.status, 405);
    const legacy = await handleScan(get(`${ORIGIN}/s`), envFor(db), () => NOW, NEW_ID);
    assert.equal(legacy.status, 302);
    assert.equal(legacy.headers.get("Location"), "https://example.com/shop");
    assert.equal(db.requests.length, 0);
    assert.equal(db.scans.length, 1);
  });

  it("gt18w8 a failed insert answers not-ready and never claims success", async () => {
    const db = fakeDb({ failInsert: true });
    const res = await handleScan(post(pageUrl(TOKEN, ""), { bagsPerCycle: 2 }), envFor(db), () => NOW, NEW_ID);
    assert.equal(res.status, 503);
    assert.equal((await res.json()).error, NOT_READY);
    assert.equal(db.requests.length, 0);
    // The page still serves when only the scan insert fails (count is a floor).
    const page = await handleScan(get(pageUrl()), envFor(db), () => NOW, NEW_ID);
    assert.equal(page.status, 200);
  });

  it("gt18w9 the page escapes what the link carries", async () => {
    const db = fakeDb();
    const res = await handleScan(get(pageUrl(TOKEN, "?v=%3Cscript%3Ex%3C%2Fscript%3E&g=9")), envFor(db), () => NOW, NEW_ID);
    assert.equal(res.status, 200);
    const body = await res.text();
    assert.ok(!body.includes("<script>x</script>"));
    assert.ok(body.includes("&lt;script&gt;x&lt;/script&gt;"));
  });

function pullReq(after, token) {
  const headers = token ? { "X-Pull-Token": token } : {};
  return new Request(`${ORIGIN}/standing-requests?after=${after}`, { method: "GET", headers });
}
async function seed(db, n) {
  for (let i = 1; i <= n; i += 1) {
    await handleScan(post(pageUrl(TOKEN, ""), { bagsPerCycle: i }), envFor(db), () => NOW, () => `req-${i}`);
  }
}

  it("gt18w10 GET /standing-requests without or with a wrong token → 401, no SQL", async () => {
    for (const t of [undefined, "wrong"]) {
      const db = fakeDb();
      const res = await handleScan(pullReq(0, t), envFor(db), () => NOW, NEW_ID);
      assert.equal(res.status, 401);
      assert.equal((await res.json()).error, "Unauthorized.");
      assert.equal(db.sql.length, 0);
    }
    const db = fakeDb();
    const bad = await handleScan(pullReq("x", "pull_fixture"), envFor(db), () => NOW, NEW_ID);
    assert.equal(bad.status, 400);
  });

  it("gt18w11 the pull serves rows after the cursor, camelCase, SELECT-only, bounds truthful", async () => {
    const db = fakeDb();
    await seed(db, 3);
    db.sql.length = 0;
    const res = await handleScan(pullReq(1, "pull_fixture"), envFor(db), () => NOW, NEW_ID);
    assert.equal(res.status, 200);
    const body = await res.json();
    assert.deepEqual(body.rows.map((r) => r.seq), [2, 3]);
    assert.deepEqual(body.rows[0], { seq: 2, requestId: "req-2", token: TOKEN, bagsPerCycle: 2, requestedAt: NOW });
    assert.equal(body.firstAvailableSeq, 1);
    assert.equal(body.maxSeq, 3);
    assert.equal(body.servedAt, NOW);
    assert.ok(db.sql.length > 0);
    assert.ok(db.sql.every((s) => /^\s*SELECT/i.test(s)), "pull issued a write");
    const past = await handleScan(pullReq(99, "pull_fixture"), envFor(db), () => NOW, NEW_ID);
    const pastBody = await past.json();
    assert.deepEqual(pastBody.rows, []);
    assert.equal(pastBody.maxSeq, 3);
    const empty = await handleScan(pullReq(0, "pull_fixture"), envFor(fakeDb()), () => NOW, NEW_ID);
    const emptyBody = await empty.json();
    assert.deepEqual(emptyBody.rows, []);
    assert.equal(emptyBody.firstAvailableSeq, null);
    assert.equal(emptyBody.maxSeq, 0);
  });

  it("gt18w12 the pull pages at 200 rows", async () => {
    const db = fakeDb();
    await seed(db, 205);
    const first = await (await handleScan(pullReq(0, "pull_fixture"), envFor(db), () => NOW, NEW_ID)).json();
    assert.equal(first.rows.length, 200);
    assert.equal(first.rows[199].seq, 200);
    assert.equal(first.maxSeq, 205);
    const second = await (await handleScan(pullReq(200, "pull_fixture"), envFor(db), () => NOW, NEW_ID)).json();
    assert.equal(second.rows.length, 5);
    assert.equal(second.rows[4].seq, 205);
  });
});
