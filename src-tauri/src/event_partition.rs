//! Closed sets for the event_log spine.
//!
//! Single source of truth for kinds, tiers, and Phase 2/9 trigger SQL.
//! Authority: `docs/track-1-inventory.md` § Phase 2 kind partition.
//! BOOKS-BOUNDARY outranks ROADMAP; the eight register classes are fixed there.
//! The eighth class (`money_in`) was admitted by an amendment to BOOKS-BOUNDARY
//! for the money-in track.
//!
//! The kind determines the tier. Callers do not choose `event_domain` /
//! `event_class` — `Kind::tier` is total over every variant.

/// Closed set of event_log.kind values. Exhaustive match in `Kind::tier` and
/// in `projection::apply_event`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    TraySown,
    TraysAdvanced,
    TraysHarvested,
    TrayDiscarded,
    TraysDiscarded,
    RecountApplied,
    Undo,
    DevBackdated,
    AttentionResolved,
    StripeSessionPaid,
    StripeRefunded,
    StripeDisputed,
    SnapshotTaken,
    /// Money left the bank — Farm OS origin cost event (Track 3).
    CostMoneyOut,
    /// Full replacement of a cost's operator fields.
    CostMoneyOutCorrected,
    /// Retires a cost entered in error. Row survives, marked voided.
    CostMoneyOutVoided,
    /// Physical consumption in units only — Farm OS origin (Track 4).
    ConsumptionPhysical,
    /// One dated trip, stored in miles (Track 4 residual).
    MileageTripLogged,
    /// Full replacement of a trip's operator fields.
    MileageTripCorrected,
    /// Retires a trip that never happened. Row survives, marked voided.
    MileageTripVoided,
    /// One asset, four operator fields, no computation (Track 4 residual).
    AssetRecorded,
    /// Full replacement of an asset's four operator fields.
    AssetCorrected,
    /// Retires an asset entered in error. Row survives, marked voided.
    AssetVoided,
    /// Money arrived. Farm OS origin (money-in track).
    IncomeReceived,
    /// Full replacement of a record's operator fields.
    IncomeCorrected,
    /// Retires a record entered in error. Row survives, marked voided.
    IncomeVoided,
    /// Marketing — venue first seen (GT-D1).
    VenueRecorded,
    /// Marketing — venue fields replaced.
    VenueCorrected,
    /// Marketing — venue retired from the active list.
    VenueArchived,
    /// Marketing — sample pack handed to a venue.
    SampleDropped,
    /// Marketing — a contact touch logged.
    TouchLogged,
    /// Marketing — a follow-up scheduled.
    FollowupSet,
    /// Marketing — a follow-up cleared.
    FollowupCleared,
    /// Marketing — pipeline stage changed (GT-D11).
    StageChanged,
    /// Marketing — operator-entered review count observation (GT-D11).
    ReviewsObserved,
    /// Marketing — a review ask decided for one venue (GT-D15).
    ReviewRequested,
    /// Wholesale order book — trays committed (GT-D14).
    WholesaleOrdered,
    /// Wholesale order book — delivered, unpaid.
    WholesaleDelivered,
    /// Wholesale order book — paid; points at income.received.
    WholesalePaid,
    /// Wholesale order book — voided from ordered or delivered only.
    WholesaleVoided,
    /// Wholesale order book — the recorded shortfall when an order is settled
    /// for less than its priced total. An allowance, never a return.
    WholesaleWriteOff,
    /// Wholesale order book — payment reversed; paid → delivered, income voided.
    WholesalePaymentReversed,
    /// Wholesale order book — a delivered, unpaid obligation the operator
    /// declares uncollectable. Clears the debt; never a payment, never a
    /// void: the delivery stands and the trays stay committed.
    WholesaleBadDebt,
    /// TILL-A (GT-D22) — a Stripe Payment Link minted for one delivered,
    /// priced wholesale order. Records the link id and the URL the operator
    /// shows; moves no money and no capacity. A second mint on a row is refused.
    WholesaleLinkMinted,
    /// A Stripe fact the register observed and could not apply to the
    /// order book. Durable trace only — never changes an order.
    StripeFactUnapplied,
    /// Marketing — a chef's standing-request candidate observed from the scan
    /// endpoint (GT-D17). Candidate only: it never changes standing; the accept
    /// path writes stage.changed. Money-free by tier.
    StandingRequested,
    /// Marketing — the operator's decision on a standing-request candidate
    /// (GT-D17, second kind): accepted or dismissed. Writes decided_at /
    /// outcome onto the candidate row; acceptance itself lands as stage.changed.
    StandingRequestDecided,
    /// Marketing — the operator's note that a day's harvest of a crop covers
    /// named commitments (GT-D19). Informational: never capacity, never
    /// allocation, never money. Newest note per (crop, day) is the truth.
    HarvestCovered,
    /// Rack-side (GT-D20) — a phone-originated physical proposal. Candidate only:
    /// never changes trays; the Confirm gate applies clean ones through the
    /// existing tray write paths. Not farm truth (identity::PROPOSAL_KINDS).
    PhoneProposed,
    /// Rack-side (GT-D20) — the operator's decision on a proposal: accepted (with
    /// the applied event ids) or discarded (with the gate reason). Writes onto
    /// the same phone_proposals row.
    PhoneProposalDecided,
    /// LO-A (GT-D24) — the operator lists leftover ounces of one harvested
    /// crop-day for retail. Capped by that day's harvested ounces at the write
    /// door; capacity-free: moves no tray, books no money. One per key.
    LeftoverListed,
    /// LO-B (GT-D24-B) — a Stripe Payment Link minted for one leftover listing
    /// at an operator-typed price. Freezes id, url, reference and the priced
    /// total on the row; books no money.
    LeftoverLinkMinted,
    /// LO-B (GT-D24-B) — the poll matched a paid Checkout Session to a listing
    /// by its lo- reference and booked income.received in the same transaction.
    LeftoverPaid,
    /// SEED-A (GT-D25) — the operator records seed of one crop arriving at the
    /// farm, in ounces. Add-only, keyed by crop id; the IN side of the jar whose
    /// OUT side is the sow path's consumption.physical oz row. Moves no tray,
    /// books no money; inverse none.
    SeedReceived,
}

