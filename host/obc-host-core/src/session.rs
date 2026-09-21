//! The parsed active-route session — a host holds one for the session's lifetime and reparses the
//! ~6.7 KB [`RouteIndex`] only when the active route's bytes actually change: a selection change,
//! a re-route rewrite, an import. A settled Map view reparses nothing.

use obc_app::App;
use obc_route::{RouteIndex, RouteReader};

use crate::RouteRepository;

/// The resident parse of the active route's `RouteIndex`, kept across frames. Its cache identity
/// (the [`RouteIndex`]'s non-persisted `identity`) rides with the parse, so a settled Map view
/// re-uses one parse indefinitely.
///
/// The ~8 KB `RouteIndex` is **boxed**: a host holds this session as a field beside its app, and a
/// host that holds it as a stack value (the deep sim tour test) must not carry a resident 8 KB
/// inline field down the render call chain. The heap slot keeps the host pointer-small.
#[derive(Default)]
pub struct ActiveRouteSession {
    index: Option<Box<RouteIndex>>,
}

impl ActiveRouteSession {
    /// A session with nothing parsed yet.
    pub const fn new() -> Self {
        ActiveRouteSession { index: None }
    }

    /// Reparse the active route's index **only when the store just re-read its bytes** (`changed`,
    /// the [`RouteRepository::sync_active`] return) — otherwise keep the resident parse. A cleared
    /// active source drops the index. Call right after `sync_active`.
    pub fn reparse(&mut self, changed: bool, routes: &dyn RouteRepository) {
        if !changed {
            return;
        }
        self.index = routes.active_source().and_then(|s| RouteIndex::read(s).ok()).map(Box::new);
    }

    /// The resident parse, for building a [`RouteReader`] over the store's active bytes.
    pub fn index(&self) -> Option<&RouteIndex> {
        self.index.as_deref()
    }

    /// Point the store at the app's active route and reparse only if its bytes moved — the two
    /// lines every frame-stepped host runs before it opens the reader it lends to the pass *and*
    /// to its render.
    pub fn sync(&mut self, app: &App, routes: &mut dyn RouteRepository) {
        let changed = routes.sync_active(app.active_route_index());
        self.reparse(changed, routes);
    }
}

/// Fill an open Route overview's decimated shape preview — the once-per-entry cue
/// every frame-stepped host shares. `nav_preview_missing` is false again the moment the copy lands,
/// so this is a per-frame no-op otherwise.
pub fn fill_nav_preview(app: &mut App, route: Option<&RouteReader>) {
    if let Some(key) = app.derived_needs().nav_preview {
        let points = route.and_then(|r| {
            if key.assistant {
                r.assistant_preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>().ok()
            } else {
                Some(r.preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>())
            }
        });
        use obc_app::device_core::{DerivedInput, DerivedInputs, DerivedTargets};
        let input = if points.is_some() { DerivedInput::filled(key) } else { DerivedInput::failed(key) };
        app.apply_derived(
            DerivedInputs::nav_preview(input),
            DerivedTargets { nav_preview: points.as_deref().unwrap_or(&[]), ..DerivedTargets::NONE },
        );
    }
}
