//! Data-free phone shell. Served on GET / without a token because a browser
//! navigation cannot set a header. No farm state lives in this document.

/// One self-contained page. `{when}` is filled at render from the PC-composed
/// `servedAtDisplay` and `attentionEvaluatedAtDisplay`. The page composes no
/// time of its own (FI-1).
///
/// D-3 (G1 ruling, 2026-08-25): the capture anchor carries rel="noreferrer".
/// Without it the browser hands this dock's own LAN origin to the public
/// capture Worker as a Referer on the way out. G1 was declined, so this is
/// the whole of that ruling's code: the return journey stays the browser's
/// Back plus the pageshow repaint, and the navigation stays plain and
/// same-tab.
pub const SHELL: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Dock</title>
<style>
html { color-scheme: light; }
body { box-sizing: border-box; max-width: 28rem; margin: 0 auto; padding: 1.25rem 1rem 3rem; font-family: system-ui,-apple-system,Segoe UI,Roboto,sans-serif; font-size: 1.0625rem; line-height: 1.45; }
#line { margin: 0 0 0.25rem; font-size: 0.9375rem; }
#evaluated { margin: 0 0 1.25rem; font-size: 0.9375rem; }
#evaluated:empty { display: none; }
#overall { margin: 0 0 0.5rem; font-size: 2.25rem; font-weight: 700; line-height: 1.2; }
#clash { margin: 0 0 1.5rem; padding-left: 0.75rem; border-left: 4px solid currentColor; font-size: 1.5rem; font-weight: 600; line-height: 1.25; }
#overall:empty, #clash:empty { display: none; }
.row { display: flex; justify-content: space-between; gap: 1rem; margin: 0; padding: 0.75rem 0 0.2rem; border-top: 1px solid #ccc; font-size: 1rem; }
.sev { font-weight: 600; }
.cface { margin: 0.1rem 0 0; font-size: 1.0625rem; }
.cage { margin: 0.1rem 0 0.6rem; font-size: 0.875rem; }
.rack { display: flex; flex-wrap: wrap; gap: 0.25rem; margin: 0.35rem 0 0; }
.rcell { box-sizing: border-box; width: 0.8rem; height: 0.8rem; border: 1px solid #ccc; }
.rcell.lit { border-color: currentColor; background: currentColor; }
.rcell.dark { border-color: currentColor; background: currentColor; opacity: 0.55; }
.rcap { margin: 0.35rem 0 0; font-size: 1rem; }
.rceil { margin: 0.1rem 0 0; font-size: 0.875rem; }
#tasksHead, #edgesHead, #queueHead, #pullHead { margin: 2rem 0 0.5rem; font-size: 0.9375rem; font-weight: 600; letter-spacing: 0.06em; text-transform: uppercase; }
#tasksHead:empty, #edgesHead:empty, #queueHead:empty, #pullHead:empty { display: none; }
.task { margin: 0; padding: 0.4rem 0 0.4rem 0.75rem; border-left: 4px solid #ccc; font-size: 1.0625rem; }
#tasks .task:nth-child(2) { border-left-color: currentColor; padding-top: 0.5rem; padding-bottom: 0.5rem; font-size: 1.25rem; font-weight: 600; }
#schematic { margin: 0 0 1rem; }
#schematic:empty { display: none; }
#schematic svg { display: block; width: 100%; height: auto; max-width: 26rem; }
.edge { margin: 0; padding: 0.4rem 0 0.4rem 0.75rem; border-left: 4px solid transparent; font-size: 1.0625rem; }
.edge.worst { border-left-color: currentColor; font-weight: 600; }
.qcount { margin: 0 0 0.4rem; font-size: 1.0625rem; }
.qrow { margin: 0; padding: 0.4rem 0 0.4rem 0.75rem; border-left: 4px solid #ccc; font-size: 1rem; }
.pline { margin: 0.2rem 0; font-size: 0.9375rem; }
#acts { display: flex; gap: 0.75rem; margin: 2rem 0 0; }
#acts > * { box-sizing: border-box; flex: 1 1 0; min-height: 3rem; padding: 0.75rem 0.5rem; border: 1px solid currentColor; background: none; color: inherit; font: inherit; text-align: center; text-decoration: none; }
#pull:disabled { opacity: 0.5; }
#capture:not([href]) { display: none; }
#tokfold { margin: 2rem 0 0; }
#toksum { box-sizing: border-box; min-height: 3rem; padding: 0.75rem 0; font-size: 0.9375rem; font-weight: 600; letter-spacing: 0.06em; text-transform: uppercase; cursor: pointer; }
form { margin: 0.5rem 0 0; display: flex; flex-wrap: wrap; gap: 0.5rem; }
form > * { box-sizing: border-box; flex: 1 1 100%; min-height: 3rem; padding: 0.75rem; border: 1px solid currentColor; background: none; color: inherit; font: inherit; }
#evidence { max-height: 16rem; overflow: auto; margin: 2rem 0 0; padding: 0.75rem; border: 1px solid #ccc; font-family: monospace; font-size: 0.75rem; white-space: pre-wrap; -webkit-user-select: all; user-select: all; }
#evidence:empty { display: none; }
</style>
</head>
<body>
<p id="line"></p>
<p id="evaluated"></p>
<p id="overall"></p>
<p id="clash"></p>
<div id="cards"></div>
<h2 id="tasksHead"></h2>
<div id="tasks"></div>
<h2 id="edgesHead"></h2>
<div id="schematic"></div>
<div id="edges"></div>
<h2 id="queueHead"></h2>
<div id="queue"></div>
<h2 id="pullHead"></h2><div id="pullLines"></div>
<button type="button" id="pull">Pull now</button>
<a id="capture" rel="noreferrer">Capture something</a>
<form id="tok">
<input id="token" type="text" autocomplete="off" autocapitalize="off" autocorrect="off" spellcheck="false">
<button type="submit">Remember</button>
<button type="button" id="forget">Forget token</button>
</form>
<pre id="evidence"></pre>
<script>
var S1 = "Read from the PC at {when}.";
var S2 = "The PC has not answered yet. These numbers are from {when}.";
var S3 = "Not connected to the PC. These numbers are from {when} and are not current. Dock again to refresh.";
var S4 = "Not connected to the PC. This phone has no farm numbers yet. Dock to the PC to read them.";
var EVAL_PRESENT = "Farm last evaluated on the PC at {when}.";
var EVAL_NEVER = "Farm not evaluated on the PC since it started.";
// FI-6 - the edge list. Signed operator text, same class as S1-S4 and TITLES.
// The arrow is an escape: h2 asserts SHELL.is_ascii(), so a literal glyph here
// would turn the build red.
var EDGES_HEAD = "Pulling against each other";
var EDGES_NONE = "Nothing is pulling against anything right now.";
// EMPTY-TWO - the second empty-edge sentence. EDGES_NONE means the wire
// carried no clash at all. This one means the wire carried work and not one
// row of it names two cards: the load is real and it sits inside a single
// loop. Same class as EDGES_NONE and S1-S4 - signed operator text, ascii,
// no farm data, nothing composed, nothing pluralised.
var EDGES_INSIDE = "Work is due inside one loop. No loop is pulling on another.";
var ARROW = " \u2192 ";
// FI-6b - the schematic's geometry (G'-3). Constant coordinates in a fixed
// viewBox: the picture measures nothing, so neither the phone's viewport nor a
// farm number can ever reach a coordinate. An edge is a boolean off the wire.
// Ring order is adjacency (G'-2b) - it draws the whole closed edge set with
// zero crossings. phone_queue sits in the ring and is touched by no line,
// because no clash in the signed table names it. That is a fact, not a gap.
var NODES = {
  system: { x: 200, y: 48, lx: 200, ly: 24, anchor: "middle" },
  money: { x: 280, y: 94, lx: 302, ly: 98, anchor: "start" },
  promise: { x: 280, y: 186, lx: 302, ly: 190, anchor: "start" },
  cover: { x: 200, y: 232, lx: 200, ly: 264, anchor: "middle" },
  rack: { x: 120, y: 186, lx: 98, ly: 190, anchor: "end" },
  phone_queue: { x: 120, y: 94, lx: 98, ly: 98, anchor: "end" }
};
var RING = ["system", "money", "promise", "cover", "rack", "phone_queue"];
var SVGNS = "http://www.w3.org/2000/svg";
// FI-7 - the section heading. The count line and every row sentence are
// composed on the PC and arrive on the wire, so nothing here is pluralised or
// assembled in JavaScript.
var QUEUE_HEAD = "Waiting for Confirm on the PC";
var PULL_HEAD = "Captures reaching the PC";
// FI-10b - the pull's own age, signed as option (b-i). Same class as CARD_AGE
// and S1-S4: constant operator text, ascii, no farm data. {when} arrives
// PC-composed through withWhen, so the phone still owns no clock here either.
var PULL_AGE = "Last checked {when}.";
// FI-8 - the task mirror's own strings. Every row sentence already exists:
// clashes[] is the ranked Today surface and the PC composed each line at FI-5.
//
// FI-8b - the wire has always arrived rank-ordered, and the shell printed that
// order while saying nothing about it. TASKS_FIRST says it. That is the whole
// residual: no new farm vocabulary, because the sentences already name the work
// - a row states the crop, the count and how late it is, in the PC's words.
var TASKS_HEAD = "What Today is asking for";
var TASKS_NONE = "Today's queue is clear.";
var TASKS_FIRST = "Start here.";
// FI-4b - the card face's one new string (S-1). Same class as S1-S4: constant
// operator text, ascii, no farm data. {when} arrives PC-composed through
// withWhen, so the phone still owns no clock in this path.
var CARD_AGE = "Last reported {when}.";
// Job 5 (SEE-RACK B) - the third rack string. Same class as EVAL_NEVER and
// CARD_AGE: constant operator text, ascii, no farm data, nothing composed.
var RACK_NO_CEILING = "ceiling not set";
// FI-4 - display titles. Six fixed labels for six fixed wire keys. Same class
// as S1-S4: constant operator text that lives in the shell and is frozen by a
// test. No farm data is involved and nothing is composed from the document -
// the keys on the wire are unchanged, only what the operator reads.
var TITLES = {
  money: "Money",
  cover: "Cover",
  promise: "Promise",
  rack: "Rack",
  phone_queue: "Phone queue",
  system: "Health"
};
var KEY = "dockToken";
var held = null;

// FI-2 refresh state. Backoff is counted in TICKS, never in elapsed time:
// f1a forbids a clock in this file, and a phone clock must never influence
// what the operator is told about age.
var TICK_MS = 60000;
var BACKOFF_MAX = 8;
var inFlight = false;
var ticker = null;
var skipTicks = 0;
var backoff = 1;

// FI-1: the phone owns no clock in this path. Every {when} is a string the PC
// composed (dock_folds::WHEN_FORMAT). The shell only substitutes it, so a
// phone with a wrong clock or a different timezone cannot change the age.
function withWhen(s, when) {
  return s.replace("{when}", when);
}

// The second age. Never derived from the served time: the port reads open rows
// and does not evaluate, so "served" and "evaluated" are different facts.
function showEvaluated(doc) {
  var when = doc.attentionEvaluatedAtDisplay;
  document.getElementById("evaluated").textContent =
    when ? withWhen(EVAL_PRESENT, when) : EVAL_NEVER;
}

function showLine(text) {
  document.getElementById("line").textContent = text;
}

function clearNumbers() {
  document.getElementById("capture").removeAttribute("href");
  document.getElementById("tasksHead").textContent = "";
  document.getElementById("tasks").textContent = "";
  document.getElementById("queueHead").textContent = "";
  document.getElementById("queue").textContent = "";
  showPull(null);
  document.getElementById("edgesHead").textContent = "";
  document.getElementById("schematic").textContent = "";
  document.getElementById("edges").textContent = "";
  document.getElementById("evaluated").textContent = "";
  document.getElementById("overall").textContent = "";
  document.getElementById("clash").textContent = "";
  document.getElementById("cards").textContent = "";
  document.getElementById("evidence").textContent = "";
}

// An unmapped key renders as the key: visibly wrong rather than invisibly
// missing. f4a proves all six are mapped, so that arm is unreachable today.
// hasOwnProperty, not a bare lookup, so an inherited property name can never
// be mistaken for a title.
function titleFor(card) {
  if (Object.prototype.hasOwnProperty.call(TITLES, card)) {
    return TITLES[card];
  }
  return card;
}

// FI-8 - the task mirror. `clashes` is already the ranked Today surface: the
// open Today attention rows in the PC's own order, plus standing shortfall,
// move due and harvest due, each carrying the sentence the PC composed. The
// phone prints them in the order they arrive and ranks nothing.
//
// Receipts are not here - worst_clash excludes them, so the desk's confirmation
// rows never reach this list. Neither is anything to press: FI-8 adds no
// control, and completing work stays a PC action.
//
// The top row is also the headline in #clash. That is one fact read once and
// shown twice, not two facts (T-2).
// FI-10 - See -> Act. The wire carries the capture page's origin only. The
// token comes from this phone's own localStorage, where pairing already put it,
// so the secret never rides the wire and never reaches the diagnosis block.
// No href when either half is missing: a dead link is worse than no link.
function showCapture(doc) {
  var a = document.getElementById("capture");
  var endpoint = doc.captureEndpoint;
  var token = localStorage.getItem(KEY) || "";
  if (endpoint && token) {
    a.setAttribute("href", endpoint + "/a/" + encodeURIComponent(token) + (doc.captureQuery || ""));
  } else {
    a.removeAttribute("href");
  }
}

function showTasks(doc) {
  document.getElementById("tasksHead").textContent = TASKS_HEAD;
  var box = document.getElementById("tasks");
  box.textContent = "";
  var list = doc.clashes || [];
  var said = [];
  for (var i = 0; i < list.length; i++) {
    var s = list[i].sentence;
    if (!s) continue;
    said.push(s);
  }
  if (said.length === 0) {
    var none = document.createElement("p");
    none.className = "task";
    none.textContent = TASKS_NONE;
    box.appendChild(none);
    return;
  }
  // FI-8b - "Start here." only when there is a here to start at. The empty
  // state returns above it, so a clear queue never gets an instruction.
  var first = document.createElement("p");
  first.className = "task";
  first.textContent = TASKS_FIRST;
  box.appendChild(first);
  for (var j = 0; j < said.length; j++) {
    // The number is the position in a list the PC ordered - an ordinal, not a
    // farm fact, the same class as withWhen's substitution. `rank` is on the
    // wire and is deliberately never printed: it is how the order was decided,
    // not something the operator needs to read.
    var ordinal = j + 1;
    var row = document.createElement("p");
    row.className = "task";
    row.textContent = ordinal + ". " + said[j];
    box.appendChild(row);
  }
}

// FI-7 - what the PC is holding. Every string here was composed on the PC:
// the count line and each capture sentence arrive ready to print. The phone
// counts nothing, pluralises nothing and decides nothing.
function showQueue(doc) {
  document.getElementById("queueHead").textContent = QUEUE_HEAD;
  var box = document.getElementById("queue");
  box.textContent = "";
  var q = doc.phoneQueue;
  var count = document.createElement("p");
  count.className = "qcount";
  count.textContent = q && q.sentence ? q.sentence : "";
  box.appendChild(count);
  var rows = q && q.rows ? q.rows : [];
  for (var i = 0; i < rows.length; i++) {
    var row = document.createElement("p");
    row.className = "qrow";
    row.textContent = rows[i].sentence;
    box.appendChild(row);
  }
}

function showPull(doc){
  var head = document.getElementById("pullHead");
  var box = document.getElementById("pullLines");
  box.textContent = "";
  var p = doc && doc.pullHealth ? doc.pullHealth : null;
  if(!p){ head.textContent = ""; return; }
  head.textContent = PULL_HEAD;
  var out = [p.message, p.lastOkMessage, p.refusalMessage, p.gapMessage];
  for(var i=0;i<out.length;i++){
    var s = out[i];
    if(!s){ continue; }
    var d = document.createElement("div");
    d.className = "pline";
    d.textContent = s;
    box.appendChild(d);
  }
  // FI-10b - when the PC last looked. A different fact from what that look
  // found, so it is read off the document and not off pullHealth. No line when
  // nothing has run: the operator reads silence, not a guess.
  var when = doc.lastPullAtDisplay;
  if(when){
    var w = document.createElement("div");
    w.className = "pline";
    w.textContent = withWhen(PULL_AGE, when);
    box.appendChild(w);
  }
}

// FI-6 - one row per distinct ordered pair, worst first.
//
// `clashes` arrives rank-sorted from the PC and `worstClash` is its head, so
// the order here is the PC's order - the phone never re-ranks anything.
//
// A clash with no pair draws nothing. Three sources carry no edge by signed
// design (delivered-unpaid, move due, harvest due), and an unlisted kind gets
// None from clash_cards rather than a guessed line. Both ends go through
// titleFor, so a raw wire key can never reach the face.
// FI-6b - one bowed edge (G'-5). Every edge curves off its midpoint on the same
// perpendicular, so a>b and b>a bow opposite ways and can never be drawn as one
// line - cover>promise and promise>cover are both reachable at once. The ends
// are pulled back by the ring radius plus a little: exact along the straight
// direction, close enough on the curve for the head to clear the ring.
function edgePath(a, b, lead) {
  var dx = b.x - a.x;
  var dy = b.y - a.y;
  var len = Math.sqrt(dx * dx + dy * dy);
  var ux = dx / len;
  var uy = dy / len;
  var x1 = a.x + ux * 20;
  var y1 = a.y + uy * 20;
  var x2 = b.x - ux * 20;
  var y2 = b.y - uy * 20;
  var bow = len * 0.14;
  var cx = (x1 + x2) / 2 - uy * bow;
  var cy = (y1 + y2) / 2 + ux * bow;
  var p = document.createElementNS(SVGNS, "path");
  p.setAttribute("d", "M " + x1.toFixed(1) + " " + y1.toFixed(1) + " Q " +
    cx.toFixed(1) + " " + cy.toFixed(1) + " " +
    x2.toFixed(1) + " " + y2.toFixed(1));
  p.setAttribute("fill", "none");
  p.setAttribute("stroke", "currentColor");
  p.setAttribute("stroke-width", lead ? "3" : "1.5");
  p.setAttribute("opacity", lead ? "1" : "0.55");
  p.setAttribute("marker-end", "url(#dockArrow)");
  return p;
}
// FI-6b - the picture. It decides nothing: `pairs` is the deduplicated set
// showEdges already drew as text, and `worst` is the same key that marked the
// worst row, so the diagram and the list cannot disagree about which edge
// leads. G'-8a: the six rings are always drawn - six unconnected rings is the
// true picture of a quiet farm. G'-7: currentColor only, no severity palette.
// G'-10: aria-hidden, because the text rows below are the accessible layer.
// RING-WEIGHT (WEIGHT-SRC A) - the ring's weight arrives as a number the PC
// decided. 1 is the heavy stroke a lead edge already uses and 2 fills the disc,
// so the picture borrows the rack cells' hollow/heavy/filled grammar instead of
// inventing a second one. The phone owns no table: an unknown or missing weight
// draws the hollow ring. G'-7 currentColor only. G'-6a static, never motion.
// HERO-RING (OWNER A) - one extra ring marks the loop the hero clash is
// owned by. The owner is a PC field on the same row that wrote the
// sentence at the top of the face, so the ring and the sentence cannot
// disagree. The phone owns no table: it matches the owner against RING
// keys, so a missing or unknown owner matches nothing and paints
// nothing. Same ink, same stillness - a wider circle, not a colour and
// not a motion.
function drawSchematic(pairs, worst, cards, hero) {
  var box = document.getElementById("schematic");
  box.textContent = "";
  var svg = document.createElementNS(SVGNS, "svg");
  svg.setAttribute("viewBox", "0 0 400 288");
  svg.setAttribute("width", "100%");
  svg.setAttribute("aria-hidden", "true");
  var defs = document.createElementNS(SVGNS, "defs");
  var marker = document.createElementNS(SVGNS, "marker");
  marker.setAttribute("id", "dockArrow");
  marker.setAttribute("viewBox", "0 0 10 10");
  marker.setAttribute("refX", "9");
  marker.setAttribute("refY", "5");
  marker.setAttribute("markerWidth", "6");
  marker.setAttribute("markerHeight", "6");
  marker.setAttribute("markerUnits", "strokeWidth");
  marker.setAttribute("orient", "auto");
  var headArrow = document.createElementNS(SVGNS, "path");
  headArrow.setAttribute("d", "M 0 0 L 10 5 L 0 10 z");
  headArrow.setAttribute("fill", "currentColor");
  marker.appendChild(headArrow);
  defs.appendChild(marker);
  svg.appendChild(defs);
  for (var i = 0; i < pairs.length; i++) {
    var a = NODES[pairs[i][0]];
    var b = NODES[pairs[i][1]];
    if (!a || !b) continue;
    var lead = (pairs[i][0] + ">" + pairs[i][1]) === worst;
    svg.appendChild(edgePath(a, b, lead));
  }
  for (var j = 0; j < RING.length; j++) {
    var n = NODES[RING[j]];
    var w = null;
    for (var k = 0; k < cards.length; k++) {
      if (cards[k] && cards[k].card === RING[j]) {
        w = cards[k].weight;
        break;
      }
    }
    var ring = document.createElementNS(SVGNS, "circle");
    ring.setAttribute("cx", n.x);
    ring.setAttribute("cy", n.y);
    ring.setAttribute("r", "16");
    ring.setAttribute("fill", w === 2 ? "currentColor" : "none");
    ring.setAttribute("stroke", "currentColor");
    ring.setAttribute("stroke-width", w === 1 ? "3" : "1.5");
    svg.appendChild(ring);
    if (RING[j] === hero) {
      var heroRing = document.createElementNS(SVGNS, "circle");
      heroRing.setAttribute("cx", n.x);
      heroRing.setAttribute("cy", n.y);
      heroRing.setAttribute("r", "22");
      heroRing.setAttribute("fill", "none");
      heroRing.setAttribute("stroke", "currentColor");
      heroRing.setAttribute("stroke-width", "1.5");
      svg.appendChild(heroRing);
    }
    var label = document.createElementNS(SVGNS, "text");
    label.setAttribute("x", n.lx);
    label.setAttribute("y", n.ly);
    label.setAttribute("text-anchor", n.anchor);
    label.setAttribute("font-size", "15");
    label.setAttribute("fill", "currentColor");
    label.textContent = titleFor(RING[j]);
    svg.appendChild(label);
  }
  box.appendChild(svg);
}
function showEdges(doc) {
  document.getElementById("edgesHead").textContent = EDGES_HEAD;
  var box = document.getElementById("edges");
  box.textContent = "";
  var list = doc.clashes || [];
  var worst =
    doc.worstClash && doc.worstClash.cards && doc.worstClash.cards.length === 2
      ? doc.worstClash.cards.join(">")
      : "";
  var hero = (doc.worstClash && doc.worstClash.owner) || "";
  var seen = {};
  var pairs = [];
  var drawn = 0;
  for (var i = 0; i < list.length; i++) {
    var c = list[i];
    if (!c.cards || c.cards.length !== 2) continue;
    var key = c.cards[0] + ">" + c.cards[1];
    if (Object.prototype.hasOwnProperty.call(seen, key)) continue;
    seen[key] = true;
    pairs.push(c.cards);
    var row = document.createElement("p");
    row.className = key === worst ? "edge worst" : "edge";
    row.textContent = titleFor(c.cards[0]) + ARROW + titleFor(c.cards[1]);
    box.appendChild(row);
    drawn++;
  }
  if (drawn === 0) {
    var none = document.createElement("p");
    none.className = "edge";
    // EMPTY-TWO - `list` is doc.clashes. No clash on the wire keeps the
    // original sentence. Clashes on the wire with no pair among them is the
    // other farm, and it gets the other sentence. Same projection shape as
    // TASKS_FIRST: one constant assigned to textContent, no template, no
    // concatenation, no wire value on this line.
    none.textContent = list.length === 0 ? EDGES_NONE : EDGES_INSIDE;
    box.appendChild(none);
  }
  // G'-9 - one paint. The picture is drawn from the set the rows were drawn
  // from, in the same call, so the two views are always exactly as stale as
  // each other and always agree on the lead edge.
  drawSchematic(pairs, worst, doc.cards || [], hero);
}

// Job 5 - one cell per tray. `n` arrives from the PC already summed: this
// loop paints it and performs no arithmetic on it.
function addCells(into, n, cls) {
  for (var i = 0; i < n; i++) {
    var cell = document.createElement("span");
    cell.className = cls ? "rcell " + cls : "rcell";
    into.appendChild(cell);
  }
}

function showNumbers(doc) {
  document.getElementById("overall").textContent = doc.overall ? doc.overall : "";
  // FI-5 - the PC sentence, alone. The old line prefixed `source` onto the
  // message and fell back to the bare key when a source shipped no sentence,
  // so the instrument could print the token "move_due" at the operator. Every
  // source now carries a sentence; `source` stays on the wire for the edge and
  // is never displayed.
  var clash = document.getElementById("clash");
  if (doc.worstClash && doc.worstClash.sentence) {
    clash.textContent = doc.worstClash.sentence;
  } else {
    clash.textContent = "";
  }
  var box = document.getElementById("cards");
  box.textContent = "";
  var list = doc.cards || [];
  for (var i = 0; i < list.length; i++) {
    var row = list[i];
    var card = document.createElement("div");
    var line = document.createElement("p");
    line.className = "row";
    var label = document.createElement("span");
    label.textContent = titleFor(row.card);
    line.appendChild(label);
    if (row.card !== "phone_queue" && row.severity) {
      var sev = document.createElement("span");
      sev.className = "sev";
      sev.textContent = row.severity;
      line.appendChild(sev);
    }
    card.appendChild(line);
    // FI-4b - the face sentence, printed and never assembled. Five cards carry
    // the body of their own worst row, composed on the PC. The queue card
    // prints the same count line the section below prints (Q-1b): one string,
    // read once, shown twice - not two statements of one fact.
    var face = row.card === "phone_queue"
      ? (doc.phoneQueue && doc.phoneQueue.sentence ? doc.phoneQueue.sentence : "")
      : (row.sentence ? row.sentence : "");
    if (face) {
      var say = document.createElement("p");
      say.className = "cface";
      say.textContent = face;
      card.appendChild(say);
    }
    // FI-4b - the age, only where one exists. The PC ships this field only when
    // the card's oldest row is genuinely older than the read, so a card read
    // live carries no line here rather than restating the top line.
    if (row.oldestRanAtDisplay) {
      var age = document.createElement("p");
      age.className = "cage";
      age.textContent = withWhen(CARD_AGE, row.oldestRanAtDisplay);
      card.appendChild(age);
    }
    // Job 5 (SEE-RACK B) - the rack under its own card. Cells come from PC
    // integers, the caption is the PC's sentence, and the ceiling line is
    // constant operator text shown when the PC reports no ceiling. The phone
    // counts nothing, pluralises nothing and compares nothing.
    if (row.card === "rack" && doc.rack) {
      var rk = doc.rack;
      var strip = document.createElement("div");
      strip.className = "rack";
      strip.setAttribute("aria-hidden", "true");
      addCells(strip, rk.light, "lit");
      addCells(strip, rk.blackout, "dark");
      addCells(strip, rk.hollow, "");
      card.appendChild(strip);
      var cap = document.createElement("p");
      cap.className = "rcap";
      cap.textContent = rk.caption ? rk.caption : "";
      card.appendChild(cap);
      if (rk.ceiling === null || rk.ceiling === undefined) {
        var ceil = document.createElement("p");
        ceil.className = "rceil";
        ceil.textContent = RACK_NO_CEILING;
        card.appendChild(ceil);
      }
    }
    box.appendChild(card);
  }
  // FI-9 - the PC composed the whole block. The shell has no formatter: it
  // prints what arrived, so there is no second format to drift.
  document.getElementById("evidence").textContent = doc.diagnosis ? doc.diagnosis : "";
}

function paintOk(doc) {
  held = doc;
  showLine(withWhen(S1, doc.servedAtDisplay));
  showEvaluated(doc);
  showNumbers(doc);
  showTasks(doc);
  showEdges(doc);
  showQueue(doc);
  showPull(doc);
  showCapture(doc);
}

function paintHeld(template) {
  showLine(withWhen(template, held.servedAtDisplay));
  showEvaluated(held);
  showNumbers(held);
  showTasks(held);
  showEdges(held);
  showQueue(held);
  showPull(held);
  showCapture(held);
}

function paintEmpty() {
  showLine(S4);
  clearNumbers();
}

function setPullEnabled(on) {
  document.getElementById("pull").disabled = !on;
}

// FI-2 - the one load path. The first paint, a token submit, Pull now, the
// interval tick and becoming visible all arrive here. One fetch, one 8s
// deadline, one place that can repaint an age. A second door would be a
// second freshness rule.
function load() {
  var token = localStorage.getItem(KEY) || "";
  document.getElementById("token").value = token;
  if (!token) {
    paintEmpty();
    return;
  }
  // A slow pull must not be overtaken by a fast one and repainted with an
  // older servedAt.
  if (inFlight) return;
  inFlight = true;
  setPullEnabled(false);
  var ctrl = new AbortController();
  var timer = setTimeout(function () { ctrl.abort(); }, 8000);
  fetch("/folds", {
    method: "GET",
    headers: { "X-Dock-Token": token },
    signal: ctrl.signal
  }).then(function (res) {
    clearTimeout(timer);
    if (!res.ok) throw new Error("fail");
    return res.json();
  }).then(function (doc) {
    paintOk(doc);
    backoff = 1;
    skipTicks = 0;
  }).catch(function (err) {
    clearTimeout(timer);
    if (held) {
      paintHeld(err && err.name === "AbortError" ? S2 : S3);
    } else {
      paintEmpty();
    }
    skipTicks = backoff;
    backoff = backoff < BACKOFF_MAX ? backoff * 2 : BACKOFF_MAX;
  }).then(function () {
    inFlight = false;
    setPullEnabled(true);
  });
}

// The quiet half. Hidden pages pull nothing, and a dead link is retried less
// and less instead of once a minute forever. Pull now does not come through
// here, so the operator can always override the backoff by asking.
function tick() {
  if (document.visibilityState !== "visible") return;
  if (skipTicks > 0) {
    skipTicks--;
    return;
  }
  load();
}

// The only site that starts a timer, and it always stops one first, so no
// sequence of Remember presses can leave two tickers running.
function arm() {
  if (ticker !== null) clearInterval(ticker);
  ticker = setInterval(tick, TICK_MS);
}

function disarm() {
  if (ticker !== null) {
    clearInterval(ticker);
    ticker = null;
  }
}

// ACT-BAR A - Pull now and Capture something read as one act, two-up and in
// flow. The box is built here and not served, so the document every gate reads
// is byte for byte the document it signed: g8's order, f10a's anchor and
// f11d's button are all untouched. Nothing passes a sibling - the two are
// already adjacent and in this order - so this only draws a box around them.
function wrapActs() {
  var pull = document.getElementById("pull");
  var cap = document.getElementById("capture");
  var bar = document.createElement("div");
  bar.id = "acts";
  pull.parentNode.insertBefore(bar, pull);
  bar.appendChild(pull);
  bar.appendChild(cap);
}

// TOKEN-FOLD A - an unpaired phone opens on the form, because pairing is the
// only thing it can do; a paired phone opens on the numbers and keeps the form
// one tap away. The summary is a disclosure and not a control: it is not a
// a control. It sends nothing, it decides nothing and it touches no farm state.
// The label is the word the Forget token button already ships, capitalised.
var TOKEN_FOLD = "Token";
function wrapToken() {
  var form = document.getElementById("tok");
  var box = document.createElement("details");
  box.id = "tokfold";
  var sum = document.createElement("summary");
  sum.id = "toksum";
  sum.textContent = TOKEN_FOLD;
  form.parentNode.insertBefore(box, form);
  box.appendChild(sum);
  box.appendChild(form);
  box.open = !localStorage.getItem(KEY);
}

document.getElementById("tok").addEventListener("submit", function (e) {
  e.preventDefault();
  var token = document.getElementById("token").value.trim();
  if (token) {
    localStorage.setItem(KEY, token);
    document.getElementById("tokfold").open = false;
  }
  else localStorage.removeItem(KEY);
  backoff = 1;
  skipTicks = 0;
  if (token) arm();
  else disarm();
  load();
});
document.getElementById("forget").addEventListener("click", function () {
  // N-3: a forgotten token stops the pulling as well as the numbers.
  disarm();
  localStorage.removeItem(KEY);
  document.getElementById("token").value = "";
  document.getElementById("tokfold").open = true;
  held = null;
  paintEmpty();
});
document.getElementById("pull").addEventListener("click", function () {
  load();
});
// tick() checks visibility itself, so this fires on both directions and only
// acts on the one that matters.
document.addEventListener("visibilitychange", tick);
// FI-10b - the way back from the capture page, signed as option (b) of the
// return-door ruling. Returning can restore this document from the browser's
// cache instead of reloading it, and pageshow is the event that fires on that
// path. It calls tick, not load: the signed FI-2 backoff still decides whether
// a pull happens, so Back never becomes a second Pull now - the bypass option
// was declined.
window.addEventListener("pageshow", tick);
wrapActs();
wrapToken();
if (localStorage.getItem(KEY)) arm();
load();
</script>
</body>
</html>
"##;
