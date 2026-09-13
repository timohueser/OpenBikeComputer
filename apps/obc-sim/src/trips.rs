//! Import legacy fixture trip references into the card's object namespace.

use obc_formats::io::SliceSource;
use obc_host_core::VecSink;
use obc_route::{write_trip, TripMeta};

pub fn remap(bytes: &[u8], routes: &[u64]) -> Result<Vec<u8>, String> {
    let trip = TripMeta::read(&SliceSource(bytes)).map_err(|error| format!("invalid trip: {error:?}"))?;
    if trip.truncated {
        return Err("trip exceeds stage capacity".into());
    }
    // ObjectId zero is reserved and can never alias a later committed route.
    let stages: Vec<_> = trip
        .stage_ids
        .iter()
        .map(|&id| usize::try_from(id).ok().and_then(|index| routes.get(index)).copied().unwrap_or(0))
        .collect();
    let mut out = VecSink::default();
    write_trip(trip.name.as_str(), &stages, &mut out).map_err(|error| format!("encode trip: {error:?}"))?;
    Ok(out.bytes().to_vec())
}

#[cfg(test)]
mod tests {
    #[test]
    fn fixture_stages_resolve_to_committed_ids_and_missing_stays_reserved() {
        let input = include_bytes!("../../../fixtures/sources/sim-grimsel/routes/TP1.OBT");
        let output = super::remap(input, &[9, u64::MAX - 1]).unwrap();
        let trip = obc_route::TripMeta::read(&obc_formats::io::SliceSource(&output)).unwrap();
        assert_eq!(&trip.stage_ids[..], &[9, u64::MAX - 1, 0]);
        assert_eq!(trip.name.as_str(), "Alpen Traverse");
    }
}
