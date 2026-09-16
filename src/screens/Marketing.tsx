import { useEffect, useState } from "react";
import type {
  AttentionItem,
  Crop,
  FollowupView,
  ReputationCounts,
  ReviewRequestView,
  ScanView,
  StageView,
  StandingPullView,
  VenueView,
  WeeklyActionsView,
} from "@/farm/types";
import {
  capacitySight,
  changeStage,
  checkAttention,
  decideStandingRequest,
  devSeedStandingRequest,
  dropSample,
  gbpVerifiedOn,
  listCrops,
  listOpenFollowups,
  listReviewRequests,
  listSamples,
  listStages,
  listVenues,
  logTouch,
  marketingSummary,
  observeReviews,
  pullScans,
  pullStandingRequests,
  qrLinkForToken,
  standingPullView,
  recordReviewRequest,
  recordVenue,
  correctVenue,
  sampleGateLine,
  reputationCounts,
  resolveFollowup,
  scanConfig,
  scanView,
  setGbpVerified,
  weeklyActions,
} from "@/farm/api";
import { ObservedValue } from "@/components/ObservedValue";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

const STAGE_LADDER = [
  "scouted",
  "sampled",
  "talking",
  "trial",
  "standing",
  "dormant",
  "passed",
] as const;

const ACTIVE_STAGE_ORDER = [
  "standing",
  "trial",
  "talking",
  "sampled",
  "scouted",
] as const;

const PARKED_STAGES = ["dormant", "passed"] as const;

const RECEIVING_NOTES_WARNING =
  "Receiving notes (door, hours, contact person) are strongly recommended.";

const FOLLOWUPS_VISIBLE = 7;
const STAGE_VISIBLE = 7;

