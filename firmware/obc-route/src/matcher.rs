//! Forward-biased route matcher: snap a live position onto the loaded route.
//!
//! [`RouteMatch`] keeps a cursor `(chunk, segment, progress)` and, for each fix, searches a
//! bounded forward window around it for the nearest route segment, so the cost is O(window), not
//! O(route). Past a distance threshold it flags off-route, with hysteresis so the flag does not
//! flap on GPS jitter, and freezes progress so a far fix cannot drag the route position. It
//! widens the search at the same time, so a rejoin is still found. The forward bias stops a
//! loop's second pass from snapping back to the first.
//!
//! One reused chunk-decode buffer holds the handful of chunks the window spans, so matching
//! allocates nothing per fix.

use heapless::Vec;

use crate::geo::project_to_segment;
use crate::reader::{RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};
use obc_map_scene::{cos_lat, ground_dist_m_cl};

/// Cross-track distance (m) at or above which the rider is off-route.
const OFF_M: f32 = 25.0;
/// Cross-track distance (m) below which the rider is back on-route. The gap to [`OFF_M`] is the
/// hysteresis band that keeps the flag from flapping on GPS noise.
const ON_M: f32 = 15.0;
/// Segments of backward slack in the on-route search window. It absorbs a little GPS jitter
/// without losing the forward bias.
const BACK_SEGS: i64 = 3;
/// Forward search window (segments) while on-route. One fix's travel is far less than this
/// at any cycling speed, so the nearest segment is well inside it.
const FWD_SEGS_ON: i64 = 64;
/// Wider forward window while off-route, so a rejoin further along the route is found
/// without an unbounded full scan.
const FWD_SEGS_OFF: i64 = 320;
/// Tie-break margin (m) for the first lock only. The initial scan runs front-to-back, so this
/// margin keeps the earliest of several near-equal matches. On an out-and-back a few metres of
/// cross-track offset would otherwise latch the cursor onto the finish, and the forward bias
/// could never follow the outbound leg.
const TIE_EPS_M: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    /// Distance travelled along the route to the matched point (m), clamped to the route length.
    /// It is frozen while off-route.
    pub progress_m: u32,
    pub off_route: bool,
    /// Cross-track distance from the fix to the nearest route point (m). It stays live while
    /// off-route. `u32::MAX` means no usable segment was decoded.
    pub dist_m: u32,
}

/// A scan's nearest candidate: `(chunk, seg, dist_m, progress_m)`.
type Best = (usize, usize, f32, u32);

#[derive(Clone, Copy)]
enum RecoveryScan {
    Nearest,
    Check { progress_m: u32, dist_m: u32 },
}

/// A forward-biased cursor that snaps fixes to a route. One per active route, reset on route
/// load or change.
pub struct RouteMatch {
    chunk: usize,
    seg: usize,
    progress_m: u32,
    /// Durable lower bound installed by a skip-ahead commit. It survives off-route fixes and
    /// stops the backward slack from re-entering the skipped stretch.
    floor_progress_m: u32,
    /// Global segment containing `floor_progress_m`; segments before it are not candidates.
    floor_global_seg: u32,
    off_route: bool,
    /// `false` until the first fix has been matched. That first match scans the whole route, to
    /// lock on from anywhere.
    started: bool,
    /// Widen the next match's forward window to the rejoin window, then clear. Set when the
    /// caller knows fixes went unmatched, so the cursor is stale by more than one fix's travel.
    wide_next: bool,
    buf: Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
}

impl Default for RouteMatch {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteMatch {
    pub fn new() -> Self {
        RouteMatch {
            chunk: 0,
            seg: 0,
            progress_m: 0,
            floor_progress_m: 0,
            floor_global_seg: 0,
            off_route: false,
            started: false,
            wide_next: false,
            buf: Vec::new(),
        }
    }

    /// Forget all match state, for a route load or swap.
    pub fn reset(&mut self) {
        self.chunk = 0;
        self.seg = 0;
        self.progress_m = 0;
        self.floor_progress_m = 0;
        self.floor_global_seg = 0;
        self.off_route = false;
        self.started = false;
        self.wide_next = false;
        self.buf.clear();
    }

    /// Install a forward-only navigation floor at `progress_m` and move the cursor to the
    /// containing segment. The route bytes do not change. Returns the clamped position, or `None`
    /// when the route has no decodable geometry.
    pub fn set_progress_floor(&mut self, route: &RouteReader, progress_m: u32) -> Option<crate::reader::RoutePosition> {
        let progress_m = progress_m.max(self.progress_m).max(self.floor_progress_m);
        let pos = route.locate_progress(progress_m, &mut self.buf)?;
        self.chunk = pos.chunk;
        self.seg = pos.seg;
        self.progress_m = pos.progress_m;
        self.floor_progress_m = pos.progress_m;
        self.floor_global_seg = route.global_seg_index(pos.chunk, pos.seg) as u32;
        self.off_route = false;
        self.started = true;
        Some(pos)
    }

