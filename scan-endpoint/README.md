# Scan endpoint

Optional. You do not need this folder unless you print QR codes on sample
packs. The desktop app is complete without it.

This is a Cloudflare Worker with five jobs and nothing else (GT-D16, GT-D18):

- `GET /s` — record one scan, redirect to `REDIRECT_URL` (packs printed before
  tokens).
- `GET /s/<token>` — record one scan, serve the customer page for that pack.
- `POST /s/<token>` — the write door: store one standing-request candidate
  (`request_id`, `token`, `bags_per_cycle`, `requested_at`).
- `GET /scans` — the scan count since a cursor. Read-only. Pull-token gated.
- `GET /standing-requests?after=<seq>` — candidate rows after a cursor, at most
  200, ascending. Read-only. Pull-token gated (same `PULL_TOKEN`).
- `GET /a/<token>` — the field-terminal page for the paired Admin phone (GT-D21). Records no scan.
- `POST /a/<token>` — the field write door: store one phone-proposal candidate (`proposal_id`, `device_token`, `verb`, `crop_id`, `quantity`, `actual_yield_oz`, `phone_captured_at`, `note`).
- `GET /a/sw.js` — the page's service worker (offline shell).
- `GET /field-proposals?after=<seq>` — candidate rows after a cursor, at most 200, ascending. Read-only. Pull-token gated (same `PULL_TOKEN`).

It holds no farm data beyond the candidate row. It stores nothing about
tokens: what the customer page shows — the sampled varieties and the crop's
cycle — rides in the link the desktop composes (`?v=…&g=…`) and is never
persisted. It cannot tell a well-formed forged token from a real one; the
desktop resolves every candidate against its own samples when it pulls and is
the final authority. Nothing here writes farm truth. It stores nothing about devices beyond the token on each row and no crop names; the desktop resolves every token when it pulls and is the final authority.

The count is QR redirect and page hits, not people. It is a floor, not a
total: if the D1 insert fails the person is still served and that hit is lost.

## Configuration

- `FARM_NAME` — wrangler var; the one line of identity on the customer page.
  The page and its write door refuse to serve until it is set.
- `REDIRECT_URL` — wrangler var; destination for the legacy `GET /s`.
- `PULL_TOKEN` — wrangler secret; required for `GET /scans`.

## Test

```bash
cd scan-endpoint
npm ci
node --test
```

## Deploy

Install Wrangler and authenticate with Cloudflare, then create the D1
database. Put the live database id in `wrangler.toml.local` (gitignored),
never in the tracked `wrangler.toml` — that one keeps the
`REPLACE_WITH_YOUR_D1_DATABASE_ID` placeholder so no live id is ever
committed. Wrangler does not read the `.local` file on its own: pass it with
`--config` or paste the id when deploying. Then:

```bash
npx wrangler d1 execute scan-endpoint --remote --file=schema.sql   # idempotent; also after this update
npx wrangler secret put PULL_TOKEN
npx wrangler deploy
```

Set `FARM_NAME` and `REDIRECT_URL` in wrangler.toml and redeploy to change
them. Local look: `npx wrangler dev` (apply schema.sql with `--local` first).
