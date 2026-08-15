//! Monotonic ids for immutable catalog objects.
//!
//! A candidate is a reservation, not a mutation: validation failure, abort, or a retry before the
//! object becomes visible gets the same id. Only a successful media commit advances the sequence,
//! and that next value is what the board persists as its reboot floor. Mount recovery feeds valid
//! committed filenames through [`observe_committed`]; inert staged files are reclaimed by the
//! object policy and do not consume an id.
//!
//! These small functions deliberately operate on the board owner's existing `u16`. That keeps the
//! policy host-testable without changing the owner layout or obscuring release-code range proofs.

/// Raise `next` to a persisted exclusive floor.
#[inline(always)]
pub fn observe_floor(next: &mut u16, floor: u16) {
    *next = (*next).max(floor);
}

/// Recover past a valid committed id found during a catalog scan.
#[inline(always)]
pub fn observe_committed(next: &mut u16, id: u16) {
    observe_floor(next, id.saturating_add(1));
}

/// Reserve the current candidate without consuming it. `limit` is exclusive.
#[inline(always)]
pub const fn candidate(next: u16, limit: u16) -> Option<u16> {
    if next < limit {
        Some(next)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UPLOAD_LIMIT: u16 = 0x8000;

    #[test]
    fn reservation_does_not_burn_on_abort_validation_failure_or_retry() {
        let next = 0;
        let first = candidate(next, UPLOAD_LIMIT);
        assert_eq!(candidate(next, UPLOAD_LIMIT), first);
        assert_eq!(next, 0);
    }

    #[test]
    fn boot_recovery_combines_valid_catalog_ids_with_the_persisted_floor() {
        let mut next = 0;
        observe_committed(&mut next, 7);
        observe_committed(&mut next, 3);
        observe_floor(&mut next, 12);
        observe_floor(&mut next, 10);
        assert_eq!(candidate(next, UPLOAD_LIMIT), Some(12));
    }

    #[test]
    fn an_inert_swept_candidate_is_not_observed_or_burned() {
        let mut next = 0;
        observe_committed(&mut next, 4);
        assert_eq!(candidate(next, UPLOAD_LIMIT), Some(5));
    }

    #[test]
    fn limit_refuses_a_new_candidate_without_wraparound() {
        let next = UPLOAD_LIMIT;
        assert_eq!(candidate(next, UPLOAD_LIMIT), None);
    }
}
