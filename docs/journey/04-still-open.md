# Still open
Named rather than hidden. This is the honest floor as of tip `455b2e2`
(2026-09-02). Each item says what would close it. None of them is a
second product philosophy; they are packaging, reach, and scope.
## Open
| Item | State | What closes it |
|------|-------|----------------|
| Public installer pack | The app is built from source. An unsigned Windows installer builds locally, but there is deliberately no download link yet. | A published release with checksums, after the security sweep. |
| Security sweep | Not yet run as a formal pass. It is the last blocker before a public home for the repository. | A signed sweep of the tree for anything that must never ship. |
| Dead-laptop recovery drill | Written, never run end to end, and removed from scope for now. Restore from an export bundle is tested; the full cold-machine drill is not. | Running the drill on a fresh machine and recording the result. Until then, keep the old machine or a USB copy after any move. |
| Windows only, one machine, one user | By design for v0.1. | Not a residual — a scope line. A read-only second presence is a later, signed piece of work. |
| Offline See on the phone | The phone sees the desk's snapshot only while on the same network with the desk running. TLS on the dock was declined; an offline See surface would need its own signature. | A signed decision, if the board ever wants it. |
| Internationalization | The laws travel; the chrome does not. Copy is English, paper sizes and date formats follow the desk, crop defaults are microgreens. | Forks for language, currency, and regional crop packs — invited, under the same laws. A currency abstraction inside the core would be its own signed residual. |
| Unbounded disk flush on a stuck volume | A known mechanism, with every production call site inventoried and a trip condition signed: revisit if the app ever freezes on write, launch, or close, or if the farm file moves to a network, removable, or encrypted volume. | The trip condition firing. Not a hidden risk; a watched one. |
## Declined, so not open
- A step vocabulary on the phone wire. The phone shows full PC sentences.
- TLS / secure context on the LAN dock.
- Extra caps on leftover listings beyond the signed key and the harvested-ounce gate.
- Any second writer of farm truth, in any mode, for any reason.
## After everything else
Two bodies of work are explicitly sequenced after the current spine and
are not being pulled forward: systematic counters for each place a paid
tool currently looks stronger (capture latency, continuity, convenience,
collection force, onboarding, logistics), and the limited read-only second
presence described in `../doctrine/01-sole-writer.md`. Both must pass the
same laws.
## What "open" does not mean
It does not mean the integrity core is unfinished. The append-only log,
computed Health, stamp-checked restore, verify-replay, and full export are
built and tested. It means the road from a working desk to a stranger's
desk still has named steps on it.
