use crate::assets;
use crate::consumption;
use crate::costs;
use crate::events::{EventRecord, Kind};
use crate::income;
use crate::marketing;
use crate::mileage;
use crate::money;
use crate::phone;
use crate::trays;
use rusqlite::Transaction;

/// Single projection entry point — handlers and verify-replay both call this.
pub fn apply_event(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    match event.kind {
        Kind::TraySown => trays::apply_tray_sown(tx, event),
        Kind::TraysAdvanced => trays::apply_trays_advanced(tx, event),
        Kind::TraysHarvested => trays::apply_trays_harvested(tx, event),
        Kind::TrayDiscarded => trays::apply_tray_discarded(tx, event),
        Kind::TraysDiscarded => trays::apply_trays_discarded(tx, event),
        Kind::RecountApplied => trays::apply_recount_applied(tx, event),
        Kind::Undo => trays::apply_undo(tx, event),
        Kind::DevBackdated => trays::apply_dev_backdated(tx, event),
        // Explicit no-op: attention is outside the replay ledger (Ruling 2
        // extension). Live resolve updates the row in the handler; replay does
        // not reconstruct attention rows and must not fail on resolve.
        Kind::AttentionResolved => Ok(()),
        Kind::StripeSessionPaid => money::apply_stripe_session_paid(tx, event),
        Kind::StripeRefunded => money::apply_stripe_refunded(tx, event),
        Kind::StripeDisputed => money::apply_stripe_disputed(tx, event),
        Kind::StripeFactUnapplied => money::apply_stripe_fact_unapplied(tx, event),
        Kind::CostMoneyOut => costs::apply_cost_money_out(tx, event),
        Kind::CostMoneyOutCorrected => costs::apply_cost_money_out_corrected(tx, event),
        Kind::CostMoneyOutVoided => costs::apply_cost_money_out_voided(tx, event),
        Kind::ConsumptionPhysical => consumption::apply_consumption_physical(tx, event),
        Kind::MileageTripLogged => mileage::apply_mileage_trip(tx, event),
        Kind::MileageTripCorrected => mileage::apply_mileage_trip_corrected(tx, event),
        Kind::MileageTripVoided => mileage::apply_mileage_trip_voided(tx, event),
        Kind::AssetRecorded => assets::apply_asset_recorded(tx, event),
        Kind::AssetCorrected => assets::apply_asset_corrected(tx, event),
        Kind::AssetVoided => assets::apply_asset_voided(tx, event),
        Kind::IncomeReceived => income::apply_income_received(tx, event),
        Kind::IncomeCorrected => income::apply_income_corrected(tx, event),
        Kind::IncomeVoided => income::apply_income_voided(tx, event),
        Kind::VenueRecorded => marketing::apply_venue_recorded(tx, event),
        Kind::VenueCorrected => marketing::apply_venue_corrected(tx, event),
        Kind::VenueArchived => marketing::apply_venue_archived(tx, event),
        Kind::SampleDropped => marketing::apply_sample_dropped(tx, event),
        Kind::TouchLogged => marketing::apply_touch_logged(tx, event),
        Kind::FollowupSet => marketing::apply_followup_set(tx, event),
        Kind::FollowupCleared => marketing::apply_followup_cleared(tx, event),
        Kind::StageChanged => marketing::apply_stage_changed(tx, event),
        Kind::ReviewsObserved => marketing::apply_reviews_observed(tx, event),
        Kind::ReviewRequested => marketing::apply_review_requested(tx, event),
        Kind::StandingRequested => marketing::apply_standing_requested(tx, event),
        Kind::StandingRequestDecided => marketing::apply_standing_request_decided(tx, event),
        Kind::HarvestCovered => marketing::apply_harvest_covered(tx, event),
        Kind::PhoneProposed => phone::apply_phone_proposed(tx, event),
        Kind::PhoneProposalDecided => phone::apply_phone_proposal_decided(tx, event),
        Kind::WholesaleOrdered => crate::wholesale::apply_wholesale_ordered(tx, event),
        Kind::WholesaleDelivered => crate::wholesale::apply_wholesale_delivered(tx, event),
        Kind::WholesalePaid => crate::wholesale::apply_wholesale_paid(tx, event),
        Kind::WholesaleVoided => crate::wholesale::apply_wholesale_voided(tx, event),
        Kind::WholesaleWriteOff => crate::wholesale::apply_wholesale_write_off(tx, event),
        Kind::WholesalePaymentReversed => {
            crate::wholesale::apply_wholesale_payment_reversed(tx, event)
        }
        Kind::WholesaleBadDebt => crate::wholesale::apply_wholesale_bad_debt(tx, event),
        Kind::WholesaleLinkMinted => crate::wholesale::apply_wholesale_link_minted(tx, event),
        Kind::LeftoverListed => crate::leftover::apply_leftover_listed(tx, event),
        Kind::LeftoverLinkMinted => crate::leftover::apply_leftover_link_minted(tx, event),
        Kind::LeftoverPaid => crate::leftover::apply_leftover_paid(tx, event),
        Kind::SeedReceived => crate::seed::apply_seed_received(tx, event),
        // Explicit no-op: filesystem artifact only (Ruling 2 category 3).
        Kind::SnapshotTaken => Ok(()),
    }
}
