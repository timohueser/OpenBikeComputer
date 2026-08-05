//! One replay frame: advance the GPX playback and tick the shared app on the playback clock.

use obc_app::App;
use obc_ports::{CadenceSource, CompassSource, HeartRateSource, PowerSource, RideClock, Sensors, TrackSink};
use obc_replay::{BaroSensor, GpxPlayer};
use obc_route::RouteReader;

/// The starting camera for a freshly-opened map: centered on the bbox, zoomed so
/// its longitude span fills the window width. Returns `(cam_lon, cam_lat, zoom)`
/// in the [`AppState`](obc_app::AppState) convention (microdegrees, pixels-per-microdegree).
pub fn initial_camera(reader: &obc_reader::Reader, width: u32) -> (i32, i32, f32) {
    let b = reader.bbox;
    let cam_lon = (b.min_lon as i64 + b.max_lon as i64) / 2;
    let cam_lat = (b.min_lat as i64 + b.max_lat as i64) / 2;
    let span_lon = (b.max_lon as i64 - b.min_lon as i64).max(1) as f32;
    (cam_lon as i32, cam_lat as i32, width as f32 / span_lon)
}

/// The optional synthetic BLE-sensor sources (HR / power / cadence) a host can drive alongside the
/// replay (epic #707 SE8). Bundled so [`replay_step`]'s signature doesn't grow three parameters;
/// every field defaults to `None` (a plain GPX replay with no sensors — the [`Default`]).
#[derive(Default)]
pub struct ReplaySensors<'s> {
    pub hr: Option<&'s mut dyn HeartRateSource>,
    pub power: Option<&'s mut dyn PowerSource>,
    pub cadence: Option<&'s mut dyn CadenceSource>,
}

/// Advance the GPX replay by `dt` seconds and run one app tick on the **playback**
/// clock. The millis derive from playback-time (not wall-clock), so Avg. Speed isn't
/// scaled by the replay-speed multiplier. Shared by the live hosts' frame loops and
/// `obc-sim`'s headless `--png` replay.
///
/// `sensors` carries the optional synthetic HR/power/cadence sources (SE8); pass
/// [`ReplaySensors::default()`] for a plain replay. A host feeds those sources on the same playback
/// clock **before** calling this (so a sample is stamped onto the point this tick logs).
// Each argument models a distinct sensor seam the app tick binds together; bundling them further
// would just relocate the same fan-out behind an opaque struct.
#[allow(clippy::too_many_arguments)]
pub fn replay_step<'s>(
    app: &mut App,
    player: &'s mut GpxPlayer,
    baro: &'s mut BaroSensor,
    compass: Option<&'s mut dyn CompassSource>,
    dt: f64,
    route: Option<&RouteReader>,
    track: Option<&'s mut dyn TrackSink>,
    sensors: ReplaySensors<'s>,
) {
    // The sensor handles share one lifetime `'s` so the invariant `Sensors<'a>` can bind them
    // together. The compass only matters while stationary (GPS course drops to `None`).
    player.advance(dt);
    baro.feed(player.elevation_at(player.time()), player.time());
    let now_ms = (player.time() * 1000.0) as u32;
    let sensors = Sensors {
        altimeter: Some(baro),
        compass,
        track,
        hr: sensors.hr,
        power: sensors.power,
        cadence: sensors.cadence,
        ..Sensors::new(player)
    };
    app.tick(RideClock(now_ms), sensors, route);
}
