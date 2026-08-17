//! The scenario suite, run against the in-memory transaction.
//!
//! Every body lives in [`scenarios`](super::scenarios), written once and generic over the store
//! under the engine. What is here is the fixture that says "the store is the fake" and one `#[test]`
//! per scenario, so a failure names itself. `obc-storage` runs the identical list against the
//! kernel-backed transaction, and the two must agree.

use std::vec::Vec;

use crate::engine::PrincipalScope;
use crate::ids::{LogicalObjectId, OperationId, Revision};
use crate::registry::ObjectKind;

use super::scenarios::{self, Fault, Fixture, Store};
use super::transaction::FakeTransaction;

/// The fixture whose store is the in-memory transaction.
struct Fake;

impl Fixture for Fake {
    type Store = FakeTransaction;

    fn store(&mut self) -> FakeTransaction {
        FakeTransaction::new(scenarios::STORE)
    }
}

impl Store for FakeTransaction {
    fn head(&self, kind: ObjectKind, logical_object_id: LogicalObjectId) -> Option<(Revision, u64, u32)> {
        FakeTransaction::head(self, kind, logical_object_id)
    }

    fn payload_is(&mut self, kind: ObjectKind, logical_object_id: LogicalObjectId, expected: &[u8]) -> bool {
        FakeTransaction::payload(self, kind, logical_object_id) == Some(expected)
    }

    fn has_lease(&self) -> bool {
        FakeTransaction::has_lease(self)
    }

    fn retains(&self, operation_id: OperationId) -> bool {
        FakeTransaction::retains(self, operation_id)
    }

    fn retained_results(&self) -> usize {
        FakeTransaction::retained_results(self)
    }

    fn publish_local(&mut self, kind: ObjectKind, bytes: &[u8]) -> (LogicalObjectId, Revision) {
        FakeTransaction::publish_local(self, kind, bytes)
    }

    fn retain_local_result(&mut self, operation_id: OperationId) {
        FakeTransaction::retain_local_result(self, operation_id)
    }

    fn claim_install_update(&mut self, operation_id: OperationId, principal: PrincipalScope) {
        FakeTransaction::claim_install_update(self, operation_id, principal)
    }

    fn arm(&mut self, fault: Fault) {
        match fault {
            Fault::FailValidation(detail) => self.faults.fail_validation = Some(detail),
            Fault::FailPublication => self.faults.fail_publication = true,
            Fault::FailSeal => self.faults.fail_seal = true,
            Fault::RacePublication => self.faults.race_publication = true,
            Fault::RefuseClaim(cause) => self.faults.refuse_claim = Some(cause),
            Fault::FailAbort => self.faults.fail_abort = true,
        }
    }
}

#[test]
fn the_suite_names_every_scenario_exactly_once() {
    let mut names: Vec<&str> = scenarios::suite::<Fake>().iter().map(|(name, _)| *name).collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total, "a scenario is registered twice");
    assert!(total >= 40, "the suite carries {total} scenarios");
}

#[test]
fn one_engine_serves_both_links_with_byte_identical_records() {
    scenarios::one_engine_serves_both_links_with_byte_identical_records(&mut Fake);
}

#[test]
fn the_two_bindings_frame_the_same_records_differently_and_carry_them_the_same() {
    scenarios::the_two_bindings_frame_the_same_records_differently_and_carry_them_the_same(&mut Fake);
}

#[test]
fn nothing_is_admitted_before_hello_and_a_second_request_is_busy() {
    scenarios::nothing_is_admitted_before_hello_and_a_second_request_is_busy(&mut Fake);
}

#[test]
fn a_command_outstanding_on_one_link_is_not_disturbed_by_traffic_on_the_other() {
    scenarios::a_command_outstanding_on_one_link_is_not_disturbed_by_traffic_on_the_other(&mut Fake);
}

#[test]
fn an_outcome_for_a_dead_connection_is_never_answered_into_the_one_that_replaced_it() {
    scenarios::an_outcome_for_a_dead_connection_is_never_answered_into_the_one_that_replaced_it(&mut Fake);
}

