import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  ALLOW_LIVE_KEYS,
  handleCheckout,
  validateStripeKey,
} from "../src/handler.js";

const ORIGIN = "https://shop.example.com";
const RESTRICTED_PREFIX = ["r", "k", "_"].join("");
const SECRET_PREFIX = ["s", "k", "_"].join("");
const KEY = `${RESTRICTED_PREFIX}test_unit_fixture_not_a_real_key`;
const LIVE_KEY = `${RESTRICTED_PREFIX}live_should_be_refused`;
const SECRET_KEY = `${SECRET_PREFIX}test_secret`;

const baseEnv = {
  STRIPE_RESTRICTED_KEY: KEY,
  ALLOWED_ORIGIN: ORIGIN,
  SUCCESS_URL: "https://shop.example.com/?paid=1",
  CANCEL_URL: "https://shop.example.com/?cancelled=1",
};

function cart(overrides = {}) {
  return {
    reference: "cart_ref_9f1c2a3b",
    harvestDate: "2026-08-14",
    currency: "cad",
    total: 4500,
    lines: [
      { priceId: "price_peas", quantity: 3 },
      { priceId: "price_sun", quantity: 2 },
    ],
    ...overrides,
  };
}

function postRequest(body, { origin = ORIGIN, method = "POST" } = {}) {
  return new Request("https://checkout.example.com/", {
    method,
    headers: {
      Origin: origin,
      "Content-Type": "application/json",
    },
    body: typeof body === "string" ? body : JSON.stringify(body),
  });
}

/** @returns {{ fetchImpl: typeof fetch, calls: object[] }} */
function fakeStripe({
  prices = {
    price_peas: { id: "price_peas", active: true, currency: "cad", unit_amount: 1000 },
    price_sun: { id: "price_sun", active: true, currency: "cad", unit_amount: 750 },
  },
  sessionUrl = "https://checkout.stripe.com/c/pay/cs_test_abc",
} = {}) {
  const calls = [];
  const fetchImpl = async (url, init = {}) => {
    const u = String(url);
    const method = (init.method || "GET").toUpperCase();
    const headers = init.headers || {};
    calls.push({ url: u, method, headers, body: init.body || null });

    if (u.includes("/v1/prices/")) {
      const id = decodeURIComponent(u.split("/v1/prices/")[1]);
      const price = prices[id];
      if (!price) {
        return new Response(JSON.stringify({ error: { message: "missing" } }), {
          status: 404,
        });
      }
      return new Response(JSON.stringify(price), { status: 200 });
    }

    if (u.includes("/v1/checkout/sessions") && method === "POST") {
      return new Response(JSON.stringify({ id: "cs_test_abc", url: sessionUrl }), {
        status: 200,
      });
    }

    return new Response("not found", { status: 404 });
  };
  return { fetchImpl, calls };
}

function sessionCalls(calls) {
  return calls.filter(
    (c) => c.url.includes("/v1/checkout/sessions") && c.method === "POST",
  );
}

function stripeCalls(calls) {
  return calls.filter((c) => c.url.includes("api.stripe.com"));
}

function assertNoKeyLeak(responseText, calls, logs) {
  assert.ok(!responseText.includes(KEY), "response must not contain the key");
  assert.ok(
    !responseText.includes(RESTRICTED_PREFIX + "test"),
    "response must not echo key prefix",
  );
  for (const line of logs) {
    assert.ok(!line.includes(KEY), `log leaked key: ${line}`);
  }
  // Auth header may carry the key to Stripe — that is not a response/log leak.
  for (const c of calls) {
    const body = c.body == null ? "" : String(c.body);
    assert.ok(!body.includes(KEY), "Stripe request body must not contain the key");
  }
}