    /// Whether the matcher has locked onto the route at least once. Before that, `progress_m` is
    /// a default 0 that nothing must derive from.
    pub fn started(&self) -> bool {
        self.started
    }

    /// Stored chunk/segment occurrence, distinct at repeated coordinates on the same route.
    pub fn occurrence(&self) -> u32 {
        ((self.chunk as u32) << 16) | self.seg as u32
    }

    /// Resolve a persisted progress anchor without advancing the live match before its ACK.
    pub fn occurrence_at(&mut self, route: &RouteReader, progress_m: u32) -> Option<u32> {
        let pos = route.locate_progress(progress_m, &mut self.buf)?;
        Some(((pos.chunk as u32) << 16) | pos.seg as u32)
    }

    /// Ask the next match to search the wide rejoin window once, then fall back to the tight one.
    ///
    /// Call it when fixes went unmatched while the cursor stood still, such as the pause during a
    /// route search that ends with the same route under the rider. The tight window reaches only
    /// one fix's travel ahead, so the first fix after such a pause would otherwise find nothing.
    pub fn relock_wide(&mut self) {
        self.wide_next = true;
    }

    /// Match `(lon, lat)` (microdegrees) onto `route`, advancing the cursor.
    pub fn update(&mut self, lon: i32, lat: i32, route: &RouteReader) -> Match {
        self.update_to(lon, lat, route, u32::MAX)
    }

    /// Match only the accepted phase. A later overlapping return leg is not an outbound candidate.
    pub fn update_to(&mut self, lon: i32, lat: i32, route: &RouteReader, ceiling_m: u32) -> Match {
        self.update_window(lon, lat, route, ceiling_m, None)
    }

    /// Recover a unique position inside a durable phase without the live matcher's forward bias.
    /// The second scan rejects near-equal projections onto separated parts of the phase.
    pub fn recover(&mut self, lon: i32, lat: i32, route: &RouteReader, lower_m: u32, upper_m: u32) -> Option<Match> {
        self.reset();
        self.set_progress_floor(route, lower_m)?;
        self.started = false;
        let nearest = self.update_window(lon, lat, route, upper_m, Some(RecoveryScan::Nearest));
        if nearest.off_route {
            return None;
        }
        self.started = false;
        let checked = self.update_window(
            lon,
            lat,
            route,
            upper_m,
            Some(RecoveryScan::Check { progress_m: nearest.progress_m, dist_m: nearest.dist_m }),
        );
        (!checked.off_route).then_some(checked)
    }

    /// The first-lock scan without a cursor to move: the route point nearest `(lon, lat)`, biased
    /// to the earliest. A later candidate replaces the kept one only when it is more than `tie_m`
    /// nearer; the live first lock uses 8 m. `off_route` says whether a lock would follow from
    /// there. `None` when no segment decodes.
    #[inline(never)]
    pub fn nearest(lon: i32, lat: i32, route: &RouteReader, tie_m: f32) -> Option<Match> {
        let mut probe = RouteMatch::new();
        let (best, _) = probe.scan((lon, lat), route, u32::MAX, None, tie_m).ok()?;
        best.map(|(_, _, dist, progress_m)| Match { progress_m, off_route: dist >= OFF_M, dist_m: dist as u32 })
    }

    fn update_window(
        &mut self,
        lon: i32,
        lat: i32,
        route: &RouteReader,
        ceiling_m: u32,
        recovery: Option<RecoveryScan>,
    ) -> Match {
        if route.chunks().is_empty() {
            return Match { progress_m: 0, off_route: true, dist_m: u32::MAX };
        }
        let Ok((best, ambiguous)) = self.scan((lon, lat), route, ceiling_m, recovery, TIE_EPS_M) else {
            return Match { progress_m: self.progress_m, off_route: true, dist_m: u32::MAX };
        };
        let Some((bc, bs, bdist, bprog)) = best else {
            // No segment in range, which happens only with a 1-point route.
            return Match { progress_m: self.progress_m, off_route: true, dist_m: u32::MAX };
        };

        let now_off = if ambiguous || bdist >= OFF_M {
            true
        } else if bdist < ON_M {
            false
        } else {
            self.off_route
        };
        self.off_route = now_off;
        self.started = true;

        // Advance only when on-route, so a far fix cannot drag progress.
        if !now_off {
            self.chunk = bc;
            self.seg = bs;
            self.progress_m = bprog;
        }
        Match { progress_m: self.progress_m, off_route: now_off, dist_m: bdist as u32 }
    }