impl Kind {
    /// Every variant. Used to prove `tier` is total at runtime and to drive
    /// trigger SQL so the database cannot drift from the type system.
    pub const ALL: [Kind; 54] = [
        Kind::TraySown,
        Kind::TraysAdvanced,
        Kind::TraysHarvested,
        Kind::TrayDiscarded,
        Kind::TraysDiscarded,
        Kind::RecountApplied,
        Kind::Undo,
        Kind::DevBackdated,
        Kind::AttentionResolved,
        Kind::StripeSessionPaid,
        Kind::StripeRefunded,
        Kind::StripeDisputed,
        Kind::SnapshotTaken,
        Kind::CostMoneyOut,
        Kind::CostMoneyOutCorrected,
        Kind::CostMoneyOutVoided,
        Kind::ConsumptionPhysical,
        Kind::MileageTripLogged,
        Kind::MileageTripCorrected,
        Kind::MileageTripVoided,
        Kind::AssetRecorded,
        Kind::AssetCorrected,
        Kind::AssetVoided,
        Kind::IncomeReceived,
        Kind::IncomeCorrected,
        Kind::IncomeVoided,
        Kind::VenueRecorded,
        Kind::VenueCorrected,
        Kind::VenueArchived,
        Kind::SampleDropped,
        Kind::TouchLogged,
        Kind::FollowupSet,
        Kind::FollowupCleared,
        Kind::StageChanged,
        Kind::ReviewsObserved,
        Kind::ReviewRequested,
        Kind::WholesaleOrdered,
        Kind::WholesaleDelivered,
        Kind::WholesalePaid,
        Kind::WholesaleVoided,
        Kind::WholesaleWriteOff,
        Kind::WholesalePaymentReversed,
        Kind::WholesaleBadDebt,
        Kind::WholesaleLinkMinted,
        Kind::StripeFactUnapplied,
        Kind::StandingRequested,
        Kind::StandingRequestDecided,
        Kind::HarvestCovered,
        Kind::PhoneProposed,
        Kind::PhoneProposalDecided,
        Kind::LeftoverListed,
        Kind::LeftoverLinkMinted,
        Kind::LeftoverPaid,
        Kind::SeedReceived,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Kind::TraySown => "tray.sown",
            Kind::TraysAdvanced => "trays.advanced",
            Kind::TraysHarvested => "trays.harvested",
            Kind::TrayDiscarded => "tray.discarded",
            Kind::TraysDiscarded => "trays.discarded",
            Kind::RecountApplied => "recount.applied",
            Kind::Undo => "undo",
            Kind::DevBackdated => "dev.backdated",
            Kind::AttentionResolved => "attention.resolved",
            Kind::StripeSessionPaid => "stripe.session_paid",
            Kind::StripeRefunded => "stripe.refunded",
            Kind::StripeDisputed => "stripe.disputed",
            Kind::SnapshotTaken => "snapshot.taken",
            Kind::CostMoneyOut => "cost.money_out",
            Kind::CostMoneyOutCorrected => "cost.money_out_corrected",
            Kind::CostMoneyOutVoided => "cost.money_out_voided",
            Kind::ConsumptionPhysical => "consumption.physical",
            Kind::MileageTripLogged => "mileage.trip",
            Kind::MileageTripCorrected => "mileage.trip_corrected",
            Kind::MileageTripVoided => "mileage.trip_voided",
            Kind::AssetRecorded => "asset.recorded",
            Kind::AssetCorrected => "asset.corrected",
            Kind::AssetVoided => "asset.voided",
            Kind::IncomeReceived => "income.received",
            Kind::IncomeCorrected => "income.corrected",
            Kind::IncomeVoided => "income.voided",
            Kind::VenueRecorded => "venue.recorded",
            Kind::VenueCorrected => "venue.corrected",
            Kind::VenueArchived => "venue.archived",
            Kind::SampleDropped => "sample.dropped",
            Kind::TouchLogged => "touch.logged",
            Kind::FollowupSet => "followup.set",
            Kind::FollowupCleared => "followup.cleared",
            Kind::StageChanged => "stage.changed",
            Kind::ReviewsObserved => "reviews.observed",
            Kind::ReviewRequested => "review.requested",
            Kind::WholesaleOrdered => "wholesale.ordered",
            Kind::WholesaleDelivered => "wholesale.delivered",
            Kind::WholesalePaid => "wholesale.paid",
            Kind::WholesaleVoided => "wholesale.voided",
            Kind::WholesaleWriteOff => "wholesale.write_off",
            Kind::WholesalePaymentReversed => "wholesale.payment_reversed",
            Kind::WholesaleBadDebt => "wholesale.bad_debt",
            Kind::WholesaleLinkMinted => "wholesale.link_minted",
            Kind::StripeFactUnapplied => "stripe.fact_unapplied",
            Kind::StandingRequested => "standing.requested",
            Kind::StandingRequestDecided => "standing.request_decided",
            Kind::HarvestCovered => "harvest.covered",
            Kind::PhoneProposed => "phone.proposed",
            Kind::PhoneProposalDecided => "phone.proposal_decided",
            Kind::LeftoverListed => "leftover.listed",
            Kind::LeftoverLinkMinted => "leftover.link_minted",
            Kind::LeftoverPaid => "leftover.paid",
            Kind::SeedReceived => "seed.received",
        }
    }

    pub fn parse(s: &str) -> Result<Kind, String> {
        match s {
            "tray.sown" => Ok(Kind::TraySown),
            "trays.advanced" => Ok(Kind::TraysAdvanced),
            "trays.harvested" => Ok(Kind::TraysHarvested),
            "tray.discarded" => Ok(Kind::TrayDiscarded),
            "trays.discarded" => Ok(Kind::TraysDiscarded),
            "recount.applied" => Ok(Kind::RecountApplied),
            "undo" => Ok(Kind::Undo),
            "dev.backdated" => Ok(Kind::DevBackdated),
            "attention.resolved" => Ok(Kind::AttentionResolved),
            "stripe.session_paid" => Ok(Kind::StripeSessionPaid),
            "stripe.refunded" => Ok(Kind::StripeRefunded),
            "stripe.disputed" => Ok(Kind::StripeDisputed),
            "snapshot.taken" => Ok(Kind::SnapshotTaken),
            "cost.money_out" => Ok(Kind::CostMoneyOut),
            "cost.money_out_corrected" => Ok(Kind::CostMoneyOutCorrected),
            "cost.money_out_voided" => Ok(Kind::CostMoneyOutVoided),
            "consumption.physical" => Ok(Kind::ConsumptionPhysical),
            "mileage.trip" => Ok(Kind::MileageTripLogged),
            "mileage.trip_corrected" => Ok(Kind::MileageTripCorrected),
            "mileage.trip_voided" => Ok(Kind::MileageTripVoided),
            "asset.recorded" => Ok(Kind::AssetRecorded),
            "asset.corrected" => Ok(Kind::AssetCorrected),
            "asset.voided" => Ok(Kind::AssetVoided),
            "income.received" => Ok(Kind::IncomeReceived),
            "income.corrected" => Ok(Kind::IncomeCorrected),
            "income.voided" => Ok(Kind::IncomeVoided),
            "venue.recorded" => Ok(Kind::VenueRecorded),
            "venue.corrected" => Ok(Kind::VenueCorrected),
            "venue.archived" => Ok(Kind::VenueArchived),
            "sample.dropped" => Ok(Kind::SampleDropped),
            "touch.logged" => Ok(Kind::TouchLogged),
            "followup.set" => Ok(Kind::FollowupSet),
            "followup.cleared" => Ok(Kind::FollowupCleared),
            "stage.changed" => Ok(Kind::StageChanged),
            "reviews.observed" => Ok(Kind::ReviewsObserved),
            "review.requested" => Ok(Kind::ReviewRequested),
            "wholesale.ordered" => Ok(Kind::WholesaleOrdered),
            "wholesale.delivered" => Ok(Kind::WholesaleDelivered),
            "wholesale.paid" => Ok(Kind::WholesalePaid),
            "wholesale.voided" => Ok(Kind::WholesaleVoided),
            "wholesale.write_off" => Ok(Kind::WholesaleWriteOff),
            "wholesale.payment_reversed" => Ok(Kind::WholesalePaymentReversed),
            "wholesale.bad_debt" => Ok(Kind::WholesaleBadDebt),
            "wholesale.link_minted" => Ok(Kind::WholesaleLinkMinted),
            "stripe.fact_unapplied" => Ok(Kind::StripeFactUnapplied),
            "standing.requested" => Ok(Kind::StandingRequested),
            "standing.request_decided" => Ok(Kind::StandingRequestDecided),
            "harvest.covered" => Ok(Kind::HarvestCovered),
            "phone.proposed" => Ok(Kind::PhoneProposed),
            "phone.proposal_decided" => Ok(Kind::PhoneProposalDecided),
            "leftover.listed" => Ok(Kind::LeftoverListed),
            "leftover.link_minted" => Ok(Kind::LeftoverLinkMinted),
            "leftover.paid" => Ok(Kind::LeftoverPaid),
            "seed.received" => Ok(Kind::SeedReceived),
            other => Err(format!("unknown event kind: {other}")),
        }
    }

    /// Total map: every Kind has exactly one (domain, class) pair.
    /// Adding a Kind variant without an arm here fails to compile.
    pub const fn tier(self) -> (EventDomain, Option<EventClass>) {
        match self {
            Kind::TraySown
            | Kind::TraysAdvanced
            | Kind::TraysHarvested
            | Kind::TrayDiscarded
            | Kind::TraysDiscarded
            | Kind::RecountApplied
            | Kind::Undo
            | Kind::DevBackdated
            | Kind::AttentionResolved
            | Kind::PhoneProposed
            | Kind::PhoneProposalDecided => (EventDomain::Grow, None),
            Kind::StripeSessionPaid
            | Kind::StripeRefunded
            | Kind::StripeDisputed
            | Kind::StripeFactUnapplied
            | Kind::WholesaleOrdered
            | Kind::WholesaleDelivered
            | Kind::WholesalePaid
            | Kind::WholesaleVoided
            | Kind::WholesaleWriteOff
            | Kind::WholesalePaymentReversed
            | Kind::WholesaleBadDebt
            | Kind::WholesaleLinkMinted
            | Kind::LeftoverListed
            | Kind::LeftoverLinkMinted
            | Kind::LeftoverPaid => (EventDomain::Register, Some(EventClass::SaleFarmOsPath)),
            Kind::SnapshotTaken => (EventDomain::Register, Some(EventClass::Snapshot)),
            Kind::CostMoneyOut | Kind::CostMoneyOutCorrected | Kind::CostMoneyOutVoided => {
                (EventDomain::Register, Some(EventClass::MoneyOut))
            }
            Kind::ConsumptionPhysical | Kind::SeedReceived => {
                (EventDomain::Register, Some(EventClass::PhysicalConsumption))
            }
            Kind::MileageTripLogged | Kind::MileageTripCorrected | Kind::MileageTripVoided => {
                (EventDomain::Register, Some(EventClass::Mileage))
            }
            Kind::AssetRecorded | Kind::AssetCorrected | Kind::AssetVoided => {
                (EventDomain::Register, Some(EventClass::AssetRegister))
            }
            Kind::IncomeReceived | Kind::IncomeCorrected | Kind::IncomeVoided => {
                (EventDomain::Register, Some(EventClass::MoneyIn))
            }
            Kind::VenueRecorded
            | Kind::VenueCorrected
            | Kind::VenueArchived
            | Kind::SampleDropped
            | Kind::TouchLogged
            | Kind::FollowupSet
            | Kind::FollowupCleared
            | Kind::StageChanged
            | Kind::ReviewsObserved
            | Kind::ReviewRequested
            | Kind::StandingRequested
            | Kind::StandingRequestDecided
            | Kind::HarvestCovered => (EventDomain::Marketing, None),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventDomain {
    Grow,
    Register,
    Marketing,
}

impl EventDomain {
    #[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
    pub const ALL: [EventDomain; 3] = [
        EventDomain::Grow,
        EventDomain::Register,
        EventDomain::Marketing,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            EventDomain::Grow => "grow",
            EventDomain::Register => "register",
            EventDomain::Marketing => "marketing",
        }
    }
}

/// The eight register-tier event_class values. Grow rows carry NULL.
/// Commercial-app classes are not variants — unrepresentable, not rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventClass {
    MoneyOut,
    PhysicalConsumption,
    Mileage,
    AssetRegister,
    SaleFarmOsPath,
    CapacityCommitment,
    Snapshot,
    /// Money arriving. The mirror of MoneyOut (money-in track).
    MoneyIn,
}