function todayLocal(): string {
  const d = new Date();
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

function formatCapacityLine(iso: string, origin: string): string {
  try {
    const dt = new Date(iso);
    const hh = String(dt.getHours()).padStart(2, "0");
    const mm = String(dt.getMinutes()).padStart(2, "0");
    const today = todayLocal();
    const y = dt.getFullYear();
    const m = String(dt.getMonth() + 1).padStart(2, "0");
    const day = String(dt.getDate()).padStart(2, "0");
    const date = `${y}-${m}-${day}`;
    const when = date === today ? `${hh}:${mm} today` : `${date} ${hh}:${mm}`;
    return `Capacity from ${origin}, computed now (${when})`;
  } catch {
    return `Capacity from ${origin}, computed now`;
  }
}

type Mode = "home" | "new-venue" | "drop-sample" | "log-touch";

export function Marketing({ focus = null, onFocusHandled }: { focus?: "new-venue" | null; onFocusHandled?: () => void } = {}) {
  const [ttfso, setTtfso] = useState("No sample dropped yet.");
  const [followups, setFollowups] = useState<FollowupView[]>([]);
  const [venues, setVenues] = useState<VenueView[]>([]);
  const [stages, setStages] = useState<StageView[]>([]);
  const [crops, setCrops] = useState<Crop[]>([]);
  const [weekly, setWeekly] = useState<WeeklyActionsView | null>(null);
  const [capacityLine, setCapacityLine] = useState(
    "Capacity unknown — live farm database not readable",
  );
  const [reputation, setReputation] = useState<ReputationCounts | null>(null);
  const [scans, setScans] = useState<ScanView | null>(null);
  const [gbpOn, setGbpOn] = useState<string>("");
  const [reviewRequests, setReviewRequests] = useState<ReviewRequestView[]>([]);
  const [mode, setMode] = useState<Mode>("home");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [showAllFollowups, setShowAllFollowups] = useState(false);
  const [showAllStages, setShowAllStages] = useState<Record<string, boolean>>({});
  const [pendingStage, setPendingStage] = useState<{
    venueId: string;
    venueName: string;
    stage: string;
  } | null>(null);

  const [venueName, setVenueName] = useState("");
  const [venueType, setVenueType] = useState("restaurant");
  const [venueContact, setVenueContact] = useState("");
  const [venuePhone, setVenuePhone] = useState("");
  const [venueAddress, setVenueAddress] = useState("");
  const [venueNote, setVenueNote] = useState("");
  const [gateLine, setGateLine] = useState("");
  const [fixName, setFixName] = useState("");
  const [fixContact, setFixContact] = useState("");
  const [fixPhone, setFixPhone] = useState("");
  const [fixAddress, setFixAddress] = useState("");
  const [fixNote, setFixNote] = useState("");
  const [lastDrop, setLastDrop] = useState<{
    venueName: string; token: string | null; noteWasEmpty: boolean;
    qrLink: string | null; qrLinkError: string | null;
  } | null>(null);
  const [copied, setCopied] = useState(false);
  const [standingRequests, setStandingRequests] = useState<AttentionItem[]>([]);
  const [selectedVenueId, setSelectedVenueId] = useState<string>("");
  const [packVarieties, setPackVarieties] = useState<string[]>([]);
  const [packCount, setPackCount] = useState(1);
  const [touchChannel, setTouchChannel] = useState("visit");
  const [broadcastBody, setBroadcastBody] = useState("");
  const [broadcastTargets, setBroadcastTargets] = useState<string[]>([]);
  const [reviewCount, setReviewCount] = useState(0);
  const [showParked, setShowParked] = useState(false);
  const [targetEdits, setTargetEdits] = useState<
    Record<string, Record<string, string>>
  >({});
  const [addVariety, setAddVariety] = useState<Record<string, string>>({});
  const [pendingStanding, setPendingStanding] = useState<string | null>(null);
  const [standingPull, setStandingPull] = useState<StandingPullView | null>(null);
  const [scanEndpoint, setScanEndpoint] = useState<string | null>(null);

  async function reload() {
    // Settings fence 1 (S4): the scan endpoint URL + pull token editor left this
    // screen for Settings › Connections. scan_view (the count) still reads here.
    const [summary, open, venueList, cropList, stageList, week, rep, gbp, asks, pulled, gate, attentionItems, pullView] =
      await Promise.all([
        marketingSummary(),
        listOpenFollowups(),
        listVenues(false),
        listCrops(),
        listStages(),
        weeklyActions(),
        reputationCounts(),
        gbpVerifiedOn(),
        listReviewRequests(),
        scanView(),
        sampleGateLine(),
        checkAttention(),
        standingPullView(),
      ]);
    setTtfso(summary.timeToFirstStandingOrder);
    setFollowups(open);
    setVenues(venueList);
    setCrops(cropList);
    setStages(stageList);
    setWeekly(week);
    setReputation(rep);
    setGbpOn(gbp ?? "");
    setReviewRequests(asks);
    setScans(pulled);
    setGateLine(gate);
    setStandingRequests(
      attentionItems
        .filter((i) => i.kind === "marketing.standing_request")
        .sort((a, b) => b.createdAt.localeCompare(a.createdAt)),
    );
    setStandingPull(pullView);
    try {
      const observed = await capacitySight();
      setCapacityLine(
        formatCapacityLine(observed.fetchedAt, observed.origin),
      );
    } catch (e: unknown) {
      setCapacityLine(
        e instanceof Error
          ? e.message
          : "Capacity unknown — live farm database not readable",
      );
    }
    try {
      setScanEndpoint((await scanConfig()).endpointUrl);
    } catch {
      setScanEndpoint(null);
    }
    if (!selectedVenueId && venueList.length > 0) {
      setSelectedVenueId(venueList[0].venueId);
    }
  }

  useEffect(() => {
    void reload().catch((e: unknown) =>
      setError(e instanceof Error ? e.message : String(e)),
    );
  }, []);

    // FIRST-15 — arriving from Today's "Add your first venue": open the form, then release the focus.
    useEffect(() => {
      if (focus !== "new-venue") return;
      setMode("new-venue");
      onFocusHandled?.();
    }, [focus]);   // eslint-disable-line react-hooks/exhaustive-deps

  const today = todayLocal();
  const selectedVenue = venues.find((v) => v.venueId === selectedVenueId) ?? null;
  const needsCompletion = selectedVenue !== null && !selectedVenue.qrReady;
  const noteEmpty = selectedVenue !== null && !(selectedVenue.note ?? "").trim();
  useEffect(() => {
    setFixName(selectedVenue?.name ?? "");
    setFixContact(selectedVenue?.contact ?? "");
    setFixPhone(selectedVenue?.phone ?? "");
    setFixAddress(selectedVenue?.address ?? "");
    setFixNote(selectedVenue?.note ?? "");
  }, [selectedVenueId, venues]);   // eslint-disable-line react-hooks/exhaustive-deps
  function togglePackVariety(name: string) {
    setPackVarieties((prev) =>
      prev.includes(name) ? prev.filter((n) => n !== name) : [...prev, name],
    );
  }
  const overdueCount = followups.filter((f) => f.dueOn <= today).length;
  const sortedFollowups = [...followups].sort((a, b) => {
    const aOver = a.dueOn <= today ? 0 : 1;
    const bOver = b.dueOn <= today ? 0 : 1;
    return aOver - bOver || a.dueOn.localeCompare(b.dueOn);
  });

  async function onRecordVenue() {
    setBusy(true);
    setError(null);
    try {
      const v = await recordVenue({
        name: venueName.trim(),
        venueType: venueType.trim() || "restaurant",
        contact: venueContact.trim() || null,
        phone: venuePhone.trim() || null,
        address: venueAddress.trim() || null,
        note: venueNote.trim() || null,
      });
      setSelectedVenueId(v.venueId);
      setVenueName("");
      setVenueContact("");
      setVenuePhone("");
      setVenueAddress("");
      setVenueNote("");
      setPackVarieties([]);
      setMode("drop-sample");
      await changeStage({
        venueId: v.venueId,
        stage: "scouted",
        changedOn: today,
      });
      await reload();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function onDropSample() {
    if (!selectedVenueId) {
      setError("Pick a venue first.");
      return;
    }
    if (packVarieties.length === 0) {
      setError("Pick at least one variety.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const dropped = await dropSample({
        venueId: selectedVenueId,
        droppedOn: today,
        varieties: packVarieties,
        packCount: packCount > 0 ? packCount : 1,
      });
      let qrLink: string | null = null;
      let qrLinkError: string | null = null;
      if (dropped.token) {
        try {
          qrLink = await qrLinkForToken(dropped.token);
        } catch (e: unknown) {
          qrLinkError = e instanceof Error ? e.message : String(e);
        }
      }
      setCopied(false);
      setLastDrop({
        venueName: dropped.venueName,
        token: dropped.token,
        noteWasEmpty: noteEmpty,
        qrLink,
        qrLinkError,
      });
      await changeStage({
        venueId: selectedVenueId,
        stage: "sampled",
        changedOn: today,
      });
      setMode("home");
      setPackVarieties([]);
      await reload();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function onCompleteVenue() {
    if (!selectedVenue) { setError("Pick a venue first."); return; }
    setBusy(true);
    setError(null);
    try {
      await correctVenue({
        venueId: selectedVenue.venueId,
        name: fixName.trim(),
        venueType: selectedVenue.venueType,
        contact: fixContact.trim() || null,
        phone: fixPhone.trim() || null,
        address: fixAddress.trim() || null,
        note: fixNote.trim() || null,
      });
      await reload();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  async function onDecideRequest(item: AttentionItem, outcome: "accepted" | "dismissed") {
    if (!item.entityId) return;
    setBusy(true);
    setError(null);
    try {
      await decideStandingRequest(item.entityId, outcome);
      await reload();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  async function onDevSeedRequest() {
    setBusy(true);
    setError(null);
    try {
      const samples = await listSamples();
      const newest = samples.find((s) => s.token != null);
      if (!newest || !newest.token) {
        setError("Drop a sample first — the seed needs a token.");
        return;
      }
      await devSeedStandingRequest(newest.token, 2);
      await reload();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  async function onPullStandingRequests() {
    setBusy(true);
    setError(null);
    try {
      await pullStandingRequests();
      await reload();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  async function onCopyQrLink(url: string) {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function onLogTouch() {
    if (!selectedVenueId) {
      setError("Pick a venue first.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await logTouch({
        venueId: selectedVenueId,
        touchedOn: today,
        channel: touchChannel,
      });
      setMode("home");
      await reload();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function onResolve(f: FollowupView) {
    if (!f.attentionId) return;
    setBusy(true);
    setError(null);
    try {
      await resolveFollowup({
        attentionId: f.attentionId,
        channel: "visit",
      });
      await reload();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function onChangeStage(venueId: string, stage: string) {
    setBusy(true);
    setError(null);
    try {
      await changeStage({
        venueId,
        stage,
        changedOn: today,
      });
      await reload();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function onSaveStandingTargets(venueId: string) {
    const stage = stages.find((st) => st.venueId === venueId);
    const fromStage = stage
      ? stage.varietyTargets
        ? Object.fromEntries(
            Object.entries(stage.varietyTargets).map(([k, v]) => [
              k,
              String(v),
            ]),
          )
        : Object.fromEntries((stage.varieties ?? []).map((n) => [n, ""]))
      : {};
    const edits = targetEdits[venueId] ?? fromStage;
    const entries = Object.entries(edits);
    if (entries.length === 0) {
      setError("Add at least one variety.");
      return;
    }
    const targets: Record<string, number> = {};
    for (const [name, raw] of entries) {
      const n = Number(raw.trim());
      if (!Number.isInteger(n) || n < 1) {
        setError(`${name}: trays/week must be a whole number, 1 or more.`);
        return;
      }
      targets[name] = n;
    }
    const total = Object.values(targets).reduce((a, b) => a + b, 0);
    setBusy(true);
    setError(null);
    try {
      await changeStage({
        venueId,
        stage: "standing",
        changedOn: today,
        traysWeek: total,
        varieties: Object.keys(targets),
        varietyTargets: targets,
      });
      setPendingStanding(null);
      setTargetEdits((prev) => {
        const next = { ...prev };
        delete next[venueId];
        return next;
      });
      await reload();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function onBroadcastSent(venueId: string) {
    setBusy(true);
    setError(null);
    try {
      await logTouch({
        venueId,
        touchedOn: today,
        channel: "text",
        note: broadcastBody.trim() || "broadcast sent",
      });
      setBroadcastTargets((prev) => prev.filter((id) => id !== venueId));
      await reload();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function onSaveReviews() {
    setBusy(true);
    setError(null);
    try {
      await observeReviews({
        observedOn: today,
        count: reviewCount,
        source: "google_business_profile",
      });
      await reload();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function onPullScans() {
    setBusy(true);
    setError(null);
    try {
      await pullScans();
      await reload();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function onSaveGbp() {
    setBusy(true);
    setError(null);
    try {
      await setGbpVerified(gbpOn.trim() ? gbpOn.trim() : null);
      await reload();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function onReviewDecision(venueId: string, outcome: "asked" | "skipped") {
    setBusy(true);
    setError(null);
    try {
      await recordReviewRequest({ venueId, outcome });
      await reload();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  const venuesWithoutStage = venues.filter(
    (v) => !stages.some((s) => s.venueId === v.venueId),
  );

  // GT-D15 + ruling C: one door. The ask list IS weekly_actions' review_ask
  // entries. A filter in this file is how the GBP gate, the missing trial arm
  // and the missing delivery clause survived the backend correction in 9d4ef8e.
  const reviewAsks = (weekly?.actions ?? []).flatMap((a) =>
    a.kind === "review_ask" && a.venueId !== null
      ? [{ venueId: a.venueId, title: a.title }]
      : [],
  );

    // FIRST-15 — no scan endpoint and nothing waiting: the card folds to one line after Pipeline.
    const standingCollapsed = scanEndpoint === null && standingRequests.length === 0;

  function stageRows(stage: string): StageView[] {
    const inStage = stages.filter((s) => s.stage === stage);
    const unstaged = stage === "scouted" ? venuesWithoutStage : [];
    return [
      ...inStage,
      ...unstaged.map((v) => ({
        venueId: v.venueId,
        venueName: v.name,
        stage: "scouted",
        traysWeek: null as number | null,
        varieties: null as string[] | null,
        varietyTargets: null as Record<string, number> | null,
        changedOn: "",
        note: null as string | null,
        updatedAt: "",
      })),
    ];
  }

  const parkedCount = PARKED_STAGES.reduce(
    (n, st) => n + stageRows(st).length,
    0,
  );
  const emptyActive = ACTIVE_STAGE_ORDER.filter(
    (st) => stageRows(st).length === 0,
  );

  function editorTargets(s: StageView): Record<string, string> {
    if (targetEdits[s.venueId] !== undefined) return targetEdits[s.venueId];
    if (s.varietyTargets) {
      return Object.fromEntries(
        Object.entries(s.varietyTargets).map(([k, v]) => [k, String(v)]),
      );
    }
    if (s.varieties?.length) {
      return Object.fromEntries(s.varieties.map((n) => [n, ""]));
    }
    return {};
  }

  function renderStageSection(stage: string) {
    const rows = stageRows(stage);
    const expanded = showAllStages[stage] === true;
    const shown = expanded ? rows : rows.slice(0, STAGE_VISIBLE);
    return (
      <div key={stage} className="flex flex-col gap-2">
        <p className="text-sm font-medium">
          {stage} ({rows.length})
        </p>
        <ul className="flex flex-col gap-2">
          {shown.map((s) => {
            const showEditor =
              stage === "standing" || pendingStanding === s.venueId;
            const edits = editorTargets(s);
            const listed = Object.keys(edits);
            const canSave =
              listed.length > 0 &&
              listed.every((name) => {
                const n = Number((edits[name] ?? "").trim());
                return Number.isInteger(n) && n >= 1;
              });
            const available = crops.filter((c) => !listed.includes(c.name));
            const splitLine =
              s.varietyTargets &&
              Object.entries(s.varietyTargets)
                .map(([name, n]) => `${name} ${n}`)
                .join(" · ");
            return (
              <li
                key={`${stage}-${s.venueId}`}
                className="flex flex-col gap-1 border-b border-border pb-2"
              >
                <p className="text-sm">
                  {s.venueName}
                  {stage === "standing" && s.traysWeek != null
                    ? ` · ${s.traysWeek}/week`
                    : ""}
                </p>
                {stage === "standing" && splitLine ? (
                  <p className="text-sm text-muted-foreground">{splitLine}</p>
                ) : null}
                {showEditor && (
                  <div className="flex flex-col gap-2">
                    {stage === "standing" && s.varietyTargets == null && (
                      <p className="text-sm text-amber-700">
                        Re-state by variety — currently {s.traysWeek}/week
                        total, not yet split.
                      </p>
                    )}
                    {listed.map((name) => (
                      <div
                        key={name}
                        className="flex items-center gap-2"
                      >
                        <label className="flex items-center gap-2 text-sm text-muted-foreground">
                          {name}
                          <input
                            type="number"
                            min={1}
                            className="w-20 border border-border bg-card px-2 py-1 text-sm text-foreground"
                            value={edits[name] ?? ""}
                            onChange={(e) => {
                              const current = editorTargets(s);
                              setTargetEdits((prev) => ({
                                ...prev,
                                [s.venueId]: {
                                  ...current,
                                  [name]: e.target.value,
                                },
                              }));
                            }}
                          />
                        </label>
                        <button
                          type="button"
                          className="text-sm text-muted-foreground underline-offset-4 hover:underline active:translate-y-px"
                          onClick={() => {
                            const current = { ...editorTargets(s) };
                            delete current[name];
                            setTargetEdits((prev) => ({
                              ...prev,
                              [s.venueId]: current,
                            }));
                          }}
                        >
                          Remove
                        </button>
                      </div>
                    ))}
                    <select
                      className="self-start border border-border bg-card px-2 py-1 text-sm"
                      value={addVariety[s.venueId] ?? ""}
                      disabled={busy}
                      onChange={(e) => {
                        const name = e.target.value;
                        if (!name) return;
                        const current = editorTargets(s);
                        setTargetEdits((prev) => ({
                          ...prev,
                          [s.venueId]: { ...current, [name]: "" },
                        }));
                        setAddVariety((prev) => ({
                          ...prev,
                          [s.venueId]: "",
                        }));
                      }}
                    >
                      <option value="">Add variety</option>
                      {available.map((c) => (
                        <option key={c.id} value={c.name}>
                          {c.name}
                        </option>
                      ))}
                    </select>
                    <div className="flex gap-2">
                      <Button
                        type="button"
                        variant="outline"
                        className="h-9 px-3 text-sm active:translate-y-px"
                        disabled={busy || !canSave}
                        onClick={() => void onSaveStandingTargets(s.venueId)}
                      >
                        Save
                      </Button>
                      {pendingStanding === s.venueId && (
                        <Button
                          type="button"
                          variant="ghost"
                          className="h-9 px-3 text-sm active:translate-y-px"
                          disabled={busy}
                          onClick={() => {
                            setPendingStanding(null);
                            setTargetEdits((prev) => {
                              const next = { ...prev };
                              delete next[s.venueId];
                              return next;
                            });
                          }}
                        >
                          Cancel
                        </Button>
                      )}
                    </div>
                  </div>
                )}
                <select
                  className="border border-border bg-card px-2 py-1 text-sm"
                  value={
                    pendingStanding === s.venueId ? "standing" : s.stage
                  }
                  disabled={busy}
                  onChange={(e) => {
                    const chosen = e.target.value;
                    if (chosen === "passed" || chosen === "dormant") {
                      setPendingStage({
                        venueId: s.venueId,
                        venueName: s.venueName,
                        stage: chosen,
                      });
                      return;
                    }
                    setPendingStage(null);
                    if (chosen === "standing" && s.stage !== "standing") {
                      setPendingStanding(s.venueId);
                      setTargetEdits((prev) => ({
                        ...prev,
                        [s.venueId]: Object.fromEntries(
                          (s.varieties ?? []).map((n) => [n, ""]),
                        ),
                      }));
                      return;
                    }
                    setPendingStanding(null);
                    void onChangeStage(s.venueId, chosen);
                  }}
                >
                  {STAGE_LADDER.map((opt) => (
                    <option key={opt} value={opt}>
                      {opt}
                    </option>
                  ))}
                </select>
                {pendingStage?.venueId === s.venueId && (
                  <>
                    <p className="text-sm">
                      Mark {pendingStage.venueName} as {pendingStage.stage}?
                    </p>
                    <div className="flex gap-2">
                      <Button
                        type="button"
                        className="h-11 active:translate-y-px"
                        disabled={busy}
                        onClick={() => {
                          void onChangeStage(
                            pendingStage.venueId,
                            pendingStage.stage,
                          );
                          setPendingStage(null);
                        }}
                      >
                        {pendingStage.stage === "passed"
                          ? "Mark passed"
                          : "Mark dormant"}
                      </Button>
                      <Button
                        type="button"
                        variant="ghost"
                        className="h-11 active:translate-y-px"
                        disabled={busy}
                        onClick={() => setPendingStage(null)}
                      >
                        Cancel
                      </Button>
                    </div>
                  </>
                )}
              </li>
            );
          })}
        </ul>
        {!expanded && rows.length > STAGE_VISIBLE && (
          <button
            type="button"
            onClick={() =>
              setShowAllStages((prev) => ({ ...prev, [stage]: true }))
            }
            className="self-start text-sm text-muted-foreground underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none active:translate-y-px"
          >
            Show the rest ({rows.length - STAGE_VISIBLE})
          </button>
        )}
      </div>
    );
  }

  return (
    <main className="mx-auto flex w-full max-w-md flex-col gap-8 px-6 py-8">
      <h1 className="text-2xl font-semibold tracking-tight">Marketing</h1>
      {/* Marketing was the only tab without a page title, and the TtFSO sentence
          (marketing.rs:1837-1867, via MarketingSummary.timeToFirstStandingOrder)
          was the largest element on the page. The title and this line restore the
          h1 -> muted blurb -> headline fact order that Money already carries
          (Money.tsx:1172-1178). The blurb names this tab's job as the signed
          surface audit states it — "next relationship action (venue, sample,
          standing, review)" — and invents no next-action system: that redesign
          is deferred. LIVELY MARKETING stepped the TtFSO line to the hero rung. */}
      <p className="text-sm text-muted-foreground">
        Venues, samples, standing orders and reviews — the relationship work behind
        the money.
      </p>
      <p className="text-3xl font-medium tabular-nums text-foreground">{venues.length > 0 ? ttfso : "Add a venue, then drop a sample — the clock to your first standing order starts there."}</p>
      {!standingCollapsed && (
      <Card>
        <CardHeader>
          <CardTitle>
            {standingRequests.length > 0
              ? `Standing requests (${standingRequests.length})`
              : "Standing requests"}
          </CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {standingRequests.length === 0 ? (
            <p className="text-sm text-muted-foreground">No standing requests waiting.</p>
          ) : (
            <ul className="flex flex-col gap-3">
              {standingRequests.map((item) => (
                <li key={item.id} className="flex flex-col gap-2 border-b border-border pb-3">
                  <p className="text-sm">{item.message}</p>
                  <div className="flex gap-2">
                    <Button
                      type="button"
                      className="h-11 px-4 active:translate-y-px"
                      disabled={busy}
                      onClick={() => void onDecideRequest(item, "accepted")}
                    >
                      Accept
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      className="h-11 px-4 active:translate-y-px"
                      disabled={busy}
                      onClick={() => void onDecideRequest(item, "dismissed")}
                    >
                      Dismiss
                    </Button>
                  </div>
                </li>
              ))}
            </ul>
          )}
          {standingPull && (
            <div className="flex flex-col gap-1">
              <p className="text-sm text-muted-foreground">{standingPull.message}</p>
              {standingPull.lastOkMessage && (
                <p className="text-sm text-muted-foreground">{standingPull.lastOkMessage}</p>
              )}
              {standingPull.refusalMessage && (
                <p className="text-sm text-amber-700">{standingPull.refusalMessage}</p>
              )}
              {standingPull.gapMessage && (
                <p className="text-sm text-amber-700">{standingPull.gapMessage}</p>
              )}
            </div>
          )}
          <Button
            type="button"
            variant="outline"
            className="self-start active:translate-y-px"
            disabled={busy}
            onClick={() => void onPullStandingRequests()}
          >
            Pull standing requests
          </Button>
          {import.meta.env.DEV && (
            <Button
              type="button"
              variant="outline"
              className="self-start text-sm active:translate-y-px"
              disabled={busy}
              onClick={() => void onDevSeedRequest()}
            >
              Dev: seed a standing request
            </Button>
          )}
        </CardContent>
      </Card>
      )}

      <Card>
        <CardHeader>
          <CardTitle>
            {overdueCount > 0
              ? `Samples & follow-ups — ${overdueCount} overdue`
              : "Samples & follow-ups"}
          </CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">

          {followups.length === 0 ? (
            <p className="text-sm text-muted-foreground">No open follow-ups.</p>
          ) : (
            <ul className="flex flex-col gap-3">
              {(showAllFollowups
                ? sortedFollowups
                : sortedFollowups.slice(0, FOLLOWUPS_VISIBLE)
              ).map((f) => {
                const overdue = f.dueOn <= today;
                return (
                  <li
                    key={f.followupId}
                    className="flex flex-col gap-2 border-b border-border pb-3"
                  >
                    <p className="text-sm">
                      <span className="font-medium">{f.venueName}</span>
                      {" · "}
                      {overdue ? (
                        <span className="text-amber-700">overdue {f.dueOn}</span>
                      ) : (
                        <>due {f.dueOn}</>
                      )}
                    </p>
                    <p className="text-sm text-muted-foreground">{f.what}</p>
                    {f.attentionId && (
                      <Button
                        type="button"
                        variant="outline"
                        disabled={busy}
                        onClick={() => void onResolve(f)}
                      >
                        Logged visit
                      </Button>
                    )}
                  </li>
                );
              })}
              {!showAllFollowups &&
                sortedFollowups.length > FOLLOWUPS_VISIBLE && (
                  <li>
                    <button
                      type="button"
                      onClick={() => setShowAllFollowups(true)}
                      className="px-2 py-3 text-left text-sm text-muted-foreground underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none active:translate-y-px"
                    >
                      Show later follow-ups (
                      {sortedFollowups.length - FOLLOWUPS_VISIBLE})
                    </button>
                  </li>
                )}
            </ul>
          )}

          {mode === "home" && (
            <div className="flex flex-col gap-2 pt-2">
              {lastDrop && (
                <div className="flex flex-col gap-1">
                  <p className="text-sm">
                    Sample dropped at {lastDrop.venueName} · token {lastDrop.token ?? "none"}
                  </p>
                  {lastDrop.qrLink && (
                    <div className="flex flex-col gap-1">
                      <p className="text-sm break-all">QR link: {lastDrop.qrLink}</p>
                      <Button
                        type="button"
                        variant="outline"
                        className="self-start text-sm active:translate-y-px"
                        onClick={() => {
                          const url = lastDrop.qrLink;
                          if (url) void onCopyQrLink(url);
                        }}
                      >
                        {copied ? "Copied" : "Copy"}
                      </Button>
                    </div>
                  )}
                  {lastDrop.qrLinkError && (
                    <p className="text-sm text-amber-700">{lastDrop.qrLinkError}</p>
                  )}
                  {lastDrop.noteWasEmpty && (
                    <p className="text-sm text-amber-700">{RECEIVING_NOTES_WARNING}</p>
                  )}
                </div>
              )}
              {venues.length === 0 ? (
                <>
                  <Button type="button" disabled={busy} onClick={() => setMode("new-venue")}>
                    Add a venue
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    disabled={busy}
                    onClick={() => {
                      setPackVarieties([]);
                      setMode("drop-sample");
                    }}
                  >
                    Log sample drop
                  </Button>
                  <Button type="button" variant="ghost" disabled={busy} onClick={() => setMode("log-touch")}>
                    Log touch
                  </Button>
                </>
              ) : (
                <>
              <Button
                type="button"
                disabled={busy}
                onClick={() => {
                  setPackVarieties([]);
                  setMode("drop-sample");
                }}
              >
                Log sample drop
              </Button>
              <Button
                type="button"
                variant="outline"
                disabled={busy}
                onClick={() => setMode("log-touch")}
              >
                Log touch
              </Button>
              <Button
                type="button"
                variant="ghost"
                disabled={busy}
                onClick={() => setMode("new-venue")}
              >
                New venue
              </Button>
                </>
              )}
            </div>
          )}

          {mode === "new-venue" && (
            <div className="flex flex-col gap-3">
              <label className="flex flex-col gap-1 text-sm">
                Name
                <input
                  className="border border-border bg-card px-3 py-2"
                  value={venueName}
                  onChange={(e) => setVenueName(e.target.value)}
                  placeholder="Venue name"
                  autoFocus
                />
              </label>
              <label className="flex flex-col gap-1 text-sm">
                Type
                <select
                  className="border border-border bg-card px-3 py-2"
                  value={venueType}
                  onChange={(e) => setVenueType(e.target.value)}
                >
                  <option value="restaurant">restaurant</option>
                  <option value="cafe">cafe</option>
                  <option value="grocer">grocer</option>
                  <option value="other">other</option>
                </select>
              </label>
              <label className="flex flex-col gap-1 text-sm">
                Contact
                <input
                  className="border border-border bg-card px-3 py-2"
                  value={venueContact}
                  onChange={(e) => setVenueContact(e.target.value)}
                  placeholder="Who to ask for"
                />
              </label>
              <label className="flex flex-col gap-1 text-sm">
                Phone
                <input
                  inputMode="tel"
                  className="border border-border bg-card px-3 py-2"
                  value={venuePhone}
                  onChange={(e) => setVenuePhone(e.target.value)}
                  placeholder="Phone"
                />
              </label>
              <label className="flex flex-col gap-1 text-sm">
                Address
                <input
                  className="border border-border bg-card px-3 py-2"
                  value={venueAddress}
                  onChange={(e) => setVenueAddress(e.target.value)}
                  placeholder="Address"
                />
              </label>
              <label className="flex flex-col gap-1 text-sm">
                Note
                <input
                  className="border border-border bg-card px-3 py-2"
                  value={venueNote}
                  onChange={(e) => setVenueNote(e.target.value)}
                  placeholder="Anything worth remembering"
                />
              </label>
              <div className="flex gap-2">
                <Button type="button" disabled={busy || !venueName.trim()} onClick={() => void onRecordVenue()}>
                  Save venue
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  onClick={() => setMode("home")}
                >
                  Cancel
                </Button>
              </div>
            </div>
          )}

          {mode === "drop-sample" && (
            <div className="flex flex-col gap-3">
              <label className="flex flex-col gap-1 text-sm">
                Venue
                <select
                  className="border border-border bg-card px-3 py-2"
                  value={selectedVenueId}
                  onChange={(e) => setSelectedVenueId(e.target.value)}
                >
                  {venues.length === 0 && (
                    <option value="">No venues yet</option>
                  )}
                  {venues.map((v) => (
                    <option key={v.venueId} value={v.venueId}>
                      {v.name}
                    </option>
                  ))}
                </select>
              </label>
              {needsCompletion && selectedVenue && (
                <div className="flex flex-col gap-2 border border-amber-700 p-3">
                  <p className="text-sm text-amber-700">{gateLine}</p>
                  <label className="flex flex-col gap-1 text-sm">
                    Name
                    <input className="border border-border bg-card px-3 py-2"
                      value={fixName} onChange={(e) => setFixName(e.target.value)} />
                  </label>
                  <label className="flex flex-col gap-1 text-sm">
                    Contact
                    <input className="border border-border bg-card px-3 py-2"
                      value={fixContact} onChange={(e) => setFixContact(e.target.value)}
                      placeholder="Who to ask for" />
                  </label>
                  <label className="flex flex-col gap-1 text-sm">
                    Phone
                    <input inputMode="tel" className="border border-border bg-card px-3 py-2"
                      value={fixPhone} onChange={(e) => setFixPhone(e.target.value)} placeholder="Phone" />
                  </label>
                  <label className="flex flex-col gap-1 text-sm">
                    Address
                    <input className="border border-border bg-card px-3 py-2"
                      value={fixAddress} onChange={(e) => setFixAddress(e.target.value)}
                      placeholder="Delivery address" />
                  </label>
                  <label className="flex flex-col gap-1 text-sm">
                    Note
                    <input className="border border-border bg-card px-3 py-2"
                      value={fixNote} onChange={(e) => setFixNote(e.target.value)}
                      placeholder="Receiving notes (door, hours, contact person)" />
                  </label>
                  <Button type="button" variant="outline" disabled={busy || !fixName.trim()}
                    onClick={() => void onCompleteVenue()}>
                    Save venue details
                  </Button>
                </div>
              )}
              {noteEmpty && (
                <p className="text-sm text-amber-700">{RECEIVING_NOTES_WARNING}</p>
              )}
              <p className="text-sm text-muted-foreground">Pack date {today}</p>
              <fieldset className="flex flex-col gap-1 text-sm">
                <legend className="text-sm">Varieties in the pack</legend>
                {crops.map((c) => (
                  <label key={c.id} className="flex items-center gap-2">
                    <input
                      type="checkbox"
                      checked={packVarieties.includes(c.name)}
                      onChange={() => togglePackVariety(c.name)}
                    />
                    {c.name}
                  </label>
                ))}
              </fieldset>
              <label className="flex flex-col gap-1 text-sm">
                Packs
                <input
                  type="number"
                  min={1}
                  className="border border-border bg-card px-3 py-2"
                  value={packCount}
                  onChange={(e) => setPackCount(Number(e.target.value) || 1)}
                />
              </label>
              <div className="flex gap-2">
                <Button
                  type="button"
                  disabled={busy || !selectedVenueId || packVarieties.length === 0 || needsCompletion}
                  onClick={() => void onDropSample()}
                >
                  Save drop
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  onClick={() => setMode("home")}
                >
                  Cancel
                </Button>
              </div>
              {venues.length === 0 && (
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => setMode("new-venue")}
                >
                  Add a venue first
                </Button>
              )}
            </div>
          )}

          {mode === "log-touch" && (
            <div className="flex flex-col gap-3">
              <label className="flex flex-col gap-1 text-sm">
                Venue
                <select
                  className="border border-border bg-card px-3 py-2"
                  value={selectedVenueId}
                  onChange={(e) => setSelectedVenueId(e.target.value)}
                >
                  {venues.map((v) => (
                    <option key={v.venueId} value={v.venueId}>
                      {v.name}
                    </option>
                  ))}
                </select>
              </label>
              <label className="flex flex-col gap-1 text-sm">
                Channel
                <select
                  className="border border-border bg-card px-3 py-2"
                  value={touchChannel}
                  onChange={(e) => setTouchChannel(e.target.value)}
                >
                  {["visit", "call", "text", "email", "ig"].map((c) => (
                    <option key={c} value={c}>
                      {c}
                    </option>
                  ))}
                </select>
              </label>
              <div className="flex gap-2">
                <Button
                  type="button"
                  disabled={busy || !selectedVenueId}
                  onClick={() => void onLogTouch()}
                >
                  Save touch
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  onClick={() => setMode("home")}
                >
                  Cancel
                </Button>
              </div>
            </div>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Weekly actions</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <p className="text-sm text-muted-foreground">{capacityLine}</p>
          {weekly?.pitchingAdvice != null && (
            <p
              className={
                weekly.pitchingAdvice.startsWith("Hold pitching")
                  ? "text-sm font-medium text-amber-700"
                  : "text-sm font-medium"
              }
            >
              {weekly.pitchingAdvice}
            </p>
          )}
          {weekly && weekly.actions.length === 0 ? (
            <p className="text-sm text-muted-foreground">No actions this week.</p>
          ) : (
            <ul className="flex flex-col gap-2">
              {weekly?.actions.map((a, i) => (
                <li key={`${a.kind}-${i}`} className="text-sm border-b border-border pb-2">
                  {a.title}
                </li>
              ))}
            </ul>
          )}
          {weekly && weekly.dropped > 0 && (
            <p className="text-sm text-muted-foreground">
              {weekly.dropped} more action{weekly.dropped === 1 ? "" : "s"} not
              shown — cap reached.
            </p>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Pipeline</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {ACTIVE_STAGE_ORDER.filter((st) => stageRows(st).length > 0).map(
            (stage) => renderStageSection(stage),
          )}
          {parkedCount > 0 && (
            <button
              type="button"
              onClick={() => setShowParked((v) => !v)}
              className="self-start text-sm text-muted-foreground underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none active:translate-y-px"
            >
              {showParked
                ? "Hide dormant & passed"
                : `Show dormant & passed (${parkedCount})`}
            </button>
          )}
          {showParked &&
            PARKED_STAGES.filter((st) => stageRows(st).length > 0).map((stage) =>
              renderStageSection(stage),
            )}
          {emptyActive.length > 0 && (
            <p className="text-sm text-muted-foreground">
              No venues in {emptyActive.join(", ")}.
            </p>
          )}
        </CardContent>
      </Card>
      {standingCollapsed && (
        <p className="text-sm text-muted-foreground">
          Standing requests arrive through a scan endpoint (Settings › Connections).
        </p>
      )}

      <Card>
        <CardHeader>
          <CardTitle>Broadcast drafts</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <p className="text-sm text-muted-foreground">
            Compose once. Send by hand from your phone, then tap sent.
          </p>
          <textarea
            className="min-h-24 border border-border bg-background px-3 py-2 text-sm"
            value={broadcastBody}
            onChange={(e) => setBroadcastBody(e.target.value)}
            placeholder="Draft message"
          />
          <Button
            type="button"
            variant="outline"
            disabled={busy || venues.length === 0}
            onClick={() => setBroadcastTargets(venues.map((v) => v.venueId))}
          >
            Checklist all venues
          </Button>
          <ul className="flex flex-col gap-2">
            {venues
              .filter((v) => broadcastTargets.includes(v.venueId))
              .map((v) => (
                <li
                  key={v.venueId}
                  className="flex items-center justify-between gap-2 border-b border-border pb-2"
                >
                  <span className="text-sm">{v.name}</span>
                  <Button
                    type="button"
                    variant="outline"
                    disabled={busy}
                    onClick={() => void onBroadcastSent(v.venueId)}
                  >
                    Sent
                  </Button>
                </li>
              ))}
          </ul>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Reputation</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {reputation && (
            <ul className="flex flex-col gap-1 text-sm">
              <li className="border-b border-border pb-2">Active warm venues: {reputation.activeWarmVenues}</li>
              <li className="border-b border-border pb-2">Samples outstanding: {reputation.samplesOutstanding}</li>
              <li className="border-b border-border pb-2">
                Standing orders: {reputation.standingOrders} (
                {reputation.standingTraysWeek} trays/week)
              </li>
              <li className="border-b border-border pb-2">Overdue follow-ups: {reputation.overdueFollowups}</li>
              <li className="border-b border-border pb-2">
                Google reviews:{" "}
                {reputation.googleReviewCount == null
                  ? "none recorded"
                  : `${reputation.googleReviewCount} (observed ${reputation.googleReviewObservedOn})`}
              </li>
            </ul>
          )}
          {scans && (
            <div className="flex flex-col gap-2">
              <p>{scans.message}</p>
              {scans.count != null && (
                <>
                  <ObservedValue observed={scans.count}>
                    {(n) => <span>{n}</span>}
                  </ObservedValue>
                  <p className="text-sm text-muted-foreground">{scans.count.origin}</p>
                </>
              )}
              {scans.gapMessage && (
                <p className="text-sm text-amber-700">{scans.gapMessage}</p>
              )}
              <p className="text-sm text-muted-foreground">{scans.printedNote}</p>
              <Button
                type="button"
                variant="outline"
                disabled={busy}
                onClick={() => void onPullScans()}
              >
                Pull scan count
              </Button>
            </div>
          )}
          <label className="flex flex-col gap-1 text-sm">
            Google Business Profile verified on
            <input
              type="date"
              className="border border-border bg-card px-3 py-2"
              value={gbpOn}
              onChange={(e) => setGbpOn(e.target.value)}
            />
          </label>
          <Button
            type="button"
            variant="outline"
            disabled={busy}
            onClick={() => void onSaveGbp()}
          >
            Save verified date
          </Button>
          {reviewAsks.length > 0 && (
            <ul className="flex flex-col gap-2">
              {reviewAsks.map((a) => (
                <li
                  key={a.venueId}
                  className="flex items-center justify-between gap-2 border-b border-border pb-2"
                >
                  <span className="text-sm">{a.title}</span>
                  <span className="flex gap-2">
                    <Button
                      type="button"
                      variant="outline"
                      disabled={busy}
                      onClick={() => void onReviewDecision(a.venueId, "asked")}
                    >
                      Asked
                    </Button>
                    <Button
                      type="button"
                      variant="outline"
                      disabled={busy}
                      onClick={() => void onReviewDecision(a.venueId, "skipped")}
                    >
                      Skip
                    </Button>
                  </span>
                </li>
              ))}
            </ul>
          )}
          {reviewRequests.length > 0 && (
            <ul className="flex flex-col gap-1 text-sm">
              {reviewRequests.map((r) => (
                <li key={r.venueId} className="border-b border-border pb-2">
                  {r.venueName}: {r.outcome} on {r.decidedOn}
                </li>
              ))}
            </ul>
          )}
          <label className="flex flex-col gap-1 text-sm">
            Record Google review count
            <input
              type="number"
              min={0}
              className="border border-border bg-card px-3 py-2"
              value={reviewCount}
              onChange={(e) => setReviewCount(Number(e.target.value) || 0)}
            />
          </label>
          <Button
            type="button"
            variant="outline"
            disabled={busy}
            onClick={() => void onSaveReviews()}
          >
            Save observation
          </Button>
        </CardContent>
      </Card>

      {error && <p role="alert" className="text-sm text-red-700">{error}</p>}
    </main>
  );
}