#[test]
fn a_repeated_hello_pages_and_a_changed_one_is_refused() {
    scenarios::a_repeated_hello_pages_and_a_changed_one_is_refused(&mut Fake);
}

#[test]
fn a_stale_or_wrong_wire_session_cannot_advance_or_release_anything() {
    scenarios::a_stale_or_wrong_wire_session_cannot_advance_or_release_anything(&mut Fake);
}

#[test]
fn a_released_session_is_tombstoned_and_its_late_frames_are_discarded() {
    scenarios::a_released_session_is_tombstoned_and_its_late_frames_are_discarded(&mut Fake);
}

#[test]
fn a_frame_at_the_wrong_offset_faults_and_durably_aborts_restart_only_work() {
    scenarios::a_frame_at_the_wrong_offset_faults_and_durably_aborts_restart_only_work(&mut Fake);
}

#[test]
fn a_checkpoint_off_the_granule_or_the_next_offset_is_refused_without_touching_the_work() {
    scenarios::a_checkpoint_off_the_granule_or_the_next_offset_is_refused_without_touching_the_work(&mut Fake);
}

#[test]
fn append_validation_and_publication_failures_each_leave_one_terminal_aborted_result() {
    scenarios::append_validation_and_publication_failures_each_leave_one_terminal_aborted_result(&mut Fake);
}

#[test]
fn a_replace_that_loses_the_race_is_refused_at_the_commit_lock_and_leaves_the_head_alone() {
    scenarios::a_replace_that_loses_the_race_is_refused_at_the_commit_lock_and_leaves_the_head_alone(&mut Fake);
}

#[test]
fn a_lost_result_is_recovered_with_query_operation_and_replayed_by_the_same_intent() {
    scenarios::a_lost_result_is_recovered_with_query_operation_and_replayed_by_the_same_intent(&mut Fake);
}

#[test]
fn the_retained_window_is_sixty_four_and_eviction_makes_a_query_unknown() {
    scenarios::the_retained_window_is_sixty_four_and_eviction_makes_a_query_unknown(&mut Fake);
}

#[test]
fn link_loss_before_the_seal_durably_aborts_the_work_and_after_it_changes_nothing() {
    scenarios::link_loss_before_the_seal_durably_aborts_the_work_and_after_it_changes_nothing(&mut Fake);
}

#[test]
fn link_loss_during_a_download_releases_the_lease_exactly_once() {
    scenarios::link_loss_during_a_download_releases_the_lease_exactly_once(&mut Fake);
}

#[test]
fn device_control_runs_mid_transfer_without_touching_the_session() {
    scenarios::device_control_runs_mid_transfer_without_touching_the_session(&mut Fake);
}

#[test]
fn a_direct_mutation_publishes_through_the_same_command_machine() {
    scenarios::a_direct_mutation_publishes_through_the_same_command_machine(&mut Fake);
}

#[test]
fn an_abort_the_medium_refuses_is_answered_once_rather_than_retried() {
    scenarios::an_abort_the_medium_refuses_is_answered_once_rather_than_retried(&mut Fake);
}

#[test]
fn fuzzed_control_and_data_frames_never_panic_and_never_advance_a_session() {
    scenarios::fuzzed_control_and_data_frames_never_panic_and_never_advance_a_session(&mut Fake);
}

#[test]
fn every_checked_in_transcript_record_survives_both_bindings_byte_for_byte() {
    scenarios::every_checked_in_transcript_record_survives_both_bindings_byte_for_byte(&mut Fake);
}

#[test]
fn every_transcript_record_decodes_through_the_codec_the_engine_dispatches_on() {
    scenarios::every_transcript_record_decodes_through_the_codec_the_engine_dispatches_on(&mut Fake);
}

#[test]
fn the_end_to_end_transcript_drives_the_engine_identically_on_both_links() {
    scenarios::the_end_to_end_transcript_drives_the_engine_identically_on_both_links(&mut Fake);
}

#[test]
fn every_transcript_the_harness_does_not_drive_names_the_reason() {
    scenarios::every_transcript_the_harness_does_not_drive_names_the_reason(&mut Fake);
}

