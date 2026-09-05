# 1. The PC is the sole writer of farm truth
**Law.** The PC is the sole writer of farm truth. The phone proposes; it
does not invent the farm.
**Why.** A farm record is only worth keeping if there is exactly one place
where it can change and one order in which it changed. Two writers mean
two histories, and the moment they disagree nobody can say what happened.
Most farm software lets anyone update the numbers. Groundtruth makes sure
only one machine can — that is why the numbers stay true.
**What it forbids.**
- Any phone, browser, agent, or sync process writing to the farm record.
- Two authoritative copies of the farm. No dual-write, no "team mode" that
  implies shared authorship of farm truth, no sync model that can fork the log.
- Restore or import quietly becoming a second writer. Restore rebuilds
  from one file and archives what it replaced, read-only.
**What it still allows.**
- The phone as a **field instrument**: it can *see* the PC's own snapshot
  of Today, Health, and money, with the age of that snapshot stamped by
  the PC, and it can *capture* proposals at the rack — a weight, a count,
  a note. A proposal sits in a queue until a person confirms it on the PC.
- A read-only second person, later: viewing the same truth surfaces and
  closing a small number of already-recorded facts, with a full trail.
  Creating orders, sowing trays, or changing capacity stays with the one
  writer.
**How you can tell.** Every state change passes through one typed door
into an append-only event log, and the database can be rebuilt from that
log at any time. If a change cannot be replayed, it did not happen.
