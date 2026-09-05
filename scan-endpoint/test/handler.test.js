import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { handleScan } from "../src/handler.js";

const NOW = "2026-08-15T18:00:00.000Z";
const TOKEN = "pull_token_test_fixture";
const DESTINATION = "https://example.com/shop";

function fakeDb({ rows = [], failInsert = false } = {}) {
  const sql = [];
  let nextId = rows.length ? Math.max(...rows) + 1 : 1;
  const db = {
    sql,
    rows: [...rows],
    prepare(text) {
      sql.push(text);
      let bound = [];
      const stmt = {
        bind(...args) {
          bound = args;
          return stmt;
        },
        async run() {
          if (failInsert) throw new Error("D1 unavailable");
          db.rows.push(nextId);
          nextId += 1;
          return { success: true };
        },
        async first() {
          if (text.includes("MIN(scan_id)")) {
            return db.rows.length
              ? { first_id: Math.min(...db.rows), max_id: Math.max(...db.rows) }
              : { first_id: null, max_id: null };
          }
          const after = Number(bound[0] ?? 0);
          return { n: db.rows.filter((id) => id > after).length };
        },
      };
      return stmt;
    },
  };
  return db;
}

function get(path, { token, origin = "https://scans.example" } = {}) {
  const headers = token ? { "X-Pull-Token": token } : {};
  return new Request(`${origin}${path}`, { method: "GET", headers });
}

function envFor(db, extras = {}) {
  return {
    REDIRECT_URL: DESTINATION,
    PULL_TOKEN: TOKEN,
    DB: db,
    ...extras,
  };
}

describe("scan handler", () => {
  it("p8w1 GET /s inserts one row and 302s to REDIRECT_URL", async () => {
    const db = fakeDb();
    const res = await handleScan(get("/s"), envFor(db), () => NOW);
    assert.equal(res.status, 302);
    assert.equal(res.headers.get("Location"), DESTINATION);
    assert.equal(res.headers.get("Cache-Control"), "no-store");
    assert.equal(db.rows.length, 1);
    assert.ok(db.sql.some((s) => /INSERT/i.test(s)));
  });

  it("p8w2 GET /s still 302s when the insert throws, and the row count is unchanged", async () => {
    const db = fakeDb({ failInsert: true });
    const before = db.rows.length;
    const res = await handleScan(get("/s"), envFor(db), () => NOW);
    assert.equal(res.status, 302);
    assert.equal(res.headers.get("Location"), DESTINATION);
    assert.equal(db.rows.length, before);
    assert.equal(db.rows.length, 0);
  });

  it("p8w3 GET /s with no REDIRECT_URL returns 503 and inserts nothing", async () => {
    const db = fakeDb();
    const res = await handleScan(
      get("/s"),
      envFor(db, { REDIRECT_URL: "" }),
      () => NOW,
    );
    assert.equal(res.status, 503);
    const body = await res.json();
    assert.equal(body.error, "Scan endpoint is not configured.");
    assert.equal(db.rows.length, 0);
    assert.equal(db.sql.length, 0);
  });

  it("p8w4 POST /s returns 405", async () => {
    const db = fakeDb();
    const res = await handleScan(
      new Request("https://scans.example/s", { method: "POST" }),
      envFor(db),
      () => NOW,
    );
    assert.equal(res.status, 405);
    const body = await res.json();
    assert.equal(body.error, "Method not allowed.");
    assert.equal(db.rows.length, 0);
    assert.equal(db.sql.length, 0);
  });

  it("p8w5 an unknown path returns 404", async () => {
    const db = fakeDb();
    const res = await handleScan(get("/no-such-path"), envFor(db), () => NOW);
    assert.equal(res.status, 404);
    const body = await res.json();
    assert.equal(body.error, "Not found.");
    assert.equal(db.sql.length, 0);
  });

  it("p8w6 GET /scans with no token returns 401", async () => {
    const db = fakeDb({ rows: [1, 2, 3] });
    const res = await handleScan(get("/scans"), envFor(db), () => NOW);
    assert.equal(res.status, 401);
    const body = await res.json();
    assert.equal(body.error, "Unauthorized.");
    assert.equal(body.newCount, undefined);
    assert.equal(db.sql.length, 0);
  });

  it("p8w7 GET /scans with a wrong token returns 401", async () => {
    const db = fakeDb({ rows: [1, 2, 3] });
    const res = await handleScan(
      get("/scans", { token: "wrong_token" }),
      envFor(db),
      () => NOW,
    );
    assert.equal(res.status, 401);
    const body = await res.json();
    assert.equal(body.error, "Unauthorized.");
    assert.equal(body.newCount, undefined);
    assert.equal(db.sql.length, 0);
  });

  it("p8w8 GET /scans with the token returns newCount counting only scan_id > after", async () => {
    const db = fakeDb({ rows: [1, 2, 3, 4, 5] });
    const res = await handleScan(
      get("/scans?after=2", { token: TOKEN }),
      envFor(db),
      () => NOW,
    );
    assert.equal(res.status, 200);
    const body = await res.json();
    assert.equal(body.newCount, 3);
    assert.equal(body.servedAt, NOW);
    assert.equal(db.rows.length, 5);
  });

  it("p8w9 GET /scans reports firstAvailableId and maxScanId, and after > maxScanId returns newCount 0 with maxScanId still truthful", async () => {
    const db = fakeDb({ rows: [1, 2, 3, 4, 5] });
    const env = envFor(db);

    const all = await handleScan(
      get("/scans?after=0", { token: TOKEN }),
      env,
      () => NOW,
    );
    assert.equal(all.status, 200);
    const allBody = await all.json();
    assert.equal(allBody.firstAvailableId, 1);
    assert.equal(allBody.maxScanId, 5);
    assert.equal(allBody.newCount, 5);

    const past = await handleScan(
      get("/scans?after=99", { token: TOKEN }),
      env,
      () => NOW,
    );
    assert.equal(past.status, 200);
    const pastBody = await past.json();
    assert.equal(pastBody.newCount, 0);
    assert.equal(pastBody.maxScanId, 5);
    assert.equal(pastBody.firstAvailableId, 1);
  });

  it("p8w10 GET /scans issues no INSERT, UPDATE or DELETE", async () => {
    const db = fakeDb({ rows: [1, 2, 3] });
    const res = await handleScan(
      get("/scans?after=0", { token: TOKEN }),
      envFor(db),
      () => NOW,
    );
    assert.equal(res.status, 200);
    const body = await res.json();
    assert.equal(typeof body.newCount, "number");
    assert.ok(db.sql.length > 0);
    assert.ok(db.sql.every((s) => /SELECT/i.test(s)));
    for (const text of db.sql) {
      assert.equal(
        /\b(INSERT|UPDATE|DELETE)\b/i.test(text),
        false,
        `pull issued a write: ${text}`,
      );
    }
  });
});
