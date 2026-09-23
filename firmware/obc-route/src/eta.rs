//! Time to go along a route: the ride's ETA and TIME TO GO tiles.
//!
//! The estimate itself is [`BikeType::ride_time_s`], the contract table of `OBCR_Spec.md` §1.2
//! that the phone shares. This module only feeds it the remaining distance and the remaining
//! ascent from [`Profile::ascent_between_m`], the curve the elevation profile already builds.

use crate::profile::Profile;
use obc_formats::bike::BikeType;

/// Estimated time (s) still to ride from `progress_m` to the end. Both remaining distance and
/// remaining ascent fall as `progress_m` advances, so the readout can only count down. Past the
/// end it is `0`.
pub fn time_to_go_s(ele: &Profile, route_total_m: u32, progress_m: u32, bike: BikeType) -> u32 {
    let dist = route_total_m.saturating_sub(progress_m);
    let ascent = ele.ascent_between_m(progress_m, route_total_m, route_total_m);
    bike.ride_time_s(dist, ascent)
}
