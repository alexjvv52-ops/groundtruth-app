# DESIGN-DESK — the desk face

Companion to DESIGN-DOCK.md (the phone page) and DESIGN-LOOPS.md (the six
loop picture). This file owns the Tauri window: the React desk the farmer
sits at. Nothing here is a licence to edit `src/` or `src-tauri/`. A desk
change needs its own signed chip.

Written against pin `a64de4368878fc851e344c340013eba3d7940977`.

## 0. SCOPE

The desk is the Tauri window. This file governs that window's column, ink,
type, motion, loading paint, and the ban on images.

It does not govern:

- the phone dock (DESIGN-DOCK.md)
- the six loops (DESIGN-LOOPS.md)
- money truth
- Health sentences
- the AGE law (DESIGN-LOOPS.md §5; ObservedValue, the phone snapshot mark,
  the invoice receipt line)

## 1. PIN AND SOURCE

Pin: `a64de4368878fc851e344c340013eba3d7940977`
("LOOP-FACE Chip 3: clash owner draws a hero ring").

Taste source, cited, never installed:

  https://github.com/Leonxlnx/taste-skill
  @ ccbc15639c97057cbfcf32ecebc38ef716e4bb37

TASTE-INSTALL C is paste-only. That repo is never cloned into this tree,
never fetched with npx, and no `.cursor/vendor` exists. Its dials and bans
are quoted here by hand or not used. Its landing-page anatomy, its font
rules, its image rules and its dark-mode section do not apply to this
product UI.

FORK B: no skill in this tree is ever named `design-taste-frontend`.

## 2. INK A

Every colour, radius and border in the desk comes from the tokens already
declared in `:root`. No new token lands without a signed ASK. The dead
tokens stay dead and unused. The `.dark` block is never toggled. Money is
never green; the single emerald use is the Healthy dot and nothing else
may borrow it.

Token list, `src/index.css` `:root` on this pin (every custom property
name declared in that block):

```
--radius
--background
--foreground
--card
--card-foreground
--popover
--popover-foreground
--primary
--primary-foreground
--secondary
--secondary-foreground
--muted
--muted-foreground
--accent
--accent-foreground
--destructive
--border
--input
--ring
--chart-1
--chart-2
--chart-3
--chart-4
--chart-5
--sidebar
--sidebar-foreground
--sidebar-primary
--sidebar-primary-foreground
--sidebar-accent
--sidebar-accent-foreground
--sidebar-border
--sidebar-ring
```

Dead on this pin (declared, never read outside `index.css`): `--chart-1`
through `--chart-5`, and `--sidebar` through `--sidebar-ring`. They stay
dead.

The `.dark { ... }` block in `index.css` exists. No `className` on the
tree sets `dark`. It is never toggled.

Emerald, the one exception, is not a `:root` token. StatusMark paints it
as `bg-emerald-600` and StatusMark is untouched:

  `className="ml-2 inline-block h-2 w-2 self-center rounded-full bg-emerald-600"`

Nothing else may borrow that class or that green.

## 3. DESK-WIDTH A

Re-proved at pin `6b6ccfb1ba1126f1ad6dff09be2884495c3c3dc4`
("LIVELY SHELL: nav on the window, sheets right at 900").

The content column is 28rem (`max-w-md`) and stays 28rem at every window
size, maximized included. No full-desk grid, no left nav rail, no fluid
column. What LIVELY SHELL moved is where that width is declared: the App
shell no longer carries it, each screen's `<main>` does.

The shell class, `src/App.tsx`:

  `mx-auto flex min-h-screen w-full flex-col`

The nav may span the window. It is a wrapping strip on a hairline, padded
to the window and not to the column, `src/App.tsx`:

  `flex flex-wrap items-end gap-x-3 border-b border-border px-6 text-sm min-[440px]:gap-x-4`

The column is per screen. Every screen's `<main>` carries `max-w-md`
(`Today.tsx`, `Reality.tsx`, `Money.tsx`, `Marketing.tsx`, `Settings.tsx`,
`Health.tsx`, `Books.tsx`), and the StatusMark row under the nav in
`App.tsx` carries `mx-auto w-full max-w-md px-6`.

Sheets are a right panel at 900px and up. SHEET-SIDE B,
`src/components/ui/sheet.tsx`, `side === "bottom"`:

  `min-[900px]:inset-y-0 min-[900px]:right-0 min-[900px]:left-auto min-[900px]:h-full min-[900px]:max-h-none min-[900px]:w-[28rem]`

That panel is 28rem — the content column's own width. Below 900px the same
sheet stays a bottom drawer. `side === "right"` is unchanged
(`w-3/4 sm:max-w-sm`).

The Today rack rail is IN FLOW. The `position:absolute; left:100%` hang is
retired, `src/index.css`:

  `.rack-rail { width: 100%; }`

## 4. TYPE LADDER

Re-proved at pin `6b6ccfb1ba1126f1ad6dff09be2884495c3c3dc4` across the
landed ladder: `9059469` Chip 0, `658adf2` TODAY, `cd8f616` FARM,
`d4fa2d9` MONEY, `16348b0` BOOKS, `0aaee95` SETTINGS, `4874a2b`
MARKETING, `a6621e0` HEALTH, `6b6ccfb` SHELL.

The fact outranks the page name. Classes actually in use:

