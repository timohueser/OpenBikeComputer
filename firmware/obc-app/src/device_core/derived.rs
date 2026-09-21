//! Level-triggered derived data: the one DeviceCore boundary that is not a command.
//!
//! A derived read is not an operation. Nobody asks for it once and waits: DeviceCore needs the
//! viewed ride's track, or the previewed route's shape, and keeps saying so until an answer for
//! exactly that subject arrives. Nothing here carries an [`OperationToken`](super::OperationToken),
//! because the key is the guard: an answer is wanted while its key equals the current need's key.
//!
//! A token says "this is the answer to the request I made"; a key says "this is the answer about
//! this subject, read at this revision, for this view". The second survives what the first cannot: a
//! need re-emitted over many passes, several executors answering, and an answer that arrives two
//! passes late. Change the ride, commit new bytes, or invalidate the view, and the key changes, so
//! the late answer is about something else and is dropped.
//!
//! A key has three parts. Identity says which object, as a durable [`CatalogObjectId`] and never a
//! catalog index, because an index moves under a live rescan. Source revision says which bytes, so
//! an upload that replaces a stored route under the same identity does not keep the old preview.
//! View revision says which presentation, and is bumped when the owner deliberately drops a derived
//! result without the subject or its bytes changing.
//!
//! [`DerivedResult::Failed`] is a matching answer: it clears the need for that key exactly like a
//! fill, so a dead file costs one read rather than one read per pass.

use crate::CatalogObjectId;

use super::Revision;

/// The subject of one derived ride-track read: the Ride detail's elevation profile and its
/// decimated track shape, both produced from the same stored ride object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RideTrackKey {
    pub ride: CatalogObjectId,
    /// The store revision the ride's bytes were last known to change at.
    pub source: Revision,
    /// The view generation this result must match, bumped by an explicit invalidate.
    pub view: Revision,
}

/// The subject of one derived navigation-preview read: the previewed route's decimated shape
/// polyline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavPreviewKey {
    /// Assistant reviews show a Visit through rejoin; ordinary overviews show the whole route.
    pub assistant: bool,
    pub route: CatalogObjectId,
    /// The store revision the route's bytes were last known to change at. A re-plan or a spliced
    /// detour commits new geometry under the same identity; this is what separates them.
    pub source: Revision,
    /// The view generation this result must match, bumped when a committed plan drops the old
    /// preview so the overview never opens on the shape of the route it replaced.
    pub view: Revision,
}

/// How one derived read ended. The target buffer stays DeviceCore-owned and the executor fills it
/// in place, so this says only whether it may be shown: never the data, and not a length either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerivedResult {
    /// The DeviceCore-owned target holds usable data and may be shown.
    Filled,
    /// The read did not produce usable data. This still answers the key: a source that cannot be
    /// read must not cause the same work on every pass.
    Failed,
}

impl DerivedResult {
    pub fn is_filled(self) -> bool {
        matches!(self, DerivedResult::Filled)
    }
}

/// One keyed answer: which subject it is about, and how the read ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DerivedInput<K> {
    /// The key the need carried when this read was started.
    pub key: K,
    pub result: DerivedResult,
}

impl<K> DerivedInput<K> {
    pub const fn filled(key: K) -> Self {
        DerivedInput { key, result: DerivedResult::Filled }
    }

    /// A failed read for `key`: an answer, not a retry.
    pub const fn failed(key: K) -> Self {
        DerivedInput { key, result: DerivedResult::Failed }
    }
}

/// What DeviceCore needs read right now: a level, recomputed from state on every pass and
/// re-emitted until a matching input is accepted. `None` means nothing is wanted, not "already
/// asked": an executor that misses a pass simply sees the need again on the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DerivedNeeds {
    /// The viewed ride's track, or `None` when no detail is open or the current key is answered.
    pub ride_track: Option<RideTrackKey>,
    /// The previewed route's shape, or `None` when no overview is open or the key is answered.
    pub nav_preview: Option<NavPreviewKey>,
}

impl DerivedNeeds {
    pub const NONE: DerivedNeeds = DerivedNeeds { ride_track: None, nav_preview: None };

    pub fn is_empty(&self) -> bool {
        self.ride_track.is_none() && self.nav_preview.is_none()
    }
}

