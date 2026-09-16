//! Assistant source admission and terminal planner cleanup.
use obc_app::navigator::ReviewContext;
use obc_storage::flat::{BlockDevice, FlatStore, ObjectId, Revision, Store, StoreSource};

pub(crate) fn original_allowed<D: BlockDevice>(
    store: &FlatStore<D>,
    context: ReviewContext,
    active: Option<u64>,
) -> bool {
    let Some(expected) = context.original else {
        return context.accepts_original(active, None, false);
    };
    let current = store.entries().find(|entry| {
        entry.id.0 == expected.object
            && entry.kind == obc_storage::flat::ObjectKind::Route
            && entry.flags.is_route_head()
    });
    if !store.entries_ok() || current.map(obc_storage::flat::metadata::fingerprint) != Some(expected) {
        return false;
    }
    store
        .with_source(ObjectId(expected.object), Some(Revision(expected.revision)), |source| {
            obc_route::RouteObjectInfo::read(source)
                .is_ok_and(|info| context.accepts_original(active, Some(expected), info.unresolved_avoidance))
        })
        .unwrap_or(false)
}

/// Close before release acknowledgement; a valid preview or uncertain publication keeps its source.
pub(crate) fn release_original<D: BlockDevice>(
    store: &FlatStore<D>,
    original: &mut Option<StoreSource<'_, D>>,
    retain: bool,
) {
    if !retain {
        if let Some(source) = original.take() {
            store.close(source.release());
        }
    }
}

/// Read the exact candidate shape through an index already owned by the planner grant.
#[inline(never)]
pub(crate) fn preview_shape(
    index: &mut obc_route::RouteIndex,
    source: &dyn obc_formats::io::ByteSource,
) -> Result<heapless::Vec<(i32, i32), { obc_app::NAV_PREVIEW_MAX }>, obc_formats::io::Error> {
    index.read_into(source)?;
    let route = obc_route::RouteReader::new(index, source);
    route.assistant_preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>()
}