- page name h1 — `text-2xl font-semibold tracking-tight`
  (Today, Farm, Marketing, Money, Health, Settings). Books has no page h1.
- fact — `text-3xl font-medium tabular-nums`, where a fact exists: the
  Today owed line, the Farm growing count, the Money owed line, the
  Marketing time-to-first-standing-order, and each Books figure.
  Money's cash-in and cash-out totals sit at the same size as
  `text-3xl font-semibold tabular-nums`.
- Settings and Health have no fact. Their page name is the same `text-2xl`
  and nothing on those screens sits above it.
- row / tap — `text-xl font-medium` (the Today and Farm tap and row
  classes, and the sheet titles).
- body — `text-base` / `text-base font-medium` / `text-base font-medium leading-snug`
- label — `text-sm text-muted-foreground`
- tabular numerals — `src/index.css` under `@media screen`:
  `body { font-variant-numeric: tabular-nums; }`
  Facts and count pads also carry the `tabular-nums` class.

This is the ladder the tree proves at this pin. No letter is signed here.

## 5. DIALS

Desk: VARIANCE 2 / MOTION 4 / DENSITY 8.
Phone: VARIANCE 1 / MOTION 1 / DENSITY 9.
Desk MOTION never goes above 5.

## 6. DESK-MOTION B

CSS only. Exactly four legal motions, nothing else.

M1 — state/colour feedback on an interactive element. `src/components/ui/button.tsx`:

  `transition-colors duration-150` with `motion-reduce:transition-none`

The same colour-transition string also sits on the App nav tabs. That is
M1, not a fifth motion.

M2 — reveal of content that has just arrived. `src/screens/Today.tsx`:

  `animate-in fade-in-0 slide-in-from-top-1 duration-150 motion-reduce:animate-none`

FLAG: the tree's reveal is not opacity-only. `slide-in-from-top-1` is a
transform. The words above call M2 a reveal; the pin also slides.

M3 — sheet open / close. `src/components/ui/sheet.tsx`:

  `data-[state=closed]:duration-300`
  `data-[state=open]:duration-500`

FLAG: the same `SheetContent` class also carries `transition ease-in-out`
and slide-in / slide-out transforms; the close control carries
`transition-opacity`. The durations are 300 closed / 500 open.

M4 — settle on a new paint — RESERVED. Opacity only, one shot, ≤200ms,
no transform of anything carrying a number. Lands in its own later room;
no code for it exists yet.

BAN LIST: no keyframe loops, no `requestAnimationFrame`, no scroll-driven
or parallax motion, no page-load entrance beyond M2, no motion on a
number that carries money truth, no motion library.

REDUCED MOTION. `src/index.css`:

```
  @media (prefers-reduced-motion: reduce) {
    .animate-in,
    .animate-out {
      animation: none !important;
    }
  }
```

Every one of M1–M4 is covered by reduced motion. FLAG: the quoted block
covers `.animate-in` and `.animate-out` only (M2, M3). M1 is covered by
`motion-reduce:transition-none` on the button, not by this block. M4 has
no code yet; when it lands it must still under this reduce.

## 7. STATES A

A screen that can be loading paints a skeleton, never a spinner and never
blank; the skeleton fill is `--border` and carries no motion.

Why `--border` and not `--muted`, from the `:root` list on this pin:

  `--background: oklch(0.967 0.001 286.375);`
  `--muted: oklch(0.967 0.001 286.375);`
  `--border: oklch(0.92 0.004 286.32);`

`--muted` equals `--background` and would vanish. `--border` is the
visible fill already on the list.

On this pin the tree still paints blank: Today waits on
`{!loading && shape !== null && (` and Farm waits on `{!loading && view`.
No `skeleton`, no spinner. The skeleton is later-room work. This chip
writes no CSS.

Every pressable gets an `:active` press, and the landed tap classes carry
it. Re-proved at pin `6b6ccfb1ba1126f1ad6dff09be2884495c3c3dc4`: the press
is the `active:translate-y-px` utility on the call sites — Today, Farm,
Money, Marketing, Settings, Health and Books all carry it. `src/index.css`
declares no `:active` rule and the `button.tsx` cva base carries none; the
class rides on each tap class instead.

StatusMark is untouched. Health cannot lie and the AGE law wins.

## 8. DESK-IMG A

The desk ships no images. No `<img>`, no stock, no illustration family,
no icon font. The hand-written JSX SVG already on the tree (QR marks,
glyphs) stays as it is.

CSP TRAP. `src-tauri/tauri.conf.json`:

  `"csp": "default-src 'self'; connect-src ipc: http://ipc.localhost; style-src 'self' 'unsafe-inline'"`

With no `img-src` and no `font-src`, `data:` URLs are blocked in the
built exe while the dev server is CSP-blind — so an image that shows in
`npm run dev` can vanish in the installed app. `npm run dev` is the only
look loop.

## 9. PHONE HOLDS

PHONE-MOTION A and PHONE-IMG A: this file changes nothing on the phone
dock. No motion, no images, three buttons, no `img`, the phone writes
nothing.

## 10. HOW A ROOM LANDS

One fence per room. Named files only. Line endings kept
(`git ls-files --eol`). No formatter. Never `git add -A`, never
`git add .`, never `git add .cursor`. Commit only when the last STEP
says the word. STOP when something is unnamed. Prove on the tree. Never
invent.