/// The bulk that rides beside the keyed inputs: the small polyline targets, which are cheaper to
/// copy in than to expose as borrowed buffers.
///
/// Named fields rather than two positional `&[(i32, i32)]` parameters, because they are the same
/// type and swapping them would draw a ride's track over a route overview with nothing to catch it.
/// The profile target is absent for the opposite reason: at ~5 KiB it stays DeviceCore-owned and
/// the executor fills it in place.
#[derive(Debug, Clone, Copy, Default)]
pub struct DerivedTargets<'a> {
    pub ride_preview: &'a [(i32, i32)],
    pub nav_preview: &'a [(i32, i32)],
}

impl DerivedTargets<'_> {
    /// No polylines: an answer that carries none, such as a failure or an in-place profile fill.
    pub const NONE: DerivedTargets<'static> = DerivedTargets { ride_preview: &[], nav_preview: &[] };
}

/// The keyed answers arriving this pass — one optional input per need, mirroring [`DerivedNeeds`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DerivedInputs {
    pub ride_track: Option<DerivedInput<RideTrackKey>>,
    pub nav_preview: Option<DerivedInput<NavPreviewKey>>,
}

impl DerivedInputs {
    pub const NONE: DerivedInputs = DerivedInputs { ride_track: None, nav_preview: None };

    pub const fn ride_track(input: DerivedInput<RideTrackKey>) -> Self {
        DerivedInputs { ride_track: Some(input), ..DerivedInputs::NONE }
    }

    pub const fn nav_preview(input: DerivedInput<NavPreviewKey>) -> Self {
        DerivedInputs { nav_preview: Some(input), ..DerivedInputs::NONE }
    }
}

// Layout tripwires: identities, revisions and a count. The polylines and profiles these keys name
// are tens of times larger and stay where they are.
const _: () = assert!(core::mem::size_of::<RideTrackKey>() <= 24, "an identity and two revisions");
const _: () =
    assert!(core::mem::size_of::<Option<NavPreviewKey>>() <= 32, "presentation fits the existing optional key");
const _: () = assert!(core::mem::size_of::<DerivedResult>() <= 1, "a verdict, not a report");
const _: () = assert!(core::mem::size_of::<DerivedNeeds>() <= 64, "two optional keys");
const _: () = assert!(core::mem::size_of::<DerivedInputs>() <= 80, "two optional keyed answers");

#[cfg(test)]
mod tests {
    use super::*;

    fn key(ride: CatalogObjectId, source: u64, view: u64) -> RideTrackKey {
        RideTrackKey { ride, source: Revision::new(source), view: Revision::new(view) }
    }

    /// Each of the three parts of a key is load-bearing: a different subject, different bytes, or a
    /// different view is a different need.
    #[test]
    fn every_part_of_a_key_separates_two_needs() {
        let base = key(7, 2, 1);
        assert_eq!(base, key(7, 2, 1), "the same subject, bytes and view is the same need");
        assert_ne!(base, key(8, 2, 1), "a different ride");
        assert_ne!(base, key(7, 3, 1), "the same ride re-committed");
        assert_ne!(base, key(7, 2, 2), "the same bytes, an invalidated view");
    }

    /// A failure is an answer. Only one of the two results has data to show, which is how the need's
    /// owner stops re-asking without claiming a profile it never got.
    #[test]
    fn a_failure_answers_the_key_without_claiming_data() {
        let k = key(3, 1, 0);
        let failed = DerivedInput::failed(k);
        let filled = DerivedInput::filled(k);

        assert_eq!(failed.key, filled.key, "both answer the same need");
        assert!(!failed.result.is_filled());
        assert!(filled.result.is_filled());
        assert_eq!(filled.result, DerivedResult::Filled);
    }

    #[test]
    fn the_two_needs_are_separate_slots() {
        let ride = DerivedInput::filled(key(1, 0, 0));
        let inputs = DerivedInputs::ride_track(ride);
        assert_eq!(inputs.ride_track, Some(ride));
        assert!(inputs.nav_preview.is_none());

        let route = NavPreviewKey { assistant: false, route: 2, source: Revision::ZERO, view: Revision::ZERO };
        let inputs = DerivedInputs::nav_preview(DerivedInput::failed(route));
        assert!(inputs.ride_track.is_none());
        assert_eq!(inputs.nav_preview.map(|i| i.result), Some(DerivedResult::Failed));

        let mut needs = DerivedNeeds::NONE;
        assert!(needs.is_empty());
        needs.nav_preview = Some(route);
        assert!(!needs.is_empty());
    }
}
