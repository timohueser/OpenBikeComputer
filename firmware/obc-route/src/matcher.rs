//! Forward-biased route matcher — snap a live position onto the loaded route.
//!
//! [`RouteMatch`] keeps a cursor `(chunk, segment, progress)` and, for each fix, searches
//! a **bounded forward window** around it for the nearest route segment — O(window), not
//! O(route). It advances the cursor onto that segment and reports the distance travelled
//! *along* the route (`progress_m`) plus the cross-track distance to it (`dist_m`). Past a
//! distance threshold it flags **off-route** (with hysteresis, so it doesn't flap on GPS
//! jitter) and **freezes** progress — a far fix must not drag the route position — while
//! widening the search so a rejoin is still found. The forward bias stops a loop's second
//! pass from snapping back to the first.
//!
//! It is `no_std` and allocation-free: one reused chunk-decode buffer, decoding only the
//! handful of chunks the window spans. The projection ([`project_to_segment`]) is shared
//! with the converter, so matcher and format agree on geometry.

use heapless::Vec;

use crate::geo::project_to_segment;
use crate::reader::{RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};
use obc_map_scene::{cos_lat, ground_dist_m_cl};

/// Cross-track distance (m) at/above which the rider is considered off-route…
const OFF_M: f32 = 25.0;
/// …and (below which) back on-route. `ON_M < OFF_M` gives the hysteresis band that keeps
/// the flag from flapping on GPS noise at the boundary.
const ON_M: f32 = 15.0;
/// Segments of backward slack in the on-route search window — absorbs a little GPS jitter
/// without losing the forward bias.
const BACK_SEGS: i64 = 3;
/// Forward search window (segments) while on-route. One fix's travel is far less than this
/// at any cycling speed, so the nearest segment is well inside it.
const FWD_SEGS_ON: i64 = 64;
/// Wider forward window while off-route, so a rejoin further along the route is found
/// without an unbounded full scan.
const FWD_SEGS_OFF: i64 = 320;
/// Tie-break margin (m) for the **first lock only**. The initial scan runs front-to-back;
/// requiring a candidate to beat the best by this much keeps the *earliest* of several
/// near-equal matches. On an out-and-back (start == end) a few metres of cross-track offset
/// would otherwise latch the cursor onto the finish (progress ≈ 100 %) and the forward bias
/// could never follow the outbound leg. Once tracking, the bounded forward window prevents this.
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
    /// Durable lower bound installed by a pure skip-ahead commit. Unlike a one-time progress write,
    /// this survives off-route fixes and prevents the bounded backward slack from re-entering the
    /// skipped stretch.
    floor_progress_m: u32,
    /// Global segment containing `floor_progress_m`; segments before it are not candidates.
    floor_global_seg: u32,
    off_route: bool,
    /// `false` until the first fix has been matched; the first match scans the whole route
    /// to establish an initial lock from anywhere.
    started: bool,
    /// Widen the **next** match's forward window to the rejoin window
    /// ([`relock_wide`](RouteMatch::relock_wide)), then clear. Set when the caller knows fixes went
    /// unmatched — the cursor is stale by more than one fix's travel and the tight on-route window
    /// would not reach the rider.
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

    /// Forget all match state — call when a route is loaded or swapped.
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

    /// Install a forward-only navigation floor at `progress_m` and move the matcher cursor to the
    /// containing segment. The route bytes are unchanged: this is the pure-skip semantic used when
    /// the rider plans to leave the line and rejoin later. Returns the exact clamped position, or
    /// `None` when the route has no decodable geometry.
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

    /// Whether the matcher has locked onto the route at least once (its first fix has been
    /// matched) — before that, `progress_m` is a default 0 nothing should derive from.
    pub fn started(&self) -> bool {
        self.started
    }

    /// Ask the **next** match to search the wide (rejoin-sized) forward window once, then fall back
    /// to the tight one.
    ///
    /// For the caller that knows fixes went unmatched while the cursor stood still. The on-route
    /// window is [`FWD_SEGS_ON`] segments ahead — comfortably more than one fix's travel, and
    /// comfortably *less* than a rider's travel through a multi-second gap. The Recalculating
    /// freeze (#1146 P2) is exactly such a gap: it pauses matching for the length of a route search,
    /// and without this the first fix after it would find nothing in range, flag off-route and hold
    /// progress still — a false "off route" chip on a rider who never left the line. One wide
    /// search re-locks instead. Idempotent, and free when no fix was skipped (nobody calls it).
    ///
    /// Note what it is *not* for. A search that comes back with new geometry resets the matcher
    /// outright ([`reset`](RouteMatch::reset) clears this flag with the rest of the lock), and an
    /// unstarted matcher scans the whole route anyway — wider than wide. What this covers is the
    /// freeze that ends with the *same* route still under the rider: a cancel, or a search that
    /// found nothing.
    pub fn relock_wide(&mut self) {
        self.wide_next = true;
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

        // Window: the first lock, an off-route rejoin and a requested re-lock scan wide; on-route a
        // tight forward window (with a little backward slack) keeps it O(window) and forward-biased.
        // The re-lock request is consumed here whichever branch wins, so it costs at most one wide
        // search.
        let wide_relock = core::mem::take(&mut self.wide_next);
        let (first_chunk, back, fwd) = if !self.started {
            (0usize, i64::MAX, i64::MAX) // first lock: whole route
        } else if self.off_route || wide_relock {
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
                    if (!self.started || off >= -back) && global >= self.floor_global_seg {
                        let (mut t, mut dist) = project_to_segment(a, b, p, cl);
                        let mut progress = (cum0 + intra + t * seg_len) as u32;
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
                        // First lock biases near-ties to the earliest segment (TIE_EPS_M);
                        // once tracking, the forward window bounds the search so a strict
                        // nearest is right.
                        let better = match best {
                            None => true,
                            Some((_, _, bd, _)) if self.started => dist < bd,
                            Some((_, _, bd, _)) => dist < bd - TIE_EPS_M,
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