impl EventClass {
    #[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
    pub const ALL: [EventClass; 8] = [
        EventClass::MoneyOut,
        EventClass::PhysicalConsumption,
        EventClass::Mileage,
        EventClass::AssetRegister,
        EventClass::SaleFarmOsPath,
        EventClass::CapacityCommitment,
        EventClass::Snapshot,
        EventClass::MoneyIn,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            EventClass::MoneyOut => "money_out",
            EventClass::PhysicalConsumption => "physical_consumption",
            EventClass::Mileage => "mileage",
            EventClass::AssetRegister => "asset_register",
            EventClass::SaleFarmOsPath => "sale_farm_os_path",
            EventClass::CapacityCommitment => "capacity_commitment",
            EventClass::Snapshot => "snapshot",
            EventClass::MoneyIn => "money_in",
        }
    }

    pub fn parse(s: &str) -> Result<EventClass, String> {
        match s {
            "money_out" => Ok(EventClass::MoneyOut),
            "physical_consumption" => Ok(EventClass::PhysicalConsumption),
            "mileage" => Ok(EventClass::Mileage),
            "asset_register" => Ok(EventClass::AssetRegister),
            "sale_farm_os_path" => Ok(EventClass::SaleFarmOsPath),
            "capacity_commitment" => Ok(EventClass::CapacityCommitment),
            "snapshot" => Ok(EventClass::Snapshot),
            "money_in" => Ok(EventClass::MoneyIn),
            other => Err(format!("unknown event class: {other}")),
        }
    }
}

