# Dock design - the served shell

One page. Only what exists in the repository at pin `89cf6c1`.

The dock is the phone face. It is one Rust string, `SHELL` in
`src-tauri/src/dock_shell.rs`, served at `/` by `dock_port.rs` over the local
network behind an admin token. It is not a React screen: `src/index.css` never
reaches the phone, there is no bundler, no build step and no request off the
desk. The phone reads; the PC is the sole writer.

Ten constraints. A fence may change the shell only inside them. Anything else
is an ASK letter first.

## 1. Ink
`currentColor` carries every mark. The one colour literal in the served body is
`#ccc`, and it draws hairlines only. No second hex, no oklch, rgb or hsl value,
no gradient, no shadow, no tint. A new colour needs a signed HEX letter.

## 2. Paper
The shell owns its whole look inside one `<style>` in the string.
`color-scheme: light`. No `:root` block, no `var(--)`, no dark variant, no
import. `src/index.css` is not in this file's world.

## 3. Lengths
Every length the shell states is in `rem`. The body base is `1.0625rem` and
never drops below `1rem`. Small type - ages, heads, pull lines - sits at
`0.9375rem` or `0.875rem`; nothing is smaller except `#evidence`, the
monospace pane at `0.75rem`. The only px on the tree are rule widths: four
`1px` hairlines and four `4px` `border-left` rules. No viewport units, no
fixed pixel type scale.

## 4. Taps
Every button, anchor and input is at least `3rem` tall, full width in the
column, with space between it and its neighbour. The sibling to rhyme with is
the Worker capture page (`scan-endpoint/handler.js`: 3rem targets, 3.25rem
primary, `main` at 28rem).

## 5. Column
One column, about `28rem`, centred. Nothing sits side by side on a phone.

## 6. Ages first
The served DOM order is signed grammar (g8, and f6e/f6f/f7e/f8d/f11d): line,
evaluated, overall, clash, cards, tasks, edges and schematic, queue, pull
lines, Pull now, Capture, token form, evidence. The next fact and how old it is
come before anything else. Reordering needs an ORDER letter.

## 7. Three buttons
Pull now, Remember, Forget token. One anchor: Capture something. One
visible-text input: `type="text"`, with `autocapitalize`, `autocorrect` and
`spellcheck` off (TOKEN-FIELD B). The count is pinned at seven test sites. A
fourth control needs a BUTTONS letter.

## 8. Words
ASCII only in the served body. No F/M/H digit labels (G-2). Every number and
every sentence arrives from Rust over `/folds`: the shell formats, it never
invents a number, a sentence or a verb. `g4` proves the body carries no farm
data; `g9` proves it never builds markup from strings - text goes in as text.
The phone proposes and never confirms.

## 9. Bans
No React, shadcn, Tailwind, Vite, Magic UI, 21st.dev, bento, glass, particles.
No web font, no `@font-face`, no image, no external URL, no CDN. No
`navigator.clipboard`, no share sheet, no service worker. The CSP stays
`default-src 'self'` with `script-src` and `style-src` `'self'
'unsafe-inline'` (g3), the response stays no-store with no CORS. The dock stays
plaintext HTTP on trusted Wi-Fi (TLS MOVE A): no cert work lives here.

## 10. The loop
The look is judged on a phone photo of the served page - `http://<lan>:18765`
from the running app, or the lab dock below on `:18766`. Never Vite, never
`tauri dev`, never the installed exe. Any edit to `dock_shell.rs` is a Rust
edit: `cargo fmt --all -- --check` stays silent (T-5, check only) and
`cargo test --manifest-path src-tauri/Cargo.toml dock_port_tests` stays green.

## The lab dock
`src-tauri/src/dock_lab_tests.rs` holds one `#[ignore]` test that serves the
real shell through the real door on a seeded in-memory farm, so the phone can
be photographed without opening the live farm:

    cargo test --manifest-path src-tauri/Cargo.toml dock_lab -- --ignored --nocapture --test-threads=1

It binds `0.0.0.0:18766`, prints the LAN URL and a fixture token, holds for a
bounded time and then stops. Product gates never run it.

## Open letters
Not settled. A fence that needs one of these stops and asks:
TYPE-BASE (base font size), FONT (the stack), ORDER (DOM order), ACT-BAR,
TOKEN-FOLD, SEE-RACK (rack numbers on the phone), VERSION (a documentVersion
bump past 9), CAPTURE-QUERY, QR-DOOR, BLACKOUT-INK.

## Where the facts live
`dock_shell.rs` - the served string. `dock_port.rs` - the door, the token check,
`DOCK_BIND` 0.0.0.0:18765. `dock_folds.rs` - the port document,
`PORT_DOCUMENT_VERSION` 13. `dock_port_tests.rs` - the gates g1 to g9 and
`PORT_DOC_KEYS` 18. `field_devices.rs` - pairing and the admin token.
