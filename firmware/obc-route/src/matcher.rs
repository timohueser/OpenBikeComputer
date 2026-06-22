//! Forward-biased route matcher — snap a live position onto the loaded route.
//!
//! [`RouteMatch`] keeps a cursor `(chunk, segment, progress)` and, for each fix, searches
//! a **bounded forward window** around it for the nearest route segment — O(window), not
//! O(route). It advances the cursor onto that segment and reports the distance travelled
//! *along* the route (`progress_m`) plus the cross-track distance to it (`dist_m`). Past a
//! distance threshold it flags **off-route** (with hysteresis, so it doesn't flap on GPS
//! jitter) and **freezes** progress — a far fix must not drag the route position — while
//! widening the search so a rejoin is still found. The forward bias is what stops a loop's
//! second pass from snapping back to the first.
//!
//! It is `no_std` and allocation-free: one reused chunk-decode buffer, decoding only the
//! handful of chunks the window spans. The projection ([`project_to_segment`]) is shared
//! with the converter, so matcher and format agree on geometry.

use heapless::Vec;

use crate::geo::{cos_lat, project_to_segment, seg_dist_m_cl};
use crate::reader::{RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};

/// Cross-track distance (m) at/above which the rider is considered off-route…
const OFF_M: f32 = 25.0;
/// …and (below which) back on-route. `ON_M < OFF_M` gives the hysteresis band that keeps
/// the flag from flapping on GPS noise at the boundary.
const ON_M: f32 = 15.0;
/// Segments of backward slack in the on-route search window — absorbs a little GPS jitter
/// without losing the forward bias.
const BACK_SEGS: i64 = 3;
/// Forward search window (segments) while on-route. One fix's travel is far less than this
/// at any cycling speed, so the nearest segment is comfortably inside it.
const FWD_SEGS_ON: i64 = 64;
/// Wider forward window while off-route, so a rejoin further along the route is found
/// without an unbounded full scan.
const FWD_SEGS_OFF: i64 = 320;
/// Tie-break margin (m) for the **first lock only**. The initial scan covers the whole
/// route and runs front-to-back; requiring a candidate to beat the best by this much keeps
/// the *earliest* of several near-equal matches. On an out-and-back (start == end) the
/// finish segment sits right on top of the start, so a few metres of cross-track offset
/// would otherwise latch the cursor onto the finish (progress ≈ 100 %) and the forward bias
/// could never follow the outbound leg. Only the first lock needs this; once tracking, the
/// bounded forward window already prevents latching the far-away finish.
const TIE_EPS_M: f32 = 8.0;

/// The result of matching one fix onto the route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    /// Distance travelled along the route to the matched point (m), clamped to the route
    /// length. **Frozen** while off-route.
    pub progress_m: u32,
    /// Whether the fix is off-route (nearest cross-track distance past the hysteresis
    /// threshold).
    pub off_route: bool,
    /// Cross-track distance from the fix to the nearest route point (m) — always live, so
    /// the UI can show "off route · NNN m".
    pub dist_m: u32,
}

/// A forward-biased cursor that snaps fixes to a route. One per active route; reset on
/// route load/change ([`reset`](RouteMatch::reset)). Owns a reused chunk-decode buffer so
/// matching allocates nothing per fix.
pub struct RouteMatch {
    chunk: usize,
    seg: usize,
    progress_m: u32,
    off_route: bool,
    /// `false` until the first fix has been matched; the first match scans the whole route
    /// to establish an initial lock from anywhere.
    started: bool,
    buf: Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
}

impl Default for RouteMatch {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteMatch {
    pub fn new() -> Self {
        RouteMatch { chunk: 0, seg: 0, progress_m: 0, off_route: false, started: false, buf: Vec::new() }
    }

    /// Forget all match state — call when a route is loaded or swapped.
    pub fn reset(&mut self) {
        self.chunk = 0;
        self.seg = 0;
        self.progress_m = 0;
        self.off_route = false;
        self.started = false;
        self.buf.clear();
    }

