# How we ship without softening truth
Features are easy to add. Soft money is hard to remove. This process exists
so that speed cannot quietly break the laws in `../doctrine/`. It is the
method that carried this project from an empty repository to a working farm
record in a few weeks, without a second set of books appearing anywhere.
## 1. Doctrine first
If a change fights a hard law — sole writer, money cannot lie, health can
be ugly, cash-path ranking, AI eyes only, exit tax zero — the law wins. The
change does not ship, however clever it is.
## 2. Decision before code
Product rulings are written in plain language and signed before any code
exists. Each ruling gets one line in a decision index. Code must match the
signed text exactly; reinterpreting a decision inside an implementation is
treated as a defect, and a code comment that cites a decision it does not
actually follow is treated as the same severity as a wrong trigger. A kill
list records what was refused or deferred, and it binds until amended.
## 3. Research, then employ
We look at what already exists — other paid tools in this category, and
serious engineering practice more broadly. We do not copy feature theater.
We take the jobs and the principles that work, and implement them only in
ways that obey the laws: sole writer, no soft numbers, local-first, ranked
by cash path.
## 4. Inventory before operator-facing change
Before any change to a sentence a grower reads, to what is reachable, or to
how capacity is keyed, every production path that emits or reads the thing
is listed — function and every caller — and the list is signed. The change
may touch only paths on the list. A path discovered after the fact is not a
discovery; it is a process failure, and the fix is to extend the inventory
and re-sign, not to patch quietly. This rule exists because a crop-blind
sentence once survived on a path nobody had named.
## 5. One closed job at a time
Every implementation response has a fixed shape: what the human must do in
the real world, exactly one bounded change, and either the acceptance steps
or a stop. No "block 1 of 2". No "while we're here". If a new decision is
needed, the work stops at that decision and waits — it does not invent the
answer. The old crop-blind path is deleted rather than left reachable "for
compatibility"; a reachable lie stays live.
## 6. Machine proof, then live proof
Automated checks come first: the full Rust suite must be green, and the
frontend must build. A test filter that matches zero tests is treated as a
failure, not a pass. Then a person opens the running application and
reports the exact text on the screen — not what the patch hoped it said.
Reading the source is not acceptance.
## 7. Prove once per closed job
Integrity requires proof; it does not require paying for the full suite on
every keystroke. Iterate with targeted tests. Run the full suite once at
the end of a closed job. A small named set of slow, filesystem-bound tests
runs on its own cadence; the exclusion list is named, never anonymous, and
never grows to hide a failure.
## 8. Acceptance is forward-only
After real data has been written, we do not "restore and retry" to make a
bad land look good, and we do not redefine success after the fact. A
failed land is corrected by a new, signed change.
## 9. Rank by cash path
When choosing what to build next: tray orders and honest money beat
dashboard polish. Interesting-but-not-cash work waits, and says so.
## 10. Status is a deliverable
A stale status document is a defect; it has already cost work once. Every
closed job leaves the current state written down.
## How AI fits
The code in this repository was largely written with AI coding assistants,
working one signed, bounded change at a time under the rules above, with a
human accepting every change on the running app. That is the whole reason
the rules exist: an assistant is fast, literal, and happy to build a new
path beside an old one unless the instruction names every caller. The
discipline is in the instruction and the proof, not in who typed.
## Closing
The git history is the record of pace. The laws are the record of what we
refused to break. Everything in between is in `../journey/`.
