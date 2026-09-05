/**
 * Pure checkout handler. Stores nothing. Never trusts a client-supplied amount.
 *
 * Line items are Stripe Price IDs + quantities; Stripe prices the cart.
 * The posted `total` is only used to detect a stale page (409 if it disagrees).
 */

// Live keys are refused. Flipping this is a deliberate code change, reviewed and
// redeployed — never an environment variable, never a request parameter.
export const ALLOW_LIVE_KEYS = false;

const MAX_BODY_BYTES = 8 * 1024;
const REF_RE = /^[A-Za-z0-9_-]{1,200}$/;
const DATE_RE = /^\d{4}-\d{2}-\d{2}$/;

// Built at runtime so the repo never contains credential-shaped literals.
const RESTRICTED_PREFIX = ["r", "k", "_"].join("");
const SECRET_PREFIX = ["s", "k", "_"].join("");
const LIVE_RESTRICTED_PREFIX = `${RESTRICTED_PREFIX}live_`;

/**
 * @param {Request} request
 * @param {Record<string, string>} env
 * @param {typeof fetch} fetchImpl
 * @returns {Promise<Response>}
 */
export async function handleCheckout(request, env, fetchImpl = fetch) {
  const origin = request.headers.get("Origin");
  const allowed = env.ALLOWED_ORIGIN || "";

  if (request.method === "OPTIONS") {
    if (!originOk(origin, allowed)) {
      return textResponse(403, "Origin not allowed.", origin, allowed);
    }
    return corsPreflight(origin, allowed);
  }

  if (!originOk(origin, allowed)) {
    return textResponse(403, "Origin not allowed.", origin, allowed);
  }

  if (request.method !== "POST") {
    return jsonResponse(405, { error: "Method not allowed." }, origin, allowed);
  }

  const keyCheck = validateStripeKey(env.STRIPE_RESTRICTED_KEY);
  if (!keyCheck.ok) {
    console.log("POST", 503, "key");
    return jsonResponse(503, { error: keyCheck.reason }, origin, allowed);
  }

  const successUrl = env.SUCCESS_URL;
  const cancelUrl = env.CANCEL_URL;
  if (!successUrl || !cancelUrl) {
    console.log("POST", 503, "urls");
    return jsonResponse(
      503,
      { error: "Checkout is not configured." },
      origin,
      allowed,
    );
  }

  const raw = await readBodyLimited(request, MAX_BODY_BYTES);
  if (!raw.ok) {
    console.log("POST", 400, "body");
    return jsonResponse(400, { error: raw.reason }, origin, allowed);
  }

  let body;
  try {
    body = JSON.parse(raw.text);
  } catch {
    console.log("POST", 400, "json");
    return jsonResponse(400, { error: "Body must be JSON." }, origin, allowed);
  }

  const validated = validateCart(body);
  if (!validated.ok) {
    console.log("POST", 400, truncRef(body && body.reference));
    return jsonResponse(400, { error: validated.reason }, origin, allowed);
  }

  const cart = validated.cart;
  const refLog = truncRef(cart.reference);

  try {
    const prices = [];
    for (const line of cart.lines) {
      const price = await fetchPrice(fetchImpl, keyCheck.key, line.priceId);
      if (!price.ok) {
        console.log("POST", 400, refLog);
        return jsonResponse(400, { error: price.reason }, origin, allowed);
      }
      if (!price.value.active) {
        console.log("POST", 400, refLog);
        return jsonResponse(
          400,
          { error: "One of the prices on this page is no longer for sale." },
          origin,
          allowed,
        );
      }
      const currency = (price.value.currency || "").toLowerCase();
      if (currency !== cart.currency) {
        console.log("POST", 400, refLog);
        return jsonResponse(
          400,
          { error: "A price on this page is in the wrong currency." },
          origin,
          allowed,
        );
      }
      if (
        typeof price.value.unit_amount !== "number" ||
        !Number.isInteger(price.value.unit_amount) ||
        price.value.unit_amount < 0
      ) {
        console.log("POST", 400, refLog);
        return jsonResponse(
          400,
          { error: "A price on this page could not be read." },
          origin,
          allowed,
        );
      }
      prices.push(price.value);
    }

    let computed = 0;
    for (let i = 0; i < cart.lines.length; i++) {
      computed += prices[i].unit_amount * cart.lines[i].quantity;
    }
    if (computed !== cart.total) {
      console.log("POST", 409, refLog);
      return jsonResponse(
        409,
        {
          error:
            "The prices on this page are out of date. Reload and try again.",
        },
        origin,
        allowed,
      );
    }

    const session = await createCheckoutSession(fetchImpl, keyCheck.key, {
      cart,
      successUrl,
      cancelUrl,
    });
    if (!session.ok) {
      console.log("POST", 502, refLog);
      return jsonResponse(
        502,
        { error: "Could not start checkout. Try again in a moment." },
        origin,
        allowed,
      );
    }

    console.log("POST", 200, refLog);
    return jsonResponse(200, { url: session.url }, origin, allowed);
  } catch (err) {
    // Never log the key or Stripe bodies.
    console.log("POST", 502, refLog);
    return jsonResponse(
      502,
      { error: "Could not start checkout. Try again in a moment." },
      origin,
      allowed,
    );
  }
}