/// The eight register-tier event_class string values (for SQL / flush guard).
pub const EVENT_CLASSES: &[&str] = &[
    EventClass::MoneyOut.as_str(),
    EventClass::PhysicalConsumption.as_str(),
    EventClass::Mileage.as_str(),
    EventClass::AssetRegister.as_str(),
    EventClass::SaleFarmOsPath.as_str(),
    EventClass::CapacityCommitment.as_str(),
    EventClass::Snapshot.as_str(),
    EventClass::MoneyIn.as_str(),
];

/// GROW kind strings — derived from `Kind::tier` so they cannot drift.
pub fn grow_kinds() -> Vec<&'static str> {
    Kind::ALL
        .iter()
        .filter(|k| matches!(k.tier(), (EventDomain::Grow, None)))
        .map(|k| k.as_str())
        .collect()
}

/// REGISTER kind strings — derived from `Kind::tier`.
pub fn register_kinds() -> Vec<&'static str> {
    Kind::ALL
        .iter()
        .filter(|k| matches!(k.tier().0, EventDomain::Register))
        .map(|k| k.as_str())
        .collect()
}

/// MARKETING kind strings — derived from `Kind::tier`.
pub fn marketing_kinds() -> Vec<&'static str> {
    Kind::ALL
        .iter()
        .filter(|k| matches!(k.tier(), (EventDomain::Marketing, None)))
        .map(|k| k.as_str())
        .collect()
}