    /// Match `(lon, lat)` (microdegrees) onto `route`, advancing the cursor. See the
    /// module docs for the forward-bias / off-route-freeze behaviour.
    pub fn update(&mut self, lon: i32, lat: i32, route: &RouteReader) -> Match {
        let chunks = route.chunks();
        if chunks.is_empty() {
            return Match { progress_m: 0, off_route: true, dist_m: u32::MAX };
        }
        let total = route.total_distance_m;
        let p = (lon, lat);
        let cur_gidx = route.global_seg_index(self.chunk, self.seg) as i64;

        // Window: the first lock and off-route rejoin scan wide; on-route a tight forward
        // window (with a little backward slack) keeps it O(window) and forward-biased.
        let (first_chunk, back, fwd) = if !self.started {
            (0usize, i64::MAX, i64::MAX) // first lock: whole route
        } else if self.off_route {
            (self.chunk.saturating_sub(1), BACK_SEGS, FWD_SEGS_OFF)
        } else {
            (self.chunk.saturating_sub(1), BACK_SEGS, FWD_SEGS_ON)
        };

        // Best so far: (chunk, seg, dist_m, progress_m).
        let mut best: Option<(usize, usize, f32, u32)> = None;
        let mut c = first_chunk;
        let mut base_gidx = route.global_seg_index(first_chunk, 0) as i64;
        'outer: while c < chunks.len() {
            // Whole chunk past the forward window → done (segments only run forward).
            if self.started && base_gidx - cur_gidx > fwd {
                break;
            }
            let pc_segs = (chunks[c].point_count as usize).saturating_sub(1) as i64;
            if route.decode_chunk(c, &mut self.buf).is_ok() && self.buf.len() >= 2 {
                let cum0 = chunks[c].cum_distance_m as f32;
                let mut intra = 0f32; // distance from this chunk's anchor to point s
                                      // cos(lat) barely changes across one chunk's span, so hoist it once per
                                      // chunk (the natural latitude band) rather than recomputing the `cosf` for
                                      // every segment of the forward window.
                let cl = cos_lat(self.buf[0].lat);
                let n = self.buf.len();
                for s in 0..n - 1 {
                    let off = base_gidx + s as i64 - cur_gidx;
                    if self.started && off > fwd {
                        break 'outer;
                    }
                    let a = (self.buf[s].lon, self.buf[s].lat);
                    let b = (self.buf[s + 1].lon, self.buf[s + 1].lat);
                    let seg_len = seg_dist_m_cl(a, b, cl);
                    if !self.started || off >= -back {
                        let (t, dist) = project_to_segment(a, b, p, cl);
                        // First lock biases near-ties to the earliest segment (TIE_EPS_M);
                        // once tracking, the forward window bounds the search so a strict
                        // nearest is right.
                        let better = match best {
                            None => true,
                            Some((_, _, bd, _)) if self.started => dist < bd,
                            Some((_, _, bd, _)) => dist < bd - TIE_EPS_M,
                        };
                        if better {
                            let progress = ((cum0 + intra + t * seg_len) as u32).min(total);
                            best = Some((c, s, dist, progress));
                        }
                    }
                    intra += seg_len;
                }
            }
            base_gidx += pc_segs;
            c += 1;
        }

        let Some((bc, bs, bdist, bprog)) = best else {
            // No segment in range (only with a 1-point route) — report frozen + far.
            return Match { progress_m: self.progress_m, off_route: true, dist_m: u32::MAX };
        };

        // Hysteresis on the nearest cross-track distance.
        let now_off = if bdist >= OFF_M {
            true
        } else if bdist < ON_M {
            false
        } else {
            self.off_route
        };
        self.off_route = now_off;
        self.started = true;

        // Advance only when on-route; off-route freezes progress so a far fix can't drag it.
        if !now_off {
            self.chunk = bc;
            self.seg = bs;
            self.progress_m = bprog;
        }
        Match { progress_m: self.progress_m, off_route: now_off, dist_m: bdist as u32 }
    }
}
