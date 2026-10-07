//! Import legacy fixture trip references into the card's object namespace.

use obc_formats::io::SliceSource;
use obc_host_core::VecSink;
use obc_route::{read_trip_day, write_trip, TripDay, TripMeta};

/// Rewrite a fixture trip whose day routes are indices into `routes` so each day names the
/// committed route id. Every other field is kept.
pub fn remap(bytes: &[u8], routes: &[u64]) -> Result<Vec<u8>, String> {
    let source = SliceSource(bytes);
    let trip = TripMeta::read(&source).map_err(|error| format!("invalid trip: {error:?}"))?;
    if trip.truncated {
        return Err("trip exceeds day capacity".into());
    }
    let days = (0..trip.day_routes.len() as u16)
        .map(|k| {
            let day = read_trip_day(&source, k).map_err(|error| format!("invalid trip day {k}: {error:?}"))?;
            // ObjectId zero is reserved and can never alias a later committed route.
            let route = usize::try_from(day.route).ok().and_then(|index| routes.get(index)).copied().unwrap_or(0);
            Ok(TripDay { route, ..day })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut out = VecSink::default();
    write_trip(trip.key, trip.name.as_str(), trip.start_date, &days, &mut out)
        .map_err(|error| format!("encode trip: {error:?}"))?;
    Ok(out.bytes().to_vec())
}

#[cfg(test)]
mod tests {
    #[test]
    fn fixture_days_resolve_to_committed_ids_and_missing_stays_reserved() {
        let input = include_bytes!("../../../fixtures/sources/sim-grimsel/routes/TP1.OBT");
        let output = super::remap(input, &[9, u64::MAX - 1]).unwrap();
        let trip = obc_route::TripMeta::read(&obc_formats::io::SliceSource(&output)).unwrap();
        assert_eq!(&trip.day_routes[..], &[9, u64::MAX - 1, 0]);
        assert_eq!(trip.name.as_str(), "Alpen Traverse");
        assert_eq!(trip.start_date, 20_360, "the fixture trip starts on Monday 2025-09-29");
    }
}