/// Compatibility aliases used by the flush guard (same contents as the fns).
pub const GROW_KINDS: &[&str] = &[
    "tray.sown",
    "trays.advanced",
    "trays.harvested",
    "tray.discarded",
    "trays.discarded",
    "recount.applied",
    "undo",
    "dev.backdated",
    "attention.resolved",
    "phone.proposed",
    "phone.proposal_decided",
];

pub const REGISTER_KINDS: &[&str] = &[
    "stripe.session_paid",
    "stripe.refunded",
    "stripe.disputed",
    "snapshot.taken",
    "cost.money_out",
    "cost.money_out_corrected",
    "cost.money_out_voided",
    "consumption.physical",
    "mileage.trip",
    "mileage.trip_corrected",
    "mileage.trip_voided",
    "asset.recorded",
    "asset.corrected",
    "asset.voided",
    "income.received",
    "income.corrected",
    "income.voided",
    "wholesale.ordered",
    "wholesale.delivered",
    "wholesale.paid",
    "wholesale.voided",
    "wholesale.write_off",
    "wholesale.payment_reversed",
    "wholesale.bad_debt",
    "wholesale.link_minted",
    "stripe.fact_unapplied",
    "leftover.listed",
    "leftover.link_minted",
    "leftover.paid",
    "seed.received",
];

