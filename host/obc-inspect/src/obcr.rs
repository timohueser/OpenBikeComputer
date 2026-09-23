//! OBCR routes and the trip object, read through `obc-route`.

use obc_formats::io::ByteSource;
use obc_route::{for_each_waypoint, read_trip_day, RouteIndex, RouteObjectInfo, TripSummary};

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

/// The trip object (`.obt`): a name, a key, a start date and the days in ride order. It carries no
/// magic, so the file name selects it.
pub fn trip(source: &dyn ByteSource) -> Result<Report, String> {
    let summary = TripSummary::read(source).map_err(|error| format!("trip header: {error:?}"))?;
    let mut version = [0u8; 1];
    source.read_at(0, &mut version).map_err(|error| format!("trip header: {error:?}"))?;

    let mut out = Report::new();
    out.put("version", version[0]).put("bytes", source.len());
    out.put("name", summary.name.as_str())
        .put("key", format!("{:#018x}", summary.key))
        .put("start_date", summary.start_date)
        .put("day_count", summary.day_count);
    let mut days = Vec::new();
    for k in 0..summary.day_count {
        let day = read_trip_day(source, k).map_err(|error| format!("trip day {k}: {error:?}"))?;
        let mut row = Report::new();
        row.put("route", day.route.to_string()).put("join_m", day.join_m).put("leave_m", day.leave_m);
        days.push(row);
    }
    out.list("days", days);
    Ok(out)
}
