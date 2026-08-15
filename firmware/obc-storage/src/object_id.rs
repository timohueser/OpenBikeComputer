//! Monotonic ids for immutable catalog objects.
//!
//! A candidate is a reservation, not a mutation: validation failure, abort, or a retry before the
//! object becomes visible gets the same id. Only [`ObjectIdSequence::commit`] advances the sequence
//! and hands its new reboot floor to the caller for persistence.

/// The candidate and reboot-floor state machine for one immutable-object id band.
///
/// `LIMIT` is exclusive. Recovery observes only validated committed filenames; an inert staged
/// file is swept by the media owner without reaching this sequence, so it cannot consume an id.
pub struct ObjectIdSequence<const LIMIT: u16> {
    next: u16,
}

impl<const LIMIT: u16> ObjectIdSequence<LIMIT> {
    /// An empty sequence whose first candidate is zero.
    #[inline(always)]
    pub const fn new() -> Self {
        Self { next: 0 }
    }

    /// Raise the next candidate to a persisted exclusive floor.
    #[inline(always)]
    pub fn observe_floor(&mut self, floor: u16) {
        self.next = self.next.max(floor);
    }

    /// Recover past a valid committed id found during a catalog scan.
    #[inline(always)]
    pub fn observe_committed(&mut self, id: u16) {
        self.observe_floor(id.saturating_add(1));
    }

    /// Reserve the current candidate without consuming it.
    #[inline(always)]
    pub const fn candidate(&self) -> Option<u16> {
        if self.next < LIMIT {
            Some(self.next)
        } else {
            None
        }
    }

    /// Advance after the reserved candidate became visible, then persist the new exclusive floor.
    /// The caller must have obtained that candidate through [`candidate`](Self::candidate); this is
    /// the only advancing operation, and is called only after the media commit succeeds.
    #[inline(always)]
    pub fn commit(&mut self, persist_floor: impl FnOnce(u16)) -> u16 {
        debug_assert!(self.next < LIMIT);
        let id = self.next;
        self.next = id + 1;
        persist_floor(self.next);
        id
    }
}

impl<const LIMIT: u16> Default for ObjectIdSequence<LIMIT> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UPLOAD_LIMIT: u16 = 0x8000;

    #[test]
    fn abort_validation_failure_and_retry_do_not_advance_or_persist() {
        let mut ids = ObjectIdSequence::<UPLOAD_LIMIT>::new();
        let candidate = ids.candidate();
        let mut persisted = None;

        // Abort and validation failure deliberately do not call `commit`.
        assert_eq!(ids.candidate(), candidate);
        assert_eq!(persisted, None);

        assert_eq!(Some(ids.commit(|floor| persisted = Some(floor))), candidate);
        assert_eq!(persisted, Some(1));
        assert_eq!(ids.candidate(), Some(1));
    }

    #[test]
    fn boot_recovery_combines_valid_catalog_ids_with_the_persisted_floor() {
        let mut ids = ObjectIdSequence::<UPLOAD_LIMIT>::new();
        ids.observe_committed(7);
        ids.observe_committed(3);
        ids.observe_floor(12);
        ids.observe_floor(10);
        assert_eq!(ids.candidate(), Some(12));
    }

    #[test]
    fn limit_refuses_a_candidate_without_wraparound() {
        let mut ids = ObjectIdSequence::<UPLOAD_LIMIT>::new();
        ids.observe_floor(UPLOAD_LIMIT);
        assert_eq!(ids.candidate(), None);
    }
}