/// Compatibility alias used by the flush guard (same contents as marketing_kinds).
pub const MARKETING_KINDS: &[&str] = &[
    "venue.recorded",
    "venue.corrected",
    "venue.archived",
    "sample.dropped",
    "touch.logged",
    "followup.set",
    "followup.cleared",
    "stage.changed",
    "reviews.observed",
    "review.requested",
    "standing.requested",
    "standing.request_decided",
    "harvest.covered",
];

/// Register kinds as of schema v10 (before consumption.physical). Frozen for
/// v10 fixture DBs so migration T8 can prove the v11 trigger reinstall.
#[cfg(test)]
pub const REGISTER_KINDS_V10: &[&str] = &[
    "stripe.session_paid",
    "stripe.refunded",
    "stripe.disputed",
    "snapshot.taken",
    "cost.money_out",
];

/// Register kinds as of schema v12 (before mileage / asset). Frozen for
/// v12 fixture DBs so migration T-v13 can prove the trigger reinstall.
#[cfg(test)]
pub const REGISTER_KINDS_V12: &[&str] = &[
    "stripe.session_paid",
    "stripe.refunded",
    "stripe.disputed",
    "snapshot.taken",
    "cost.money_out",
    "consumption.physical",
];

/// Register kinds as of schema v13 (before income / money_in). Frozen for
/// v13 fixture DBs so migration IN12 can prove the v14 trigger reinstall.
#[cfg(test)]
pub const REGISTER_KINDS_V13: &[&str] = &[
    "stripe.session_paid",
    "stripe.refunded",
    "stripe.disputed",
    "snapshot.taken",
    "cost.money_out",
    "consumption.physical",
    "mileage.trip",
    "mileage.trip_corrected",
    "mileage.trip_voided",
    "asset.recorded",
    "asset.corrected",
    "asset.voided",
];

