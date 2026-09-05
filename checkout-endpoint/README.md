# Checkout endpoint

Optional history. The shop page door in Groundtruth is unmounted — do not remount (GT-D24). Nothing on the desk sends an operator here; this Worker is kept as the retail checkout endpoint's record.

Live keys are accepted (`ALLOW_LIVE_KEYS = true` in src/handler.js). That is a
signed decision, not a default: GT-D13-R in `docs/GT-D-SERIES.md`, which
supersedes GT-D13 (Phase 4 Path B, 2026-08-13). Test keys still work. Do not
flip it back without a superseding signed decision recorded in the same
ledger.

This is a Cloudflare Worker that creates Stripe Checkout sessions. The desktop app is complete without it. It holds no farm data and stores nothing.

## Secrets

Set these with Wrangler. Never commit them.

- `STRIPE_RESTRICTED_KEY`
- `ALLOWED_ORIGIN`
- `SUCCESS_URL`
- `CANCEL_URL`

## Test

```bash
cd checkout-endpoint
npm ci
node --test
```

## Deploy

Install Wrangler and authenticate with Cloudflare, then:

```bash
npx wrangler secret put STRIPE_RESTRICTED_KEY
npx wrangler secret put ALLOWED_ORIGIN
npx wrangler secret put SUCCESS_URL
npx wrangler secret put CANCEL_URL
npx wrangler deploy
```