    /// Search the window the cursor state selects for the nearest segment to `p`. Returns the best
    /// candidate and whether a recovery check found it ambiguous. `Err` when a recovery scan meets a
    /// chunk it cannot decode.
    fn scan(
        &mut self,
        p: (i32, i32),
        route: &RouteReader,
        ceiling_m: u32,
        recovery: Option<RecoveryScan>,
        tie_m: f32,
    ) -> Result<(Option<Best>, bool), ()> {
        let chunks = route.chunks();
        let total = route.total_distance_m;
        let cur_gidx = route.global_seg_index(self.chunk, self.seg) as i64;

        // The first lock, an off-route rejoin and a requested re-lock scan wide. The re-lock
        // request is consumed here whichever branch wins, so it costs at most one wide search.
        let wide_relock = core::mem::take(&mut self.wide_next);
        let (first_chunk, back, fwd) = if !self.started {
            (0usize, i64::MAX, i64::MAX) // first lock: whole route
        } else if self.off_route || wide_relock {
            (self.chunk.saturating_sub(1), BACK_SEGS, FWD_SEGS_OFF)
        } else {
            (self.chunk.saturating_sub(1), BACK_SEGS, FWD_SEGS_ON)
        };

        let mut best: Option<Best> = None;
        let mut ambiguous = false;
        let mut c = first_chunk;
        let mut base_gidx = route.global_seg_index(first_chunk, 0) as i64;
        'outer: while c < chunks.len() {
            // Segments only run forward, so a chunk past the window ends the scan.
            if self.started && base_gidx - cur_gidx > fwd {
                break;
            }
            let pc_segs = (chunks[c].point_count as usize).saturating_sub(1) as i64;
            let decoded = route.decode_chunk(c, &mut self.buf).is_ok();
            if !decoded && recovery.is_some() {
                return Err(());
            }
            if decoded && self.buf.len() >= 2 {
                let cum0 = chunks[c].cum_distance_m as f32;
                let mut intra = 0f32; // distance from this chunk's anchor to point s
                                      // cos(lat) barely changes across one chunk's span, so hoist it once per
                                      // chunk rather than recomputing `cosf` for every segment of the window.
                let cl = cos_lat(self.buf[0].lat);
                let n = self.buf.len();
                for s in 0..n - 1 {
                    let off = base_gidx + s as i64 - cur_gidx;
                    let global = (base_gidx + s as i64).max(0) as u32;
                    if self.started && off > fwd {
                        break 'outer;
                    }
                    let a = (self.buf[s].lon, self.buf[s].lat);
                    let b = (self.buf[s + 1].lon, self.buf[s + 1].lat);
                    let seg_len = ground_dist_m_cl(a, b, cl);
                    if cum0 + intra > ceiling_m as f32 {
                        break 'outer;
                    }
                    if (!self.started || off >= -back) && global >= self.floor_global_seg {
                        let (mut t, mut dist) = project_to_segment(a, b, p, cl);
                        let mut progress = (cum0 + intra + t * seg_len) as u32;
                        if progress > ceiling_m {
                            t = if seg_len > 1e-3 {
                                ((ceiling_m as f32 - cum0 - intra) / seg_len).clamp(0.0, 1.0)
                            } else {
                                0.0
                            };
                            let ceiling = (
                                a.0 + libm::roundf((b.0 - a.0) as f32 * t) as i32,
                                a.1 + libm::roundf((b.1 - a.1) as f32 * t) as i32,
                            );
                            dist = ground_dist_m_cl(ceiling, p, cl);
                            progress = ceiling_m;
                        }
                        // The floor can sit inside its containing segment. A fix earlier on that
                        // same long segment must measure to the floor point, not project behind it
                        // and appear on-route inside the skipped stretch.
                        if progress < self.floor_progress_m {
                            t = if seg_len > 1e-3 {
                                ((self.floor_progress_m as f32 - cum0 - intra) / seg_len).clamp(0.0, 1.0)
                            } else {
                                0.0
                            };
                            let floor = (
                                a.0 + libm::roundf((b.0 - a.0) as f32 * t) as i32,
                                a.1 + libm::roundf((b.1 - a.1) as f32 * t) as i32,
                            );
                            dist = ground_dist_m_cl(floor, p, cl);
                            progress = self.floor_progress_m;
                        }
                        if let Some(RecoveryScan::Check { progress_m: nearest_m, dist_m: nearest_dist }) = recovery {
                            let separation = progress.abs_diff(nearest_m) as f32;
                            let nearest_dist = nearest_dist as f32;
                            // Adjacent projections around one location are harmless. Separate
                            // near-ties or repeated coordinates do not identify one occurrence.
                            ambiguous |= (dist <= nearest_dist + TIE_EPS_M
                                && separation > 2.0 * (nearest_dist + TIE_EPS_M))
                                || (dist <= nearest_dist + 1.0 && separation > 2.0 * (nearest_dist + 1.0));
                        }
                        // The first lock biases near-ties to the earliest segment. Once tracking,
                        // the forward window bounds the search, so a strict nearest is right.
                        let better = match best {
                            None => true,
                            Some((_, _, bd, _)) if self.started || recovery.is_some() => dist < bd,
                            Some((_, _, bd, _)) => dist < bd - tie_m,
                        };
                        if better {
                            best = Some((c, s, dist, progress.min(total)));
                        }
                    }
                    intra += seg_len;
                }
            }
            base_gidx += pc_segs;
            c += 1;
        }
        Ok((best, ambiguous))
    }
}