/**
 * Validate the restricted key at the edge. Returns {ok, key} or {ok:false, reason}.
 * @param {string|undefined} key
 */
export function validateStripeKey(key) {
  if (typeof key !== "string" || key.length === 0) {
    return { ok: false, reason: "Checkout is not configured." };
  }
  if (key.startsWith(SECRET_PREFIX)) {
    return {
      ok: false,
      reason:
        "Checkout refused a secret key. Use a restricted key (starts with r-k-underscore).",
    };
  }
  if (!key.startsWith(RESTRICTED_PREFIX)) {
    return {
      ok: false,
      reason: "Checkout needs a restricted key (starts with r-k-underscore).",
    };
  }
  if (key.startsWith(LIVE_RESTRICTED_PREFIX) && !ALLOW_LIVE_KEYS) {
    return {
      ok: false,
      reason:
        "Live keys are not accepted. Use a test-mode restricted key.",
    };
  }
  return { ok: true, key };
}

/**
 * @param {unknown} body
 * @returns {{ok:true, cart:object}|{ok:false, reason:string}}
 */
export function validateCart(body) {
  if (!body || typeof body !== "object" || Array.isArray(body)) {
    return { ok: false, reason: "Body must be a JSON object." };
  }

  const reference = body.reference;
  if (typeof reference !== "string" || !REF_RE.test(reference)) {
    return {
      ok: false,
      reason:
        "Reference must be 1–200 characters of letters, numbers, underscore, or hyphen.",
    };
  }

  const harvestDate = body.harvestDate;
  if (typeof harvestDate !== "string" || !DATE_RE.test(harvestDate)) {
    return {
      ok: false,
      reason: "Harvest date must be YYYY-MM-DD.",
    };
  }

  const currency =
    typeof body.currency === "string" ? body.currency.toLowerCase() : "";
  if (!currency || currency.length < 3) {
    return { ok: false, reason: "Currency is required." };
  }

  const total = body.total;
  if (typeof total !== "number" || !Number.isInteger(total) || total < 1) {
    return { ok: false, reason: "Total must be a positive integer (cents)." };
  }

  const lines = body.lines;
  if (!Array.isArray(lines) || lines.length < 1 || lines.length > 20) {
    return { ok: false, reason: "Cart must have between 1 and 20 lines." };
  }

  const seen = new Set();
  const normalized = [];
  for (const line of lines) {
    if (!line || typeof line !== "object") {
      return { ok: false, reason: "Each line must be an object." };
    }
    const priceId = line.priceId;
    if (typeof priceId !== "string" || !priceId.startsWith("price_")) {
      return {
        ok: false,
        reason: "Every line needs a Stripe Price id starting with price_.",
      };
    }
    if (seen.has(priceId)) {
      return { ok: false, reason: "Duplicate price in the cart." };
    }
    seen.add(priceId);

    const quantity = line.quantity;
    if (
      typeof quantity !== "number" ||
      !Number.isInteger(quantity) ||
      quantity < 1 ||
      quantity > 99
    ) {
      return {
        ok: false,
        reason: "Each quantity must be a whole number from 1 to 99.",
      };
    }
    normalized.push({ priceId, quantity });
  }

  return {
    ok: true,
    cart: { reference, harvestDate, currency, total, lines: normalized },
  };
}

