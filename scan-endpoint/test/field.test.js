import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  handleScan, NOT_OURS, NOT_READY,
  FIELD_LINE, FIELD_LABEL_CROP, FIELD_VERB_MOVE, FIELD_VERB_HARVEST,
  FIELD_LABEL_TRAYS, FIELD_LABEL_OZ, FIELD_LABEL_NOTE, FIELD_BUTTON,
  FIELD_HINT, FIELD_REFUSED, FIELD_NO_CROPS, FIELD_SEND_QUEUED, FIELD_NETWORK, FIELD_STATUS_SENT,
} from "../src/handler.js";

const NOW = "2026-08-17T18:00:00.000Z";
const TOKEN = "0123456789abcdef0123456789abcdef";
const FARM = "Test Farm";
const ORIGIN = "https://scans.example";
const UUID = "11111111-2222-4333-8444-555555555555";
const UUID_H = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

function fakeDb() {
  const sql = [];
  const scans = [];
  const proposals = [];
  let nextScan = 1;
  let nextSeq = 1;
  const db = {
    sql, scans, proposals,
    prepare(text) {
      sql.push(text);
      let bound = [];
      const stmt = {
        bind(...args) { bound = args; return stmt; },
        async run() {
          if (/INSERT INTO scans/i.test(text)) { scans.push(nextScan++); return { success: true }; }
          if (/INSERT OR IGNORE INTO field_proposals/i.test(text)) {
            const proposal_id = bound[0];
            if (!proposals.some((r) => r.proposal_id === proposal_id)) {
              proposals.push({
                seq: nextSeq++,
                proposal_id,
                device_token: bound[1],
                verb: bound[2],
                crop_id: bound[3],
                quantity: bound[4],
                actual_yield_oz: bound[5],
                phone_captured_at: bound[6],
                note: bound[7],
                received_at: bound[8],
              });
            }
            return { success: true };
          }
          throw new Error("unexpected write: " + text);
        },
        async first() {
          if (/MIN\(seq\) AS first_seq, MAX\(seq\) AS max_seq FROM field_proposals/i.test(text)) {
            return proposals.length
              ? { first_seq: Math.min(...proposals.map((r) => r.seq)), max_seq: Math.max(...proposals.map((r) => r.seq)) }
              : { first_seq: null, max_seq: null };
          }
          throw new Error("unexpected read: " + text);
        },
        async all() {
          if (/FROM field_proposals WHERE seq > \?1/i.test(text)) {
            const after = Number(bound[0] ?? 0);
            const limit = Number(bound[1] ?? 200);
            const results = proposals.filter((r) => r.seq > after).sort((a, b) => a.seq - b.seq).slice(0, limit);
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
function fieldUrl(token = TOKEN, query = "") {
  return `${ORIGIN}/a/${token}${query}`;
}
function get(url) { return new Request(url, { method: "GET" }); }
function post(url, body, raw = false) {
  return new Request(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: raw ? body : JSON.stringify(body),
  });
}
function pullReq(after, token) {
  const headers = token ? { "X-Pull-Token": token } : {};
  return new Request(`${ORIGIN}/field-proposals?after=${after}`, { method: "GET", headers });
}

describe("field terminal (GT-D21)", () => {
  it("gt21w1 GET /a/<TOKEN> serves the signed field page and records no scan", async () => {
    const db = fakeDb();
    const res = await handleScan(get(fieldUrl(TOKEN, "?c=kale:Kale&c=radish:Red%20arrow%20radish")), envFor(db), () => NOW);
    assert.equal(res.status, 200);
    assert.match(res.headers.get("Content-Type"), /text\/html/);
    assert.equal(res.headers.get("Cache-Control"), "no-store");
    const body = await res.text();
    assert.ok(body.includes(`<h1>${FARM}</h1>`));
    assert.ok(body.includes(FIELD_LINE));
    assert.ok(body.includes(FIELD_LABEL_CROP));
    assert.ok(body.includes(FIELD_VERB_MOVE));
    assert.ok(body.includes(FIELD_VERB_HARVEST));
    assert.ok(body.includes(FIELD_LABEL_TRAYS));
    assert.ok(body.includes('aria-label="Fewer trays"'));
    assert.ok(body.includes('aria-label="More trays"'));
    assert.ok(body.includes(FIELD_LABEL_OZ));
    assert.ok(body.includes(FIELD_LABEL_NOTE));
    assert.ok(body.includes(`>${FIELD_BUTTON}<`));
    assert.ok(body.includes(FIELD_HINT));
    assert.ok(body.includes('"/a/sw.js"'));
    assert.ok(body.includes("Kale"));
    assert.ok(body.includes("Red arrow radish"));
    assert.ok(body.includes('id="sendq"'));
    assert.ok(body.includes(`>${FIELD_SEND_QUEUED}<`));
    assert.ok(/id="sendq"[^>]*\bdisabled\b/.test(body), "Send queued starts disabled");
    assert.ok(body.includes(JSON.stringify(FIELD_NETWORK)));
    assert.ok(!/https?:\/\//.test(body), "no external URL or branding on the page");
    assert.equal(db.scans.length, 0);
  });

  it("gt21w2 GET /a/<TOKEN> with no c includes FIELD_NO_CROPS; malformed c values are dropped", async () => {
    const db = fakeDb();
    const res = await handleScan(get(fieldUrl()), envFor(db), () => NOW);
    assert.equal(res.status, 200);
    const body = await res.text();
    assert.ok(body.includes(FIELD_NO_CROPS));
    const drop = await handleScan(get(fieldUrl(TOKEN, "?c=:x&c=bad%20id!:Name")), envFor(db), () => NOW);
    assert.equal(drop.status, 200);
    const dropBody = await drop.text();
    assert.ok(dropBody.includes(FIELD_NO_CROPS));
    assert.ok(!dropBody.includes(":x") || dropBody.includes(FIELD_NO_CROPS));
  });

  it("gt21w3 GET /a/notatoken → 404 NOT_OURS; no FARM_NAME → 503 NOT_READY for GET and POST", async () => {
    const db = fakeDb();
    const bad = await handleScan(get(`${ORIGIN}/a/notatoken`), envFor(db), () => NOW);
    assert.equal(bad.status, 404);
    assert.ok((await bad.text()).includes(NOT_OURS));
    for (const farm of [undefined, "", "   "]) {
      const d = fakeDb();
      const page = await handleScan(get(fieldUrl()), envFor(d, { FARM_NAME: farm }), () => NOW);
      assert.equal(page.status, 503);
      assert.ok((await page.text()).includes(NOT_READY));
      const door = await handleScan(post(fieldUrl(), {
        proposalId: UUID, verb: "move_to_light", cropId: "kale", quantity: 1,
        actualYieldOz: null, phoneCapturedAt: NOW, note: null,
      }), envFor(d, { FARM_NAME: farm }), () => NOW);
      assert.equal(door.status, 503);
      assert.equal((await door.json()).error, NOT_READY);
    }
  });

  it("gt21w4 POST valid move stores the row; same proposalId is idempotent; harvest stores oz", async () => {
    const db = fakeDb();
    const move = {
      proposalId: UUID, verb: "move_to_light", cropId: "kale", quantity: 3,
      actualYieldOz: null, phoneCapturedAt: "2026-08-17T18:00:00.000Z", note: "back rack",
    };
    const res = await handleScan(post(fieldUrl(), move), envFor(db), () => NOW);
    assert.equal(res.status, 200);
    assert.deepEqual(await res.json(), { proposalId: UUID });
    assert.equal(db.proposals.length, 1);
    assert.deepEqual(db.proposals[0], {
      seq: 1,
      proposal_id: UUID,
      device_token: TOKEN,
      verb: "move_to_light",
      crop_id: "kale",
      quantity: 3,
      actual_yield_oz: null,
      phone_captured_at: "2026-08-17T18:00:00.000Z",
      note: "back rack",
      received_at: NOW,
    });
    assert.ok(!Object.values(db.proposals[0]).some((v) => typeof v === "string" && /kale/i.test(v) && v !== "kale"));
    const again = await handleScan(post(fieldUrl(), move), envFor(db), () => NOW);
    assert.equal(again.status, 200);
    assert.equal(db.proposals.length, 1);
    const harvest = await handleScan(post(fieldUrl(), {
      proposalId: UUID_H, verb: "harvest", cropId: "kale", quantity: 1,
      actualYieldOz: 12.5, phoneCapturedAt: "2026-08-17T18:00:00.000Z", note: null,
    }), envFor(db), () => NOW);
    assert.equal(harvest.status, 200);
    assert.equal(db.proposals.length, 2);
    assert.equal(db.proposals[1].actual_yield_oz, 12.5);
    assert.equal(db.proposals[1].verb, "harvest");
  });

  it("gt21w5 refusals → 400 FIELD_REFUSED and no row", async () => {
    const cases = [
      { proposalId: "not-a-uuid", verb: "move_to_light", cropId: "kale", quantity: 1, actualYieldOz: null, phoneCapturedAt: NOW, note: null },
      { proposalId: UUID, verb: "sow", cropId: "kale", quantity: 1, actualYieldOz: null, phoneCapturedAt: NOW, note: null },
      { proposalId: UUID, verb: "move_to_light", cropId: "kale", quantity: 0, actualYieldOz: null, phoneCapturedAt: NOW, note: null },
      { proposalId: UUID, verb: "move_to_light", cropId: "kale", quantity: 1000, actualYieldOz: null, phoneCapturedAt: NOW, note: null },
      { proposalId: UUID, verb: "move_to_light", cropId: "kale", quantity: 2.5, actualYieldOz: null, phoneCapturedAt: NOW, note: null },
      { proposalId: UUID, verb: "move_to_light", cropId: "kale", quantity: 1, actualYieldOz: 1, phoneCapturedAt: NOW, note: null },
      { proposalId: UUID, verb: "harvest", cropId: "kale", quantity: 1, actualYieldOz: null, phoneCapturedAt: NOW, note: null },
      { proposalId: UUID, verb: "harvest", cropId: "kale", quantity: 1, actualYieldOz: 0, phoneCapturedAt: NOW, note: null },
      { proposalId: UUID, verb: "move_to_light", cropId: "kale", quantity: 1, actualYieldOz: null, phoneCapturedAt: NOW, note: "x".repeat(201) },
    ];
    for (const c of cases) {
      const db = fakeDb();
      const res = await handleScan(post(fieldUrl(), c), envFor(db), () => NOW);
      assert.equal(res.status, 400, JSON.stringify(c));
      assert.equal((await res.json()).error, FIELD_REFUSED);
      assert.equal(db.proposals.length, 0);
    }
    for (const raw of ["not json", "x".repeat(1025)]) {
      const db = fakeDb();
      const res = await handleScan(post(fieldUrl(), raw, true), envFor(db), () => NOW);
      assert.equal(res.status, 400);
      assert.equal((await res.json()).error, FIELD_REFUSED);
      assert.equal(db.proposals.length, 0);
    }
  });

  it("gt21w6 GET /field-proposals is pull-token gated, camelCase, cursor-ordered", async () => {
    const db = fakeDb();
    await handleScan(post(fieldUrl(), {
      proposalId: UUID, verb: "move_to_light", cropId: "kale", quantity: 3,
      actualYieldOz: null, phoneCapturedAt: NOW, note: "back rack",
    }), envFor(db), () => NOW);
    await handleScan(post(fieldUrl(), {
      proposalId: UUID_H, verb: "harvest", cropId: "kale", quantity: 1,
      actualYieldOz: 12.5, phoneCapturedAt: NOW, note: null,
    }), envFor(db), () => NOW);
    const noHeader = await handleScan(pullReq(0), envFor(db), () => NOW);
    assert.equal(noHeader.status, 401);
    const res = await handleScan(pullReq(0, "pull_fixture"), envFor(db), () => NOW);
    assert.equal(res.status, 200);
    const body = await res.json();
    assert.deepEqual(body.rows.map((r) => r.seq), [1, 2]);
    assert.deepEqual(body.rows[0], {
      seq: 1, proposalId: UUID, deviceToken: TOKEN, verb: "move_to_light", cropId: "kale",
      quantity: 3, actualYieldOz: null, phoneCapturedAt: NOW, note: "back rack", receivedAt: NOW,
    });
    assert.equal(body.firstAvailableSeq, 1);
    assert.equal(body.maxSeq, 2);
    assert.equal(body.servedAt, NOW);
    const after = await handleScan(pullReq(1, "pull_fixture"), envFor(db), () => NOW);
    const afterBody = await after.json();
    assert.deepEqual(afterBody.rows.map((r) => r.seq), [2]);
    const bad = await handleScan(pullReq("abc", "pull_fixture"), envFor(db), () => NOW);
    assert.equal(bad.status, 400);
  });

  it("gt21w7 GET /a/sw.js → 200 application/javascript, gt-field-v1, no external URL", async () => {
    const db = fakeDb();
    const res = await handleScan(get(`${ORIGIN}/a/sw.js`), envFor(db), () => NOW);
    assert.equal(res.status, 200);
    assert.match(res.headers.get("Content-Type"), /application\/javascript/);
    const body = await res.text();
    assert.ok(body.includes("gt-field-v1"));
    assert.ok(!/https?:\/\//.test(body));
  });

  it("gt21w8 Send queued and network sentences are the signed bytes", () => {
    assert.equal(FIELD_SEND_QUEUED, "Send queued");
    assert.equal(FIELD_NETWORK, "Could not reach the relay. Try again when you have a signal.");
    assert.equal(FIELD_STATUS_SENT, "Sent to the relay");
    assert.equal(FIELD_HINT, "Queued captures send when the phone is online. Sent means the relay has it; the PC decides at the desk.");
  });
});
