//! One replay frame: advance the GPX playback and tick the shared app on the playback clock.

use obc_app::{App, CompassSource, RideClock, Sensors, TrackSink};
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

/// Advance the GPX replay by `dt` seconds and run one app tick on the **playback**
/// clock. The millis derive from playback-time (not wall-clock), so Avg. Speed isn't
/// scaled by the replay-speed multiplier. Shared by the live hosts' frame loops and
/// `obc-sim`'s headless `--png` replay.
pub fn replay_step<'s>(
    app: &mut App,
    player: &'s mut GpxPlayer,
    baro: &'s mut BaroSensor,
    compass: Option<&'s mut dyn CompassSource>,
    dt: f64,
    route: Option<&RouteReader>,
    track: Option<&'s mut dyn TrackSink>,
) {
    // The sensor handles share one lifetime `'s` so the invariant `Sensors<'a>` can bind them
    // together. The compass only matters while stationary (GPS course drops to `None`).
    player.advance(dt);
    baro.feed(player.elevation_at(player.time()), player.time());
    let now_ms = (player.time() * 1000.0) as u32;
    let sensors = Sensors {
        loc: player,
        altimeter: Some(baro),
        temperature: None,
        clock: None,
        compass,
        track,
        fuel: None,
        hr: None,
        power: None,
        cadence: None,
    };
    app.tick(RideClock(now_ms), sensors, route);
}