describe("checkout handler", () => {
  it("valid two-line cart creates one Session and returns {url} only", async () => {
    const { fetchImpl, calls } = fakeStripe();
    const logs = [];
    const orig = console.log;
    console.log = (...a) => logs.push(a.join(" "));
    try {
      const res = await handleCheckout(postRequest(cart()), baseEnv, fetchImpl);
      assert.equal(res.status, 200);
      const body = await res.json();
      assert.deepEqual(Object.keys(body).sort(), ["url"]);
      assert.equal(body.url, "https://checkout.stripe.com/c/pay/cs_test_abc");
      assert.equal(sessionCalls(calls).length, 1);
      const session = sessionCalls(calls)[0];
      assert.match(session.body, /line_items%5B0%5D%5Bprice%5D=price_peas/);
      assert.match(session.body, /client_reference_id=cart_ref_9f1c2a3b/);
      assertNoKeyLeak(JSON.stringify(body), calls, logs);
    } finally {
      console.log = orig;
    }
  });

  it("total disagreeing with summed prices → 409, no Session", async () => {
    const { fetchImpl, calls } = fakeStripe();
    const res = await handleCheckout(
      postRequest(cart({ total: 9999 })),
      baseEnv,
      fetchImpl,
    );
    assert.equal(res.status, 409);
    const body = await res.json();
    assert.equal(
      body.error,
      "The prices on this page are out of date. Reload and try again.",
    );
    assert.equal(sessionCalls(calls).length, 0);
    assert.ok(stripeCalls(calls).every((c) => c.url.includes("/v1/prices/")));
  });

  it("forged cheap total → 409, no Session", async () => {
    const { fetchImpl, calls } = fakeStripe();
    const res = await handleCheckout(
      postRequest(cart({ total: 1 })),
      baseEnv,
      fetchImpl,
    );
    assert.equal(res.status, 409);
    assert.equal(sessionCalls(calls).length, 0);
  });

  it("bad quantities → 400, no Stripe calls", async () => {
    for (const quantity of [0, -1, 100, 1.5, "3"]) {
      const { fetchImpl, calls } = fakeStripe();
      const res = await handleCheckout(
        postRequest(
          cart({
            lines: [{ priceId: "price_peas", quantity }],
            total: 1000,
          }),
        ),
        baseEnv,
        fetchImpl,
      );
      assert.equal(res.status, 400, `quantity ${quantity}`);
      assert.equal(stripeCalls(calls).length, 0, `quantity ${quantity}`);
    }
  });

  it("21 lines, duplicate priceId, bad reference, bad harvestDate → 400", async () => {
    {
      const lines = Array.from({ length: 21 }, (_, i) => ({
        priceId: `price_${i}`,
        quantity: 1,
      }));
      const { fetchImpl, calls } = fakeStripe();
      const res = await handleCheckout(
        postRequest(cart({ lines, total: 21 })),
        baseEnv,
        fetchImpl,
      );
      assert.equal(res.status, 400);
      assert.equal(stripeCalls(calls).length, 0);
    }
    {
      const { fetchImpl, calls } = fakeStripe();
      const res = await handleCheckout(
        postRequest(
          cart({
            lines: [
              { priceId: "price_peas", quantity: 1 },
              { priceId: "price_peas", quantity: 2 },
            ],
            total: 3000,
          }),
        ),
        baseEnv,
        fetchImpl,
      );
      assert.equal(res.status, 400);
      assert.equal(stripeCalls(calls).length, 0);
    }
    {
      const { fetchImpl, calls } = fakeStripe();
      const res = await handleCheckout(
        postRequest(cart({ reference: "bad ref!" })),
        baseEnv,
        fetchImpl,
      );
      assert.equal(res.status, 400);
      assert.equal(stripeCalls(calls).length, 0);
    }
    {
      const { fetchImpl, calls } = fakeStripe();
      const res = await handleCheckout(
        postRequest(cart({ harvestDate: "08/14/2026" })),
        baseEnv,
        fetchImpl,
      );
      assert.equal(res.status, 400);
      assert.equal(stripeCalls(calls).length, 0);
    }
  });

  it("inactive or missing price → 400, no Session", async () => {
    {
      const { fetchImpl, calls } = fakeStripe({
        prices: {
          price_peas: {
            id: "price_peas",
            active: false,
            currency: "cad",
            unit_amount: 1000,
          },
          price_sun: {
            id: "price_sun",
            active: true,
            currency: "cad",
            unit_amount: 750,
          },
        },
      });
      const res = await handleCheckout(postRequest(cart()), baseEnv, fetchImpl);
      assert.equal(res.status, 400);
      assert.equal(sessionCalls(calls).length, 0);
    }
    {
      const { fetchImpl, calls } = fakeStripe({
        prices: {
          price_peas: {
            id: "price_peas",
            active: true,
            currency: "cad",
            unit_amount: 1000,
          },
        },
      });
      const res = await handleCheckout(postRequest(cart()), baseEnv, fetchImpl);
      assert.equal(res.status, 400);
      assert.equal(sessionCalls(calls).length, 0);
    }
  });

  it("live restricted key refused while ALLOW_LIVE_KEYS is false", async () => {
    assert.equal(ALLOW_LIVE_KEYS, false);
    const check = validateStripeKey(LIVE_KEY);
    assert.equal(check.ok, false);
    assert.match(check.reason, /Live keys are not accepted/);

    const { fetchImpl, calls } = fakeStripe();
    const res = await handleCheckout(
      postRequest(cart()),
      { ...baseEnv, STRIPE_RESTRICTED_KEY: LIVE_KEY },
      fetchImpl,
    );
    assert.equal(res.status, 503);
    assert.equal(stripeCalls(calls).length, 0);

    const sk = validateStripeKey(SECRET_KEY);
    assert.equal(sk.ok, false);
    assert.match(sk.reason, /secret key/i);
  });

  it("mismatched Origin rejected before any Stripe call", async () => {
    const { fetchImpl, calls } = fakeStripe();
    const res = await handleCheckout(
      postRequest(cart(), { origin: "https://evil.example" }),
      baseEnv,
      fetchImpl,
    );
    assert.equal(res.status, 403);
    assert.equal(stripeCalls(calls).length, 0);
    assert.equal(calls.length, 0);
  });

  it("allowed Origin is echoed back; any other Origin is not; never *", async () => {
    const { fetchImpl } = fakeStripe();
    const ok = await handleCheckout(postRequest(cart()), baseEnv, fetchImpl);
    assert.equal(ok.status, 200);
    assert.equal(ok.headers.get("Access-Control-Allow-Origin"), ORIGIN);
    assert.notEqual(ok.headers.get("Access-Control-Allow-Origin"), "*");

    const denied = await handleCheckout(
      postRequest(cart(), { origin: "https://evil-prairieroots.example" }),
      baseEnv,
      fetchImpl,
    );
    assert.equal(denied.status, 403);
    assert.equal(denied.headers.get("Access-Control-Allow-Origin"), null);

    const preflightOk = await handleCheckout(
      new Request("https://checkout.example.com/", {
        method: "OPTIONS",
        headers: { Origin: ORIGIN },
      }),
      baseEnv,
      fetchImpl,
    );
    assert.equal(preflightOk.status, 204);
    assert.equal(preflightOk.headers.get("Access-Control-Allow-Origin"), ORIGIN);

    const preflightDenied = await handleCheckout(
      new Request("https://checkout.example.com/", {
        method: "OPTIONS",
        headers: { Origin: "https://evil.example" },
      }),
      baseEnv,
      fetchImpl,
    );
    assert.equal(preflightDenied.status, 403);
    assert.equal(preflightDenied.headers.get("Access-Control-Allow-Origin"), null);
  });

  it("comma-separated ALLOWED_ORIGIN matches exactly and echoes the matched Origin", async () => {
    const local = "http://localhost:5500";
    const loopback = "http://127.0.0.1:5500";
    const env = {
      ...baseEnv,
      ALLOWED_ORIGIN: `${local},${loopback}`,
    };
    const { fetchImpl, calls } = fakeStripe();

    const fromLocal = await handleCheckout(
      postRequest(cart(), { origin: local }),
      env,
      fetchImpl,
    );
    assert.equal(fromLocal.status, 200);
    assert.equal(fromLocal.headers.get("Access-Control-Allow-Origin"), local);

    const fromLoopback = await handleCheckout(
      postRequest(cart(), { origin: loopback }),
      env,
      fetchImpl,
    );
    assert.equal(fromLoopback.status, 200);
    assert.equal(fromLoopback.headers.get("Access-Control-Allow-Origin"), loopback);

    // Prefix / substring must not match.
    const prefix = await handleCheckout(
      postRequest(cart(), { origin: "http://localhost:5500.evil.example" }),
      env,
      fetchImpl,
    );
    assert.equal(prefix.status, 403);
    assert.equal(prefix.headers.get("Access-Control-Allow-Origin"), null);

    const substring = await handleCheckout(
      postRequest(cart(), { origin: "https://evil-prairieroots.example" }),
      {
        ...baseEnv,
        ALLOWED_ORIGIN: "https://shop.prairieroots.example",
      },
      fetchImpl,
    );
    assert.equal(substring.status, 403);
    assert.equal(substring.headers.get("Access-Control-Allow-Origin"), null);
    assert.equal(sessionCalls(calls).length, 2); // only the two allowed POSTs create sessions
  });

  it("same reference twice sends the same Idempotency-Key", async () => {
    const { fetchImpl, calls } = fakeStripe();
    const body = cart({ reference: "same_ref_twice_001" });
    await handleCheckout(postRequest(body), baseEnv, fetchImpl);
    await handleCheckout(postRequest(body), baseEnv, fetchImpl);
    const sessions = sessionCalls(calls);
    assert.equal(sessions.length, 2);
    const k1 = sessions[0].headers["Idempotency-Key"];
    const k2 = sessions[1].headers["Idempotency-Key"];
    assert.equal(k1, "same_ref_twice_001");
    assert.equal(k2, k1);
  });

  it("no response body or log line contains the key", async () => {
    const { fetchImpl, calls } = fakeStripe();
    const logs = [];
    const orig = console.log;
    console.log = (...a) => logs.push(a.join(" "));
    try {
      const ok = await handleCheckout(postRequest(cart()), baseEnv, fetchImpl);
      const stale = await handleCheckout(
        postRequest(cart({ total: 1 })),
        baseEnv,
        fetchImpl,
      );
      const text = (await ok.text()) + (await stale.text()) + logs.join("\n");
      assertNoKeyLeak(text, calls, logs);
      assert.ok(!text.includes(KEY));
      assert.ok(!logs.some((l) => l.includes(KEY)));
    } finally {
      console.log = orig;
    }
  });
});
