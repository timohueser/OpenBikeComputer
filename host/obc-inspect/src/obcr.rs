//! OBCR routes and the trip object, read through `obc-route`.

use obc_formats::io::ByteSource;
use obc_route::{for_each_waypoint, RouteIndex, RouteObjectInfo, TripMeta, TripSummary};

use crate::report::{degrees, Report};

pub fn route(source: &dyn ByteSource) -> Result<Report, String> {
    let mut version = [0u8; 5];
    source.read_at(0, &mut version).map_err(|error| format!("OBCR header: {error:?}"))?;
    let index = RouteIndex::read(source).map_err(|error| format!("OBCR route: {error:?}"))?;
    let info = RouteObjectInfo::read(source).map_err(|error| format!("OBCR header extension: {error:?}"))?;

    let mut out = Report::new();
    out.put("version", version[4]).put("bytes", source.len());
    out.put("name", index.name()).put("points", index.point_count).put("chunks", index.chunks().len());
    out.put("distance_m", index.total_distance_m)
        .put("ascent_m", index.total_ascent_m)
        .put("descent_m", index.total_descent_m);

    let mut elevation = Report::new();
    elevation.put("present", index.has_elevation()).put("min_m", index.min_ele_m).put("max_m", index.max_ele_m);
    out.group("elevation", elevation);

    let mut bbox = Report::new();
    bbox.put("min_lat", degrees(index.bbox.min_lat as i64))
        .put("min_lon", degrees(index.bbox.min_lon as i64))
        .put("max_lat", degrees(index.bbox.max_lat as i64))
        .put("max_lon", degrees(index.bbox.max_lon as i64));
    out.group("bbox", bbox);

    let mut flags = Report::new();
    flags
        .put("assistant_candidate", index.is_assistant_candidate())
        .put("unresolved_avoidance", index.has_unresolved_avoidance())
        .put("attribution_map", info.attribution_map.is_some())
        .put("visit_descriptor", info.visit.is_some());
    out.group("flags", flags);

    let mut waypoints = Vec::new();
    let stored = for_each_waypoint(source, |waypoint| {
        let mut row = Report::new();
        row.put("name", waypoint.name.as_str())
            .put("lat", degrees(waypoint.lat as i64))
            .put("lon", degrees(waypoint.lon as i64))
            .put("dist_along_m", waypoint.dist_along_m)
            .put("category", waypoint.category().map_or(waypoint.category_id.to_string(), |c| format!("{c:?}")));
        waypoints.push(row);
    })
    .map_err(|error| format!("OBCR waypoints: {error:?}"))?;
    out.put("waypoint_count", stored);
    out.list("waypoints", waypoints);
    Ok(out)
}

/// The trip object (`.obt`): a name and the ordered route ids it stages. It carries no magic, so
/// the file name selects it.
pub fn trip(source: &dyn ByteSource) -> Result<Report, String> {
    let summary = TripSummary::read(source).map_err(|error| format!("trip header: {error:?}"))?;
    let meta = TripMeta::read(source).map_err(|error| format!("trip stages: {error:?}"))?;
    let mut version = [0u8; 1];
    source.read_at(0, &mut version).map_err(|error| format!("trip header: {error:?}"))?;

    let mut out = Report::new();
    out.put("version", version[0]).put("bytes", source.len());
    out.put("name", summary.name.as_str()).put("stage_count", summary.stage_count);
    out.put("stages", meta.stage_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>());
    if meta.truncated {
        out.put("listed", meta.stage_ids.len());
    }
    Ok(out)
}