/// Event classes as of schema v13 (before money_in). Frozen so a v13 fixture
/// reproduces the old trigger exactly and does not learn about money_in.
#[cfg(test)]
pub const EVENT_CLASSES_V13: &[&str] = &[
    "money_out",
    "physical_consumption",
    "mileage",
    "asset_register",
    "sale_farm_os_path",
    "capacity_commitment",
    "snapshot",
];

/// v12-era triggers: the five Track 4 residual kinds not yet whitelisted.
#[cfg(test)]
pub fn schema_v12_event_log_triggers_sql() -> String {
    let grow = grow_kinds();
    schema_event_log_triggers_sql(&grow, REGISTER_KINDS_V12, EVENT_CLASSES_V13, &[])
}

/// v13-era triggers: income kinds and money_in class not yet whitelisted.
#[cfg(test)]
pub fn schema_v13_event_log_triggers_sql() -> String {
    let grow = grow_kinds();
    schema_event_log_triggers_sql(&grow, REGISTER_KINDS_V13, EVENT_CLASSES_V13, &[])
}

pub fn is_partition_kind(kind: &str) -> bool {
    Kind::parse(kind).is_ok()
}

pub fn sql_string_list(items: &[&str]) -> String {
    items
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Phase 2 corrected triggers — generated from `Kind` / `EventClass` so the
/// flush guard and INSERT/UPDATE enforce the same closed sets.
pub fn schema_v9_event_log_triggers_sql() -> String {
    let grow = grow_kinds();
    let register = register_kinds();
    let marketing = marketing_kinds();
    schema_event_log_triggers_sql(&grow, &register, EVENT_CLASSES, &marketing)
}

/// Build event_log INSERT/UPDATE/DELETE triggers for the given kind and class
/// whitelists.
pub fn schema_event_log_triggers_sql(
    grow: &[&str],
    register: &[&str],
    classes: &[&str],
    marketing: &[&str],
) -> String {
    let grow_kinds = sql_string_list(grow);
    let register_kinds = sql_string_list(register);
    let event_classes = sql_string_list(classes);
    let marketing_kinds = if marketing.is_empty() {
        "''".to_string() // empty whitelist: no kind matches
    } else {
        sql_string_list(marketing)
    };
    format!(
        r#"
CREATE TRIGGER IF NOT EXISTS event_log_before_insert
BEFORE INSERT ON event_log
BEGIN
  SELECT CASE
    WHEN NEW.id IS NULL OR NEW.id = ''
      THEN RAISE(ABORT, 'event_log.id required')
    WHEN NEW.origin IS NULL OR NEW.origin NOT IN ('farm_os', 'commercial_app')
      THEN RAISE(ABORT, 'event_log.origin invalid')
    WHEN NEW.event_domain IS NULL OR NEW.event_domain NOT IN ('grow', 'register', 'marketing')
      THEN RAISE(ABORT, 'event_log.event_domain invalid')
    WHEN NEW.event_domain = 'grow' AND NEW.event_class IS NOT NULL
      THEN RAISE(ABORT, 'event_log.event_class must be NULL for grow')
    WHEN NEW.event_domain = 'grow' AND (
      NEW.kind IS NULL OR NEW.kind NOT IN (
        {grow_kinds}
      )
    )
      THEN RAISE(ABORT, 'event_log.kind invalid for grow')
    WHEN NEW.event_domain = 'marketing' AND NEW.event_class IS NOT NULL
      THEN RAISE(ABORT, 'event_log.event_class must be NULL for marketing')
    WHEN NEW.event_domain = 'marketing' AND (
      NEW.kind IS NULL OR NEW.kind NOT IN (
        {marketing_kinds}
      )
    )
      THEN RAISE(ABORT, 'event_log.kind invalid for marketing')
    WHEN NEW.event_domain = 'register' AND (
      NEW.event_class IS NULL OR NEW.event_class NOT IN (
        {event_classes}
      )
    )
      THEN RAISE(ABORT, 'event_log.event_class invalid for register')
    WHEN NEW.event_domain = 'register' AND (
      NEW.kind IS NULL OR NEW.kind NOT IN (
        {register_kinds}
      )
    )
      THEN RAISE(ABORT, 'event_log.kind invalid for register')
  END;
END;

CREATE TRIGGER IF NOT EXISTS event_log_before_update
BEFORE UPDATE ON event_log
BEGIN
  SELECT CASE
    WHEN OLD.id IS NOT NEW.id
      OR OLD.seq IS NOT NEW.seq
      OR OLD.kind IS NOT NEW.kind
      OR (OLD.origin IS NOT NULL AND OLD.origin IS NOT NEW.origin)
      OR (OLD.event_domain IS NOT NULL AND OLD.event_domain IS NOT NEW.event_domain)
      OR (OLD.event_class IS NOT NULL AND OLD.event_class IS NOT NEW.event_class)
      -- grow/marketing rows must keep event_class NULL; refusing a fill that would violate that
      OR (NEW.event_domain = 'grow' AND NEW.event_class IS NOT NULL)
      OR (NEW.event_domain = 'marketing' AND NEW.event_class IS NOT NULL)
      THEN RAISE(ABORT, 'event_log immutable columns')
    -- Filling NULL origin/event_domain is permitted; resulting row must stay well-formed.
    WHEN NEW.origin IS NULL OR NEW.origin NOT IN ('farm_os', 'commercial_app')
      THEN RAISE(ABORT, 'event_log.origin invalid')
    WHEN NEW.event_domain IS NULL OR NEW.event_domain NOT IN ('grow', 'register', 'marketing')
      THEN RAISE(ABORT, 'event_log.event_domain invalid')
    WHEN NEW.event_domain = 'grow' AND (
      NEW.kind IS NULL OR NEW.kind NOT IN (
        {grow_kinds}
      )
    )
      THEN RAISE(ABORT, 'event_log.kind invalid for grow')
    WHEN NEW.event_domain = 'marketing' AND (
      NEW.kind IS NULL OR NEW.kind NOT IN (
        {marketing_kinds}
      )
    )
      THEN RAISE(ABORT, 'event_log.kind invalid for marketing')
    WHEN NEW.event_domain = 'register' AND (
      NEW.event_class IS NULL OR NEW.event_class NOT IN (
        {event_classes}
      )
    )
      THEN RAISE(ABORT, 'event_log.event_class invalid for register')
    WHEN NEW.event_domain = 'register' AND (
      NEW.kind IS NULL OR NEW.kind NOT IN (
        {register_kinds}
      )
    )
      THEN RAISE(ABORT, 'event_log.kind invalid for register')
  END;
END;

CREATE TRIGGER IF NOT EXISTS event_log_before_delete
BEFORE DELETE ON event_log
BEGIN
  SELECT RAISE(ABORT, 'event_log is append-only');
END;
"#
    )
}

/// v10-era triggers: cost.money_out whitelisted, consumption.physical not yet.
#[cfg(test)]
pub fn schema_v10_event_log_triggers_sql() -> String {
    let grow = grow_kinds();
    schema_event_log_triggers_sql(&grow, REGISTER_KINDS_V10, EVENT_CLASSES_V13, &[])
}
