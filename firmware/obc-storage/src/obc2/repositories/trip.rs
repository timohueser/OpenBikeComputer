//! The trip repository — shaped, not finished.
//!
//! It is here to prove one thing: that the repositories are **per-kind concrete types**, not
//! instances of a shared one. A trip's catalog projection (§4.3) requires a display name and an
//! ordered stage count; a route's requires a name and a retention. Neither field list is a subset of
//! the other, neither derivation reads the same bytes, and the trip's Put schema has no fields at
//! all where the route's has retention. There is no honest common method here to hoist.
//!
//! What it does today:
//!
//! - it owns the registry facts that are already frozen — §1's operation matrix row (put, get,
//!   delete, and **no** set-metadata: "trip name and stages change through payload replacement"),
//!   and §6's three detail codes;
//! - it publishes §5.3's bare reservation rather than a projection it cannot derive.
//!
//! What it does not do, and why not: `Device_Object_Registries_v2.md` §1 asks DOS5 to decide whether
//! Trip survives as an object kind at all — "Keep Trip as a separate object kind only if the issue
//! records the independent create/replace/delete/list requirement that forces it" — and #1356 opens
//! with that audit. Deriving a stage count from a payload format that may not survive the audit
//! would be work spent proving the wrong thing. So the seam is real, the policy is real, and the
//! payload rules wait for the decision that fixes them.

use obc_link::frame::Opcode;
use obc_link::ids::{LogicalObjectId, Revision};
use obc_link::registry::{subject_flags, ObjectKind};

use crate::obc2::transaction::{CatalogProjection, KernelMedia, SealedBytes, Validation};

use super::{Capability, HeadView};

/// `Device_Object_Registries_v2.md` §6's trip details.
pub mod detail {
    /// The payload is not a trip.
    pub const INVALID_TRIP_FORMAT: u16 = 1;
    /// The same route appears twice in the stage list.
    pub const DUPLICATE_ROUTE_REFERENCE: u16 = 2;
    /// A stage names a route this store does not hold.
    pub const MISSING_TRIP_ROUTE: u16 = 3;
}

/// The subject operation flags §1's matrix permits a trip. Note the absent set-metadata bit: a
/// device that advertised it would be nonconforming.
pub const OPERATIONS: u16 = subject_flags::PUT | subject_flags::GET | subject_flags::DELETE;

/// The resident trip repository.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TripRepository;

impl TripRepository {
    /// The typed validation seam, at the shape a finished trip repository will keep.
    pub fn validate(
        &mut self,
        subject: &Validation<'_>,
        _bytes: &mut dyn SealedBytes,
    ) -> Result<CatalogProjection, u16> {
        match subject.opcode {
            // §1's matrix gives trip no set-metadata bit, so the engine refuses the request as
            // `unsupportedCapability` long before a claim exists. Reaching here means the profile
            // and the registry disagree, which is this device's invariant break, not a client error
            // — and the only refusal shape available at this seam is the kind's own detail.
            Opcode::SetMetadata => Err(detail::INVALID_TRIP_FORMAT),
            // Until the payload rules land, a trip head carries §5.3's reservation. That is why
            // `QueryCatalog` stays unadvertised: a page built from reservations is well-formed and
            // wrong, and a client cannot tell the difference.
            _ => Ok(CatalogProjection::RESERVATION),
        }
    }
}

/// The borrowed trip repository.
pub struct Trips<'a, M: KernelMedia> {
    capability: Capability<'a, M>,
}

impl<'a, M: KernelMedia> Trips<'a, M> {
    /// Wraps a lent capability.
    pub(in crate::obc2) fn new(capability: Capability<'a, M>) -> Self {
        Trips { capability }
    }

    /// §4's repository revision for trips.
    pub fn revision(&self) -> Revision {
        self.capability.revision(ObjectKind::Trip)
    }

    /// The head a logical trip stands at.
    pub fn resolve(&self, logical_object_id: LogicalObjectId) -> Option<HeadView> {
        self.capability.head(ObjectKind::Trip, logical_object_id)
    }

    /// One page of trips in logical-ID order.
    pub fn list(&self, after: Option<LogicalObjectId>, out: &mut [HeadView]) -> usize {
        self.capability.page(ObjectKind::Trip, after, out)
    }

    /// How many trips the store holds.
    pub fn count(&self) -> usize {
        self.capability.count(ObjectKind::Trip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_link::registry::semantic;

    #[test]
    fn the_trip_namespace_matches_the_registry_and_the_matrix_row() {
        for (code, name) in [
            (detail::INVALID_TRIP_FORMAT, "invalidTripFormat"),
            (detail::DUPLICATE_ROUTE_REFERENCE, "duplicateRouteReference"),
            (detail::MISSING_TRIP_ROUTE, "missingTripRoute"),
        ] {
            assert_eq!(semantic::lookup(ObjectKind::Trip, code).expect("registered").name, name);
        }
        // §1: "Trip … carries no set-metadata bit"; the matrix is normative and a `no` is too.
        assert_eq!(OPERATIONS & subject_flags::SET_METADATA, 0);
        assert_eq!(OPERATIONS, subject_flags::PUT | subject_flags::GET | subject_flags::DELETE);
    }
}