#[test]
fn a_preflight_refusal_creates_no_state_and_carries_neither_claim_bit() {
    scenarios::a_preflight_refusal_creates_no_state_and_carries_neither_claim_bit(&mut Fake);
}

#[test]
fn a_second_heavy_transfer_is_refused_with_the_owners_link_kind() {
    scenarios::a_second_heavy_transfer_is_refused_with_the_owners_link_kind(&mut Fake);
}

#[test]
fn usb_completes_its_in_records_in_order_and_resets_a_malformed_record_stream() {
    scenarios::usb_completes_its_in_records_in_order_and_resets_a_malformed_record_stream(&mut Fake);
}

#[test]
fn the_idempotency_lookup_precedes_busy_and_size_refusals() {
    scenarios::the_idempotency_lookup_precedes_busy_and_size_refusals(&mut Fake);
}

#[test]
fn a_same_intent_start_upload_for_the_live_transfer_restarts_it_rather_than_refusing_it() {
    scenarios::a_same_intent_start_upload_for_the_live_transfer_restarts_it_rather_than_refusing_it(&mut Fake);
}

#[test]
fn abort_operation_marks_its_target_terminal_and_returns_its_own_abort_result() {
    scenarios::abort_operation_marks_its_target_terminal_and_returns_its_own_abort_result(&mut Fake);
}

#[test]
fn an_abort_command_naming_an_absent_or_foreign_target_says_so_without_burning_state() {
    scenarios::an_abort_command_naming_an_absent_or_foreign_target_says_so_without_burning_state(&mut Fake);
}

#[test]
fn a_foreign_principal_is_refused_rather_than_told_an_operations_status() {
    scenarios::a_foreign_principal_is_refused_rather_than_told_an_operations_status(&mut Fake);
}

#[test]
fn an_interleaved_mutation_publishes_its_own_claim_and_not_the_uploads() {
    scenarios::an_interleaved_mutation_publishes_its_own_claim_and_not_the_uploads(&mut Fake);
}

#[test]
fn reset_store_abandons_active_work_before_it_destroys_the_store_and_ends_the_connection() {
    scenarios::reset_store_abandons_active_work_before_it_destroys_the_store_and_ends_the_connection(&mut Fake);
}

#[test]
fn set_metadata_install_update_and_ride_acknowledgement_each_publish_their_own_outcome() {
    scenarios::set_metadata_install_update_and_ride_acknowledgement_each_publish_their_own_outcome(&mut Fake);
}

#[test]
fn an_abort_operation_naming_an_install_update_is_refused_as_non_cancellable() {
    scenarios::an_abort_operation_naming_an_install_update_is_refused_as_non_cancellable(&mut Fake);
}

#[test]
fn every_reachable_upload_phase_is_reported_as_the_engine_holds_it() {
    scenarios::every_reachable_upload_phase_is_reported_as_the_engine_holds_it(&mut Fake);
}

#[test]
fn two_links_that_negotiate_different_limits_still_carry_identical_records() {
    scenarios::two_links_that_negotiate_different_limits_still_carry_identical_records(&mut Fake);
}

#[test]
fn the_abort_transcript_semantics_are_reproduced_by_the_engine() {
    scenarios::the_abort_transcript_semantics_are_reproduced_by_the_engine(&mut Fake);
}

#[test]
fn the_ble_link_completes_confirmed_indications_in_order_and_recovers_from_a_lost_one() {
    scenarios::the_ble_link_completes_confirmed_indications_in_order_and_recovers_from_a_lost_one(&mut Fake);
}

#[test]
fn cancelling_the_live_upload_releases_its_session_and_retains_exactly_one_result() {
    scenarios::cancelling_the_live_upload_releases_its_session_and_retains_exactly_one_result(&mut Fake);
}

#[test]
fn a_mutation_dropped_mid_chain_still_reaches_a_terminal_state() {
    scenarios::a_mutation_dropped_mid_chain_still_reaches_a_terminal_state(&mut Fake);
}

#[test]
fn two_connections_that_drop_with_claims_in_flight_both_reach_a_terminal_state() {
    scenarios::two_connections_that_drop_with_claims_in_flight_both_reach_a_terminal_state(&mut Fake);
}
