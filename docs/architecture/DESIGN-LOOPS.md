# DESIGN-LOOPS — the loop face
Companion to DESIGN-DOCK.md. DESIGN-DOCK owns the dock shell: type, ink, chrome,
the eight paints. This file owns one thing — how the six core loops and the pull
between them are drawn, and what that picture is allowed to claim.

Scope: drawSchematic / edgePath / NODES / RING / #schematic in dock_shell.rs, and
the wire fields they are fed from clash_cards / clashes[] in dock_folds.rs.
Nothing here is a licence to edit those files. This is paper. A wire or shell
change needs its own signed chip.

## 1. The six loops
The farm has six core loops and never a seventh:

1. Money — what is sold, delivered, paid.
2. Cover — blackout, cover check, move to light.
3. Promise — the standing promise a chef is owed each week.
4. Rack — trays on the rack, cells, capacity.
5. Phone queue — captures waiting on the dock.
6. Health — the checks themselves.

The six are fixed by FI-3 and pinned as NODES and RING order by f6g. A seventh
loop is a new signed letter, not a design decision.

This file is NOT about agent loops. The .cursor/skills/loop-design-check skill is
ECC agent-loop design; it matches on the word "loop" and has nothing to do with
this face. Never load it for dock work.

## 2. Ring grammar — weight is the whole vocabulary
One ink (HEX A, #ccc). Severity reaches a ring as weight, never as colour:
- Healthy   — hollow ring, stroke 1.5
- Degraded  — heavy ring, stroke 3
- Unhealthy — filled ring

These are the same three the page already teaches: the rack cells (hollow /
heavy / filled) and the lead edge (stroke 3 vs 1.5, opacity 1 vs 0.55). The face
gains no new grammar. It spends the grammar the operator has already read.

Word ban holds. g4 and f6a ban Healthy / Degraded / Unhealthy and M1…H4 from the
served body, so a ring weight is chosen from a PC-shipped neutral field or from a
phone compare of one wire word against another — never from a CSS class or a JS
literal that spells a severity.

## 3. Clash stroke — the pull between two loops
A clash draws as a bow, not a straight line: edgePath offsets len * 0.14
perpendicular, so two loops pulling on each other read as tension and never as a
flowchart arrow-chain.

- lead clash — stroke 3, opacity 1
- every other clash — stroke 1.5, opacity 0.55
- direction — an arrow at the head end: the bow says which pair, the arrow says
  which way the pull runs

A pair draws ONLY when c.cards.length === 2 (showEdges, pinned by f6h).
One-sided work — move_due, harvest_due, delivered_unpaid — ships cards: None by
the closed table in clash_cards and draws no line. "Work due on the rack, not an
interaction between loops" is law, not an oversight. The picture must not invent
a line to look busy.

Pairs dedupe as "a>b". The lead is worstClash.cards.join(">").
worstClash.source must never reach the face (f5e).

## 4. EMPTY B — two empties, two sentences
Today one constant (EDGES_NONE) prints for two different farms: clashes[] empty,
and clashes[] full but every row edge-less. The shell cannot say which, so the
farm reading "9 trays are due to move to light." above six silent rings looks
broken. EMPTY B splits them:

- clashes[] is empty → EDGES_NONE, unchanged:
  "Nothing is pulling against anything right now."
- clashes[] is not empty and no pair draws → a SECOND signed constant.
  Draft for operator veto:
  "Work is due inside one loop. No loop is pulling on another."

Both are PC-side constants on the FI-8b boolean precedent. The phone picks
between two shipped sentences. It never composes one.

## 5. AGE A — words carry age, the picture never claims live
paintHeld repaints all eight paints from held, so a stale picture is
pixel-identical to a fresh one. That is deliberate. The age of the read is told
in words, by the snapshot mark, and nowhere else: no dimmed schematic, no
timestamp inside the picture, no "last sync" line, no live dot, no pulse. A ring
that looked alive would be lying every time the dock is held.

## 6. MOTION A — sci-fi is weight and geometry
No transition. No @keyframes. No requestAnimationFrame. No SVG <animate*>. No
animation-*. f6h bans the substrings animate / keyframes /
requestAnimationFrame; transition is banned here by spirit and is to be added to
f6h by letter whenever a motion chip opens.

The instrument feeling comes from stroke weight, the bow, the perpendicular
offset, ring radius, label size, 1px rules, mono numerals — not from movement. A
trailblazer panel is still when nothing has changed.

## 7. INK A — what the face never becomes
No Inter; FONT B holds. No purple, no gradient accent, no second hue. No bento
grid. No glass, blur, or frosted panel. No dark mode; color-scheme light holds.
No web font, no data: image, no external image — CSP on / is default-src 'self'
with no img-src. No "<svg" literal in the served body (f6e): the picture is
DOM-built with createElementNS. No measuring — getBoundingClientRect,
offsetWidth, clientWidth, innerWidth, getComputedStyle are banned by f6g.

## 8. Taste source — cited, never installed
Read for grammar only:
  https://github.com/Leonxlnx/taste-skill
  @ ccbc15639c97057cbfcf32ecebc38ef716e4bb37
  (HEAD, confirmed 2026-09-14 by
   git ls-remote https://github.com/Leonxlnx/taste-skill.git HEAD)
  file: skills/taste-skill/SKILL.md — frontmatter name design-taste-frontend
  MIT, (c) 2026 Leonxlnx

Dials as set for this dock, not that skill's baseline:
  DESIGN_VARIANCE 1   — one instrument, no per-screen invention
  MOTION_INTENSITY 1  — static; :hover at most, and this dock has no hover
  VISUAL_DENSITY 9    — cockpit: 1px rules separate data, no card boxes,
                        mono numerals

There is no .cursor/vendor/ clone, no npx skills add, and no farm fork at
.cursor/skills/design-taste-frontend/ (FORK B). What survives of that skill for
this farm is the LOOP section of .cursor/skills/dock-taste/SKILL.md: design-read
before the first line, motion must be motivated, copy self-audit, no filler
verbs, theme lock, one-ink lock, reduced motion respected.

What that skill says and this dock refuses: the total em-dash ban (the signed
" — " sentence grammar of check_title / check_body wins), "no hand-rolled SVG"
(drawSchematic IS the face), Geist / Outfit / Satoshi, fluid transitions,
mandatory dark mode, React / Next / Tailwind / motion / GSAP, real images, and
"no version footers or last sync" (the AGE law wins).

## 9. Law-in-waiting — signed nowhere, written here so the next chip is short
- ORDER B — #schematic moves above the six cards, directly under #clash, so the
  face is the first screen and not the second. f6f and DESIGN-DOCK §6 move with
  it.
- One PORT_DOCUMENT_VERSION bump per wire chip. Five files is the cap (Job 4).

No Rust, no TSX, and no wire change lands on any of these until the letter is
signed.