async function fetchPrice(fetchImpl, key, priceId) {
  const res = await fetchImpl(
    `https://api.stripe.com/v1/prices/${encodeURIComponent(priceId)}`,
    {
      method: "GET",
      headers: {
        Authorization: `Bearer ${key}`,
      },
    },
  );
  if (res.status === 404) {
    return { ok: false, reason: "One of the prices on this page was not found." };
  }
  if (!res.ok) {
    return { ok: false, reason: "Could not look up a price. Try again." };
  }
  let value;
  try {
    value = await res.json();
  } catch {
    return { ok: false, reason: "Could not look up a price. Try again." };
  }
  return { ok: true, value };
}

async function createCheckoutSession(fetchImpl, key, { cart, successUrl, cancelUrl }) {
  const params = new URLSearchParams();
  params.set("mode", "payment");
  params.set("client_reference_id", cart.reference);
  params.set("success_url", successUrl);
  params.set("cancel_url", cancelUrl);
  params.set("metadata[harvest_date]", cart.harvestDate);
  params.set("metadata[reference]", cart.reference);
  cart.lines.forEach((line, i) => {
    params.set(`line_items[${i}][price]`, line.priceId);
    params.set(`line_items[${i}][quantity]`, String(line.quantity));
  });

  const res = await fetchImpl("https://api.stripe.com/v1/checkout/sessions", {
    method: "POST",
    headers: {
      Authorization: `Bearer ${key}`,
      "Content-Type": "application/x-www-form-urlencoded",
      "Idempotency-Key": cart.reference,
    },
    body: params.toString(),
  });

  if (!res.ok) {
    return { ok: false };
  }
  let data;
  try {
    data = await res.json();
  } catch {
    return { ok: false };
  }
  if (typeof data.url !== "string" || !data.url) {
    return { ok: false };
  }
  return { ok: true, url: data.url };
}

/**
 * Parse ALLOWED_ORIGIN as a comma-separated list of exact origins.
 * Empty entries after trim are dropped. Never substring/prefix match.
 * @param {string} allowed
 * @returns {string[]}
 */
function parseAllowedOrigins(allowed) {
  return String(allowed || "")
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/**
 * Exact-equality match of the request Origin against the allow-list.
 * Returns the matched origin string, or null.
 * @param {string|null} origin
 * @param {string} allowed
 * @returns {string|null}
 */
function matchedOrigin(origin, allowed) {
  if (typeof origin !== "string" || origin.length === 0) return null;
  const list = parseAllowedOrigins(allowed);
  for (const entry of list) {
    if (origin === entry) return origin;
  }
  return null;
}

function originOk(origin, allowed) {
  return matchedOrigin(origin, allowed) !== null;
}

function truncRef(ref) {
  if (typeof ref !== "string" || ref.length === 0) return "-";
  return ref.length <= 12 ? ref : `${ref.slice(0, 12)}…`;
}

async function readBodyLimited(request, maxBytes) {
  const cl = request.headers.get("Content-Length");
  if (cl && Number(cl) > maxBytes) {
    return { ok: false, reason: "Request body is too large." };
  }
  const buf = await request.arrayBuffer();
  if (buf.byteLength > maxBytes) {
    return { ok: false, reason: "Request body is too large." };
  }
  return { ok: true, text: new TextDecoder().decode(buf) };
}

function corsHeaders(origin, allowed) {
  const matched = matchedOrigin(origin, allowed);
  if (!matched) return {};
  // Echo only the matched request Origin — never "*" and never an unmatched value.
  return {
    "Access-Control-Allow-Origin": matched,
    "Access-Control-Allow-Methods": "POST, OPTIONS",
    "Access-Control-Allow-Headers": "Content-Type",
    "Access-Control-Max-Age": "86400",
    Vary: "Origin",
  };
}

function jsonResponse(status, body, origin, allowed) {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json; charset=utf-8",
      ...corsHeaders(origin, allowed),
    },
  });
}

function textResponse(status, message, origin, allowed) {
  return new Response(message, {
    status,
    headers: {
      "Content-Type": "text/plain; charset=utf-8",
      ...corsHeaders(origin, allowed),
    },
  });
}

function corsPreflight(origin, allowed) {
  return new Response(null, {
    status: 204,
    headers: corsHeaders(origin, allowed),
  });
}
