//! The profile of one chain: the heights that minimise its soft terms inside a tube of node and slope
//! bounds, by a primal active-set method whose every iterate lies inside the tube.

use super::curve::LEAST_STEP_M;

/// A spacing under this counts as this (as `curve::LEAST_STEP_M`).
pub(crate) const MIN_STEP_M: f64 = LEAST_STEP_M;
/// A swept interval thinner than this is a fixed node at its middle.
pub(crate) const THIN_M: f64 = 0.02;
/// A step of the active set ends where no height moves more than this.
const STEP_M: f64 = 1e-9;
/// The string's pull to its start, per metre: it only makes the string unique.
const STRING_FIT: f64 = 1e-8;
/// What a slope bound opens past the string's slope.
const OPEN: f64 = 1e-6;
/// A pivot at or under this share of the largest diagonal of H is not positive.
const PIVOT: f64 = 1e-12;
/// A bound this close to a height holds it; a cut under this is none; two step limits this close tie.
const NEAR: f64 = 1e-12;
/// A multiplier has a wrong sign past this share of 1 + the largest linear term of F.
const MULTIPLIER: f64 = 1e-10;

/// The soft terms of a chain of n nodes: F(z) = Σ fit_i·(z_i − target_i)² + Σ slope_s·(z_{s+1} − z_s)²/l_s
/// + Σ λ_k·κ_k², κ_k = (z_{k+1} − z_k)/l_k − (z_k − z_{k−1})/l_{k−1}, λ_k = bend_k / ((l_{k−1} + l_k)/2).
pub(crate) struct Chain<'a> {
    /// n − 1 lengths in metres, each ≥ MIN_STEP_M.
    pub l: &'a [f64],
    /// n, ≥ 0.
    pub fit: &'a [f64],
    /// n.
    pub target: &'a [f64],
    /// n − 1, ≥ 0.
    pub slope: &'a [f64],
    /// n, smooth_m⁴ at node k; the two ends are not read.
    pub bend: &'a [f64],
}

/// The hard rules in priority order. `Fixed` comes first if present. At most one `Slopes`. A `Bounds` after
/// the `Slopes` has one side only (all lo or all hi).
pub(crate) enum Step<'a> {
    /// n; NaN = free.
    Fixed(&'a [f64]),
    /// n each; −∞ / +∞ = none.
    Bounds { layer: u8, lo: &'a [f64], hi: &'a [f64] },
    /// n − 1 each, rise per metre; −∞ / +∞ = none.
    Slopes { layer: u8, lo: &'a [f64], hi: &'a [f64] },
}

#[derive(Debug)]
pub(crate) struct Cut {
    pub layer: u8,
    pub node: usize,
    pub metres: f64,
}

#[derive(Debug)]
pub(crate) struct Opened {
    pub layer: u8,
    pub segment: usize,
    /// Rise per metre past the bound.
    pub excess: f64,
}

pub(crate) struct Tube {
    /// n; NaN = free.
    pub fixed: Vec<f64>,
    /// n each.
    pub lo: Vec<f64>,
    pub hi: Vec<f64>,
    /// n − 1 each, in metres of rise over the segment (slope × l).
    pub a: Vec<f64>,
    pub b: Vec<f64>,
    pub cuts: Vec<Cut>,
    pub opened: Vec<Opened>,
}

pub(crate) struct Profile {
    pub z: Vec<f64>,
    /// z is the plain solve.
    pub plain: bool,
    pub converged: bool,
    pub steps: usize,
    /// Per node: a bound holds it.
    pub at_lo: Vec<bool>,
    pub at_hi: Vec<bool>,
}

/// The band matrix of a chain is not positive definite.
#[derive(Debug)]
pub(crate) struct Pivot;

/// The minimiser of F with the fixed nodes (NaN = free) only.
pub(crate) fn plain(chain: &Chain, fixed: &[f64]) -> Result<Vec<f64>, Pivot> {
    Quadratic::new(chain).least(&Map::fixed(fixed))
}

/// The reach of a tube (L, U): every height in [L_i, U_i] continues to both sides inside the tube. The tube
/// has a solution if and only if L_i ≤ U_i for every i.
pub(crate) fn sweep(tube: &Tube) -> (Vec<f64>, Vec<f64>) {
    let n = tube.lo.len();
    let (mut lower, mut upper): (Vec<f64>, Vec<f64>) = (0..n)
        .map(|i| if tube.fixed[i].is_nan() { (tube.lo[i], tube.hi[i]) } else { (tube.fixed[i], tube.fixed[i]) })
        .unzip();
    for i in 0..n.saturating_sub(1) {
        upper[i + 1] = upper[i + 1].min(upper[i] + tube.b[i]);
        lower[i + 1] = lower[i + 1].max(lower[i] + tube.a[i]);
    }
    for i in (0..n.saturating_sub(1)).rev() {
        upper[i] = upper[i].min(upper[i + 1] - tube.a[i]);
        lower[i] = lower[i].max(lower[i + 1] - tube.b[i]);
    }
    (lower, upper)
}

/// The tube of the steps, layer by layer: each layer of node bounds is cut to what the tube so far
/// reaches, the slope layer opens where the tight string breaks it, and a thin swept interval is a fixed
/// node. So the tube has a solution after each layer.
pub(crate) fn tube(chain: &Chain, steps: &[Step]) -> Result<Tube, Pivot> {
    let n = chain.fit.len();
    let mut tube = Tube {
        fixed: vec![f64::NAN; n],
        lo: vec![f64::NEG_INFINITY; n],
        hi: vec![f64::INFINITY; n],
        a: vec![f64::NEG_INFINITY; n.saturating_sub(1)],
        b: vec![f64::INFINITY; n.saturating_sub(1)],
        cuts: Vec::new(),
        opened: Vec::new(),
    };
    let mut rest = steps;
    if let Some((Step::Fixed(fixed), more)) = steps.split_first() {
        tube.fixed = fixed.to_vec();
        rest = more;
    }
    let start = plain(chain, &tube.fixed).unwrap_or_else(|_| vec![0.0; n]);
    let mut sloped = false;
    for step in rest {
        match *step {
            Step::Fixed(_) => panic!("a Fixed step comes first"),
            Step::Bounds { layer, lo, hi } => {
                debug_assert!(
                    !sloped || lo.iter().all(|v| *v == f64::NEG_INFINITY) || hi.iter().all(|v| *v == f64::INFINITY)
                );
                bound(&mut tube, layer, lo, hi);
            }
            Step::Slopes { layer, lo, hi } => {
                debug_assert!(!sloped, "at most one Slopes step");
                sloped = true;
                slope(chain, &mut tube, &start, layer, lo, hi)?;
            }
        }
    }
    loop {
        let (lower, upper) = sweep(&tube);
        let mut thin = false;
        for i in 0..n {
            if tube.fixed[i].is_nan() && upper[i] - lower[i] < THIN_M {
                tube.fixed[i] = (lower[i] + upper[i]) / 2.0;
                thin = true;
            }
        }
        if !thin {
            debug_assert!((0..n).all(|i| lower[i] <= upper[i] + 1e-9));
            return Ok(tube);
        }
    }
}

/// A layer of node bounds, each moved into the reach of the tube so far.
fn bound(tube: &mut Tube, layer: u8, lo: &[f64], hi: &[f64]) {
    let (lower, upper) = sweep(tube);
    for i in 0..tube.lo.len() {
        let height = tube.fixed[i];
        if !height.is_nan() {
            for broken in [lo[i] - height, height - hi[i]] {
                if broken > NEAR {
                    tube.cuts.push(Cut { layer, node: i, metres: broken });
                }
            }
            continue;
        }
        let into = |v: f64| if v.is_finite() { v.max(lower[i]).min(upper[i]) } else { v };
        let (mut new_lo, mut new_hi) = (into(lo[i]), into(hi[i]));
        if lo[i].is_finite() && hi[i].is_finite() && new_lo > new_hi {
            let mean = (new_lo + new_hi) / 2.0;
            (new_lo, new_hi) = (mean, mean);
        }
        for (want, got) in [(lo[i], new_lo), (hi[i], new_hi)] {
            if want.is_finite() && (got - want).abs() > NEAR {
                tube.cuts.push(Cut { layer, node: i, metres: (got - want).abs() });
            }
        }
        tube.lo[i] = tube.lo[i].max(new_lo);
        tube.hi[i] = tube.hi[i].min(new_hi);
    }
}

/// The slope layer. Where no curve of the tube holds it, the segments that the tight string breaks open
/// to the string's rise.
fn slope(chain: &Chain, tube: &mut Tube, start: &[f64], layer: u8, lo: &[f64], hi: &[f64]) -> Result<(), Pivot> {
    let n = tube.lo.len();
    let l = |s: usize| chain.l[s].max(MIN_STEP_M);
    for s in 0..n.saturating_sub(1) {
        (tube.a[s], tube.b[s]) = (lo[s] * l(s), hi[s] * l(s));
    }
    let (lower, upper) = sweep(tube);
    if (0..n).all(|i| lower[i] <= upper[i]) {
        return Ok(());
    }
    let metres: Vec<f64> =
        (0..n).map(|i| (if i > 0 { l(i - 1) } else { 0.0 } + if i + 1 < n { l(i) } else { 0.0 }) / 2.0).collect();
    let fit: Vec<f64> = metres.iter().map(|m| STRING_FIT * m).collect();
    let target: Vec<f64> = (0..n).map(|i| start[i].max(tube.lo[i]).min(tube.hi[i])).collect();
    let (slope, bend) = (vec![1.0; n.saturating_sub(1)], vec![0.0; n]);
    let string = Chain { l: chain.l, fit: &fit, target: &target, slope: &slope, bend: &bend };
    let loose = Tube {
        fixed: tube.fixed.clone(),
        lo: tube.lo.clone(),
        hi: tube.hi.clone(),
        a: vec![f64::NEG_INFINITY; n.saturating_sub(1)],
        b: vec![f64::INFINITY; n.saturating_sub(1)],
        cuts: Vec::new(),
        opened: Vec::new(),
    };
    let z = profile(&string, &loose, None)?.z;
    for s in 0..n.saturating_sub(1) {
        let rise = z[s + 1] - z[s];
        if rise > tube.b[s] {
            tube.opened.push(Opened { layer, segment: s, excess: (rise - tube.b[s]) / l(s) });
            tube.b[s] = rise + OPEN * l(s);
        } else if rise < tube.a[s] {
            tube.opened.push(Opened { layer, segment: s, excess: (tube.a[s] - rise) / l(s) });
            tube.a[s] = rise - OPEN * l(s);
        }
    }
    Ok(())
}

/// The least of F in the tube. Every iterate lies in the tube, so the step cap returns a valid profile.
pub(crate) fn profile(chain: &Chain, tube: &Tube, cap: Option<usize>) -> Result<Profile, Pivot> {
    let n = chain.fit.len();
    let h = Quadratic::new(chain);
    let z0 = h.least(&Map::fixed(&tube.fixed))?;
    let mut work = Work { node: vec![Held::No; n], segment: vec![Held::No; n.saturating_sub(1)] };
    if inside(tube, &z0) {
        return Ok(work.profile(z0, true, true, 0));
    }
    let mut x = start(tube, &z0);
    work.take_active(tube, &x);
    let count = (0..4 * n.saturating_sub(1) + 2).filter(|&c| exists(tube, c)).count();
    let cap = cap.unwrap_or((3 * count + 100).min(20_000));
    let tol = MULTIPLIER * (1.0 + h.c.iter().fold(0.0, |m: f64, c| m.max(c.abs())));
    for step in 1..=cap {
        let runs = work.runs(tube);
        let ideal = h.least(&runs.map)?;
        let p: Vec<f64> = (0..n).map(|i| if runs.map.unknown[i].is_some() { ideal[i] - x[i] } else { 0.0 }).collect();
        if p.iter().all(|v| v.abs() <= STEP_M) {
            match work.wrong_sign(&h, &ideal, tube, &runs, tol) {
                None => return Ok(work.profile(ideal, false, true, step)),
                Some(c) => work.release(c, n),
            }
            x = ideal;
            continue;
        }
        let (alpha, block) = blocking(tube, &work, &x, &p);
        for i in 0..n {
            x[i] = if alpha == 1.0 && runs.map.unknown[i].is_some() { ideal[i] } else { x[i] + alpha * p[i] };
        }
        if let Some(c) = block {
            if c < 2 * n {
                x[c / 2] = if c.is_multiple_of(2) { tube.lo[c / 2] } else { tube.hi[c / 2] };
            }
            if runs.independent(c, n) {
                work.hold(c, n);
            }
        }
    }
    Ok(work.profile(x, false, false, cap))
}

/// A curve of the tube near `target`: each node as near as the swept interval and the node before allow.
fn start(tube: &Tube, target: &[f64]) -> Vec<f64> {
    let (lower, upper) = sweep(tube);
    let mut x: Vec<f64> = Vec::with_capacity(target.len());
    for i in 0..target.len() {
        if !tube.fixed[i].is_nan() {
            x.push(tube.fixed[i]);
            continue;
        }
        let (mut from, mut to) = (lower[i], upper[i]);
        if i > 0 {
            from = from.max(x[i - 1] + tube.a[i - 1]);
            to = to.min(x[i - 1] + tube.b[i - 1]);
        }
        // A lower limit passes its upper limit only by rounding.
        debug_assert!(from - to < 1e-9);
        x.push(if from > to { (from + to) / 2.0 } else { target[i].max(from).min(to) });
    }
    x
}

/// Every bound of the tube holds exactly.
fn inside(tube: &Tube, z: &[f64]) -> bool {
    (0..z.len()).all(|i| tube.lo[i] <= z[i] && z[i] <= tube.hi[i])
        && (0..tube.a.len()).all(|s| tube.a[s] <= z[s + 1] - z[s] && z[s + 1] - z[s] <= tube.b[s])
}

/// Constraint numbers: 2i the lo of node i, 2i + 1 its hi; 2n + 2s the a of segment s, 2n + 2s + 1 its b.
/// A constraint with an infinite bound does not exist.
fn exists(tube: &Tube, c: usize) -> bool {
    let n = tube.lo.len();
    match (c < 2 * n, c % 2) {
        (true, 0) => tube.lo[c / 2] > f64::NEG_INFINITY,
        (true, _) => tube.hi[c / 2] < f64::INFINITY,
        (false, 0) => tube.a[(c - 2 * n) / 2] > f64::NEG_INFINITY,
        (false, _) => tube.b[(c - 2 * n) / 2] < f64::INFINITY,
    }
}

/// The step α ≤ 1 along p that keeps x inside every bound out of the working set, and the bound that
/// stops it: the lowest number among the limits within NEAR of the least.
fn blocking(tube: &Tube, work: &Work, x: &[f64], p: &[f64]) -> (f64, Option<usize>) {
    let n = x.len();
    let limit = |c: usize| -> Option<f64> {
        if !exists(tube, c) || work.holds(c, n) {
            return None;
        }
        let (slack, rate) = if c < 2 * n {
            let i = c / 2;
            if c.is_multiple_of(2) {
                (x[i] - tube.lo[i], -p[i])
            } else {
                (tube.hi[i] - x[i], p[i])
            }
        } else {
            let s = (c - 2 * n) / 2;
            let (rise, moves) = (x[s + 1] - x[s], p[s + 1] - p[s]);
            if c.is_multiple_of(2) {
                (rise - tube.a[s], -moves)
            } else {
                (tube.b[s] - rise, moves)
            }
        };
        (rate > 0.0).then(|| slack.max(0.0) / rate)
    };
    let total = 4 * n.saturating_sub(1) + 2;
    let least = (0..total).filter_map(limit).fold(f64::INFINITY, f64::min);
    if least >= 1.0 {
        return (1.0, None);
    }
    (least, (0..total).find(|&c| limit(c).is_some_and(|t| t <= least + NEAR)))
}

/// F as ½·zᵀHz − cᵀz: the diagonal of H, its two bands, and c.
struct Quadratic {
    d: Vec<f64>,
    e1: Vec<f64>,
    e2: Vec<f64>,
    c: Vec<f64>,
}

impl Quadratic {
    fn new(chain: &Chain) -> Self {
        let n = chain.fit.len();
        let l = |s: usize| chain.l[s].max(MIN_STEP_M);
        let mut q = Quadratic {
            d: vec![0.0; n],
            e1: vec![0.0; n.saturating_sub(1)],
            e2: vec![0.0; n.saturating_sub(2)],
            c: vec![0.0; n],
        };
        for i in 0..n {
            q.d[i] += 2.0 * chain.fit[i];
            q.c[i] = 2.0 * chain.fit[i] * chain.target[i];
        }
        for s in 0..n.saturating_sub(1) {
            let w = 2.0 * chain.slope[s] / l(s);
            q.d[s] += w;
            q.d[s + 1] += w;
            q.e1[s] -= w;
        }
        for k in 1..n.saturating_sub(1) {
            let (l0, l1) = (l(k - 1), l(k));
            let w = 2.0 * chain.bend[k] / ((l0 + l1) / 2.0);
            let c = [1.0 / l0, -(1.0 / l0 + 1.0 / l1), 1.0 / l1];
            q.d[k - 1] += w * c[0] * c[0];
            q.d[k] += w * c[1] * c[1];
            q.d[k + 1] += w * c[2] * c[2];
            q.e1[k - 1] += w * c[0] * c[1];
            q.e1[k] += w * c[1] * c[2];
            q.e2[k - 1] += w * c[0] * c[2];
        }
        q
    }

    fn times(&self, z: &[f64]) -> Vec<f64> {
        let n = z.len();
        (0..n)
            .map(|i| {
                let mut v = self.d[i] * z[i];
                if i + 1 < n {
                    v += self.e1[i] * z[i + 1];
                }
                if i >= 1 {
                    v += self.e1[i - 1] * z[i - 1];
                }
                if i + 2 < n {
                    v += self.e2[i] * z[i + 2];
                }
                if i >= 2 {
                    v += self.e2[i - 2] * z[i - 2];
                }
                v
            })
            .collect()
    }

    /// The least of F over z = P·y + q: the reduced matrix PᵀHP keeps two bands, because each unknown is
    /// a run of nodes in chain order.
    fn least(&self, map: &Map) -> Result<Vec<f64>, Pivot> {
        let n = self.d.len();
        let m = map.count;
        let hq = self.times(&map.q);
        let (mut d, mut e1, mut e2, mut r) = (vec![0.0; m], vec![0.0; m], vec![0.0; m], vec![0.0; m]);
        for (i, &unknown) in map.unknown.iter().enumerate() {
            let Some(u) = unknown else { continue };
            r[u] += self.c[i] - hq[i];
            d[u] += self.d[i];
            for (k, v) in [(i + 1, self.e1.get(i)), (i + 2, self.e2.get(i))] {
                if let (Some(&v), Some(Some(w))) = (v, map.unknown.get(k)) {
                    match w - u {
                        0 => d[u] += 2.0 * v,
                        1 => e1[u] += v,
                        _ => e2[u] += v,
                    }
                }
            }
        }
        let floor = PIVOT * self.d.iter().fold(0.0, |m: f64, &v| m.max(v));
        let y = ldl(d, e1, e2, r, floor)?;
        Ok((0..n).map(|i| map.unknown[i].map_or(map.q[i], |u| y[u] + map.q[i])).collect())
    }
}

/// Solves the symmetric matrix of a diagonal and two bands by LDLᵀ without pivoting, in place.
fn ldl(mut d: Vec<f64>, mut e1: Vec<f64>, mut e2: Vec<f64>, mut r: Vec<f64>, floor: f64) -> Result<Vec<f64>, Pivot> {
    let m = d.len();
    // e1[i] becomes L[i + 1][i], e2[i] becomes L[i + 2][i], d[i] becomes D[i].
    for i in 0..m {
        if i >= 1 {
            d[i] -= e1[i - 1] * e1[i - 1] * d[i - 1];
        }
        if i >= 2 {
            d[i] -= e2[i - 2] * e2[i - 2] * d[i - 2];
        }
        if d[i].is_nan() || d[i] <= floor {
            return Err(Pivot);
        }
        if i + 1 < m {
            if i >= 1 {
                e1[i] -= e2[i - 1] * e1[i - 1] * d[i - 1];
            }
            e1[i] /= d[i];
        }
        if i + 2 < m {
            e2[i] /= d[i];
        }
    }
    for i in 0..m {
        if i >= 1 {
            r[i] -= e1[i - 1] * r[i - 1];
        }
        if i >= 2 {
            r[i] -= e2[i - 2] * r[i - 2];
        }
    }
    for i in 0..m {
        r[i] /= d[i];
    }
    for i in (0..m).rev() {
        if i + 1 < m {
            r[i] -= e1[i] * r[i + 1];
        }
        if i + 2 < m {
            r[i] -= e2[i] * r[i + 2];
        }
    }
    Ok(r)
}

/// The heights z = P·y + q: per node the unknown that moves it (none for a known height), and q.
struct Map {
    unknown: Vec<Option<usize>>,
    q: Vec<f64>,
    count: usize,
}

impl Map {
    fn fixed(fixed: &[f64]) -> Self {
        let mut count = 0;
        let unknown = fixed
            .iter()
            .map(|v| {
                v.is_nan().then(|| {
                    count += 1;
                    count - 1
                })
            })
            .collect();
        Map { unknown, q: fixed.iter().map(|v| if v.is_nan() { 0.0 } else { *v }).collect(), count }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Held {
    No,
    Lo,
    Hi,
}

/// The working set: per node the bound that holds it, per segment the slope bound (a = Lo, b = Hi) that
/// ties its two nodes.
struct Work {
    node: Vec<Held>,
    segment: Vec<Held>,
}

/// The runs of the working set: nodes tied by held slopes move as one unknown; a run with a fixed or a
/// held node is known.
struct Runs {
    map: Map,
    first: Vec<usize>,
    known: Vec<Option<usize>>,
}

impl Runs {
    /// A node bound is independent of the working set if its run is not known; a slope bound unless the
    /// runs of its two nodes are both known.
    fn independent(&self, c: usize, n: usize) -> bool {
        if c < 2 * n {
            return self.known[c / 2].is_none();
        }
        let s = (c - 2 * n) / 2;
        self.first[s] != self.first[s + 1] && (self.known[s].is_none() || self.known[s + 1].is_none())
    }
}

impl Work {
    fn holds(&self, c: usize, n: usize) -> bool {
        let side = if c.is_multiple_of(2) { Held::Lo } else { Held::Hi };
        if c < 2 * n {
            self.node[c / 2] == side
        } else {
            self.segment[(c - 2 * n) / 2] == side
        }
    }

    fn hold(&mut self, c: usize, n: usize) {
        let side = if c.is_multiple_of(2) { Held::Lo } else { Held::Hi };
        if c < 2 * n {
            self.node[c / 2] = side;
        } else {
            self.segment[(c - 2 * n) / 2] = side;
        }
    }

    fn release(&mut self, c: usize, n: usize) {
        if c < 2 * n {
            self.node[c / 2] = Held::No;
        } else {
            self.segment[(c - 2 * n) / 2] = Held::No;
        }
    }

    /// The bounds active at x, in number order, each if it is independent of those taken. All node bounds
    /// come before the first slope bound, so a node's run is its node until the slopes join them.
    fn take_active(&mut self, tube: &Tube, x: &[f64]) {
        let n = x.len();
        for (i, &x) in x.iter().enumerate() {
            if !tube.fixed[i].is_nan() {
                continue;
            }
            if x - tube.lo[i] <= NEAR {
                self.node[i] = Held::Lo;
            } else if tube.hi[i] - x <= NEAR {
                self.node[i] = Held::Hi;
            }
        }
        let known = |i: usize| !tube.fixed[i].is_nan() || self.node[i] != Held::No;
        let mut left = n > 0 && known(0);
        for s in 0..n.saturating_sub(1) {
            let right = known(s + 1);
            let rise = x[s + 1] - x[s];
            let side = if rise - tube.a[s] <= NEAR {
                Held::Lo
            } else if tube.b[s] - rise <= NEAR {
                Held::Hi
            } else {
                Held::No
            };
            if side != Held::No && !(left && right) {
                self.segment[s] = side;
                left = left || right;
            } else {
                left = right;
            }
        }
    }

    fn runs(&self, tube: &Tube) -> Runs {
        let n = tube.lo.len();
        let mut map = Map { unknown: vec![None; n], q: vec![0.0; n], count: 0 };
        let (mut first, mut known) = (vec![0; n], vec![None; n]);
        let mut j = 0;
        while j < n {
            // q holds the rise from the first node of the run.
            let mut k = j;
            while k + 1 < n && self.segment[k] != Held::No {
                map.q[k + 1] = map.q[k] + if self.segment[k] == Held::Lo { tube.a[k] } else { tube.b[k] };
                k += 1;
            }
            let held = (j..=k).find(|&i| !tube.fixed[i].is_nan() || self.node[i] != Held::No);
            for i in j..=k {
                (first[i], known[i]) = (j, held);
            }
            match held {
                Some(h) => {
                    let height = match self.node[h] {
                        _ if !tube.fixed[h].is_nan() => tube.fixed[h],
                        Held::Lo => tube.lo[h],
                        _ => tube.hi[h],
                    };
                    let from = map.q[h];
                    for i in j..=k {
                        map.q[i] = height + (map.q[i] - from);
                    }
                }
                None => {
                    for i in j..=k {
                        map.unknown[i] = Some(map.count);
                    }
                    map.count += 1;
                }
            }
            j = k + 1;
        }
        Runs { map, first, known }
    }

    /// The lowest numbered constraint of the working set whose multiplier at `z` has a wrong sign past
    /// `tol`. The multipliers are running sums of the gradient along each run: from its first node, and
    /// from its last node past its known node.
    fn wrong_sign(&self, h: &Quadratic, z: &[f64], tube: &Tube, runs: &Runs, tol: f64) -> Option<usize> {
        let n = z.len();
        let hz = h.times(z);
        let g: Vec<f64> = (0..n).map(|i| hz[i] - h.c[i]).collect();
        let mut wrong: Option<usize> = None;
        let mut check = |c: usize, held: Held, multiplier: f64| {
            let bad = match held {
                Held::Lo => multiplier < -tol,
                Held::Hi => multiplier > tol,
                Held::No => false,
            };
            if bad {
                wrong = Some(wrong.map_or(c, |w| w.min(c)));
            }
        };
        let mut j = 0;
        while j < n {
            let mut k = j;
            while k + 1 < n && runs.first[k + 1] == j {
                k += 1;
            }
            let known = runs.known[j];
            let mut sum = 0.0;
            for (s, g) in g.iter().enumerate().take(k).skip(j) {
                sum += g;
                if known.is_none_or(|q| s < q) {
                    check(2 * n + 2 * s + usize::from(self.segment[s] == Held::Hi), self.segment[s], -sum);
                }
            }
            if let Some(q) = known {
                let mut sum = 0.0;
                for s in (q..k).rev() {
                    sum += g[s + 1];
                    check(2 * n + 2 * s + usize::from(self.segment[s] == Held::Hi), self.segment[s], sum);
                }
                if tube.fixed[q].is_nan() {
                    let beta = (j..=k).fold(0.0, |sum, i| sum + g[i]);
                    check(2 * q + usize::from(self.node[q] == Held::Hi), self.node[q], beta);
                }
            }
            j = k + 1;
        }
        wrong
    }

    fn profile(&self, z: Vec<f64>, plain: bool, converged: bool, steps: usize) -> Profile {
        Profile {
            z,
            plain,
            converged,
            steps,
            at_lo: self.node.iter().map(|h| *h == Held::Lo).collect(),
            at_hi: self.node.iter().map(|h| *h == Held::Hi).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INF: f64 = f64::INFINITY;

    enum Rule {
        Fixed(Vec<f64>),
        Bounds(u8, Vec<f64>, Vec<f64>),
        Slopes(u8, Vec<f64>, Vec<f64>),
    }

    struct Case {
        l: Vec<f64>,
        fit: Vec<f64>,
        target: Vec<f64>,
        slope: Vec<f64>,
        bend: Vec<f64>,
        rules: Vec<Rule>,
    }

    impl Case {
        fn chain(&self) -> Chain<'_> {
            Chain { l: &self.l, fit: &self.fit, target: &self.target, slope: &self.slope, bend: &self.bend }
        }

        fn steps(&self) -> Vec<Step<'_>> {
            self.rules
                .iter()
                .map(|r| match r {
                    Rule::Fixed(v) => Step::Fixed(v),
                    Rule::Bounds(layer, lo, hi) => Step::Bounds { layer: *layer, lo, hi },
                    Rule::Slopes(layer, lo, hi) => Step::Slopes { layer: *layer, lo, hi },
                })
                .collect()
        }

        fn solve(&self, cap: Option<usize>) -> (Tube, Profile) {
            let tube = tube(&self.chain(), &self.steps()).unwrap();
            let profile = profile(&self.chain(), &tube, cap).unwrap();
            (tube, profile)
        }
    }

    /// n nodes l metres apart.
    fn even(n: usize, l: f64) -> Vec<f64> {
        vec![l; n - 1]
    }

    /// The fit of a ground node: its metres.
    fn ground(l: &[f64]) -> Vec<f64> {
        let n = l.len() + 1;
        (0..n).map(|i| (if i > 0 { l[i - 1] } else { 0.0 } + if i + 1 < n { l[i] } else { 0.0 }) / 2.0).collect()
    }

    fn bend(n: usize, m: f64) -> Vec<f64> {
        vec![m * m * m * m; n]
    }

    fn along(l: &[f64]) -> Vec<f64> {
        std::iter::once(0.0)
            .chain(l.iter().scan(0.0, |s, l| {
                *s += l;
                Some(*s)
            }))
            .collect()
    }

    /// Fixed heights at some nodes of n.
    fn at(n: usize, nodes: &[(usize, f64)]) -> Vec<f64> {
        let mut v = vec![f64::NAN; n];
        nodes.iter().for_each(|&(i, h)| v[i] = h);
        v
    }

    /// Every node bound and slope bound holds to 1e-9, and every fixed node.
    fn inside(tube: &Tube, z: &[f64]) -> bool {
        let e = 1e-9;
        (0..z.len()).all(|i| tube.fixed[i].is_nan() || (z[i] - tube.fixed[i]).abs() <= e)
            && (0..z.len()).all(|i| tube.lo[i] - e <= z[i] && z[i] <= tube.hi[i] + e)
            && (0..tube.a.len()).all(|s| tube.a[s] - e <= z[s + 1] - z[s] && z[s + 1] - z[s] <= tube.b[s] + e)
    }

    /// F of the chain at z, from its three sums.
    fn value(chain: &Chain, z: &[f64]) -> f64 {
        let n = z.len();
        let l = |s: usize| chain.l[s].max(MIN_STEP_M);
        let mut f = 0.0;
        for (i, z) in z.iter().enumerate() {
            f += chain.fit[i] * (z - chain.target[i]) * (z - chain.target[i]);
        }
        for s in 0..n - 1 {
            f += chain.slope[s] * (z[s + 1] - z[s]) * (z[s + 1] - z[s]) / l(s);
        }
        for k in 1..n - 1 {
            let kappa = (z[k + 1] - z[k]) / l(k) - (z[k] - z[k - 1]) / l(k - 1);
            f += chain.bend[k] / ((l(k - 1) + l(k)) / 2.0) * kappa * kappa;
        }
        f
    }

    /// A 64-bit LCG; a uniform number in [0, 1) from its top 53 bits.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// A chain of n nodes of the generator; with `fixed`, one fixed node inside its bound.
    fn generated(rng: &mut Lcg, n: usize, fixed: bool) -> Case {
        let l: Vec<f64> = (0..n - 1).map(|_| 1.0 + 2.0 * rng.next()).collect();
        let mut target = vec![0.0; n];
        for i in 1..n {
            target[i] = target[i - 1] + 4.0 * rng.next() - 2.0;
        }
        let width: Vec<f64> = (0..n).map(|_| 0.5 + 2.5 * rng.next()).collect();
        let lo: Vec<f64> = (0..n).map(|i| target[i] - width[i]).collect();
        let hi: Vec<f64> = (0..n).map(|i| target[i] + width[i]).collect();
        let mut rules = Vec::new();
        if fixed {
            let i = ((rng.next() * n as f64) as usize).min(n - 1);
            rules.push(Rule::Fixed(at(n, &[(i, lo[i] + rng.next() * (hi[i] - lo[i]))])));
        }
        rules.push(Rule::Bounds(2, lo, hi));
        rules.push(Rule::Slopes(4, vec![-0.3; n - 1], vec![0.3; n - 1]));
        Case { fit: ground(&l), target, slope: vec![0.0; n - 1], bend: bend(n, 8.0), l, rules }
    }

    /// Case 1: eleven ground nodes 2 m apart.
    fn level_or_sloped(sloped: bool) -> Case {
        let l = even(11, 2.0);
        let target = along(&l).iter().map(|s| if sloped { 0.05 * s } else { 100.0 }).collect();
        Case { fit: ground(&l), target, slope: vec![0.0; 10], bend: bend(11, 15.0), l, rules: Vec::new() }
    }

    /// Cases 3 and 4: a step of the terrain to `high` in the middle of 20 ground nodes.
    fn step_of_terrain(high: f64) -> Case {
        let (n, l) = (20, even(20, 2.0));
        let target: Vec<f64> = (0..n).map(|i| if i <= 9 { 0.0 } else { high }).collect();
        let rules = vec![
            Rule::Bounds(2, target.iter().map(|t| t - 5.0).collect(), target.iter().map(|t| t + 5.0).collect()),
            Rule::Slopes(4, vec![-0.5; n - 1], vec![0.5; n - 1]),
        ];
        Case { fit: ground(&l), target, slope: vec![0.0; n - 1], bend: bend(n, 3.0), l, rules }
    }

    /// Case 9: two fixed heights 3 m apart over one segment of a road of 12 %.
    fn contradicting_reads() -> Case {
        let (n, l) = (21, even(21, 2.0));
        let rules = vec![
            Rule::Fixed(at(n, &[(10, 0.0), (11, 3.0)])),
            Rule::Bounds(2, vec![-5.0; n], vec![5.0; n]),
            Rule::Slopes(4, vec![-0.12; n - 1], vec![0.12; n - 1]),
        ];
        Case { fit: ground(&l), target: vec![0.0; n], slope: vec![0.0; n - 1], bend: bend(n, 20.0), l, rules }
    }

    /// A structure of n nodes 2 m apart between fixed ends at 0, with the 1:1 bound.
    fn structure(n: usize, slope: f64, mut rules: Vec<Rule>) -> Case {
        let l = even(n, 2.0);
        rules.insert(0, Rule::Slopes(1, vec![-1.0; n - 1], vec![1.0; n - 1]));
        rules.insert(0, Rule::Fixed(at(n, &[(0, 0.0), (n - 1, 0.0)])));
        Case { fit: vec![0.0; n], target: vec![0.0; n], slope: vec![slope; n - 1], bend: bend(n, 15.0), l, rules }
    }

    /// The bound `v` at some nodes of n, none elsewhere.
    fn only(n: usize, nodes: &[usize], v: f64, none: f64) -> Vec<f64> {
        (0..n).map(|i| if nodes.contains(&i) { v } else { none }).collect()
    }

    fn rises(z: &[f64]) -> Vec<f64> {
        z.windows(2).map(|w| w[1] - w[0]).collect()
    }

    #[test]
    fn the_plain_solve_keeps_a_level_and_a_sloped_line() {
        for sloped in [false, true] {
            let case = level_or_sloped(sloped);
            let z = plain(&case.chain(), &[f64::NAN; 11]).unwrap();
            assert!(z.iter().zip(&case.target).all(|(z, t)| (z - t).abs() < 1e-9), "{z:?}");
        }
    }

    #[test]
    fn a_profile_inside_its_tube_is_the_plain_solve_bit_for_bit() {
        let mut case = level_or_sloped(true);
        case.rules = vec![
            Rule::Bounds(
                2,
                case.target.iter().map(|t| t - 5.0).collect(),
                case.target.iter().map(|t| t + 5.0).collect(),
            ),
            Rule::Slopes(4, vec![-0.12; 10], vec![0.12; 10]),
        ];
        let (tube, profile) = case.solve(None);
        assert!(profile.plain);
        assert_eq!(profile.steps, 0);
        assert!(profile.z == plain(&case.chain(), &tube.fixed).unwrap());
    }

    #[test]
    fn a_grade_holds_across_a_step_of_the_terrain_inside_the_band() {
        let (tube, profile) = step_of_terrain(10.0).solve(None);
        let z = &profile.z;
        assert!(tube.opened.is_empty(), "{:?}", tube.opened);
        assert!((0..20).all(|i| (z[i] + z[19 - i] - 10.0).abs() < 1e-6), "{z:?}");
        assert!((z[10] - z[9] - 1.0).abs() < 1e-9, "{z:?}");
        assert!((z[9] - 4.5).abs() < 1e-6, "{z:?}");
        assert!(inside(&tube, z));
        assert!(profile.converged);
    }

    #[test]
    fn the_string_opens_only_the_segment_that_the_band_forces() {
        let (tube, profile) = step_of_terrain(20.0).solve(None);
        assert_eq!(tube.opened.len(), 1, "{:?}", tube.opened);
        assert!(tube.opened[0].segment == 9 && tube.opened[0].layer == 4, "{:?}", tube.opened);
        assert!((tube.opened[0].excess - 4.5).abs() < 1e-6, "{:?}", tube.opened);
        let rise = rises(&profile.z);
        assert!(rise[9] >= 10.0 - 1e-9 && rise[9] <= 10.0 + 1e-5, "{rise:?}");
        assert!((0..19).filter(|&s| s != 9).all(|s| rise[s].abs() <= 1.0 + 1e-9), "{rise:?}");
        assert!(inside(&tube, &profile.z));
    }

    #[test]
    fn a_shaft_opens_the_structure_slope_between_its_fixed_ends() {
        let l = even(16, 2.0);
        let case = Case {
            fit: vec![0.0; 16],
            target: vec![0.0; 16],
            slope: vec![1.0; 15],
            bend: bend(16, 15.0),
            l,
            rules: vec![Rule::Fixed(at(16, &[(0, 0.0), (15, -60.0)])), Rule::Slopes(1, vec![-1.0; 15], vec![1.0; 15])],
        };
        let (tube, profile) = case.solve(None);
        assert_eq!(tube.opened.len(), 15, "{:?}", tube.opened);
        assert!(tube.opened.iter().all(|o| (o.excess - 1.0).abs() < 1e-9), "{:?}", tube.opened);
        // The openings leave a tube OPEN × 30 m wide, so the thin rule fixes every node at its middle.
        assert!(tube.fixed.iter().all(|v| !v.is_nan()), "{:?}", tube.fixed);
        assert!(profile.converged && profile.z == tube.fixed && inside(&tube, &profile.z));
        assert!((0..16).all(|i| (profile.z[i] + 4.0 * i as f64).abs() < OPEN * 30.0), "{:?}", profile.z);
    }

    #[test]
    fn a_deck_bound_by_a_landing_is_cut_to_the_one_to_one_line() {
        let case = structure(21, 1e-3, vec![Rule::Bounds(2, only(21, &[1, 2], 6.0, -INF), vec![INF; 21])]);
        let (tube, profile) = case.solve(None);
        let cuts: Vec<(u8, usize, f64)> = tube.cuts.iter().map(|c| (c.layer, c.node, c.metres)).collect();
        assert_eq!(cuts.len(), 2, "{cuts:?}");
        assert!(cuts[0].0 == 2 && cuts[0].1 == 1 && (cuts[0].2 - 4.0).abs() < 1e-9, "{cuts:?}");
        assert!(cuts[1].0 == 2 && cuts[1].1 == 2 && (cuts[1].2 - 2.0).abs() < 1e-9, "{cuts:?}");
        assert!((tube.fixed[1] - 2.0).abs() < 1e-12 && (tube.fixed[2] - 4.0).abs() < 1e-12, "{:?}", tube.fixed);
        assert!(inside(&tube, &profile.z));
        assert!(rises(&profile.z).iter().all(|r| r.abs() <= 2.0 + 1e-9), "{:?}", profile.z);
    }

    #[test]
    fn a_tunnel_bound_gives_way_to_a_deck_bound() {
        let case = structure(
            21,
            1.0,
            vec![
                Rule::Bounds(2, only(21, &[10], 3.0, -INF), vec![INF; 21]),
                Rule::Bounds(3, vec![-INF; 21], only(21, &[11], 0.0, INF)),
            ],
        );
        let (tube, profile) = case.solve(None);
        assert_eq!(tube.cuts.len(), 1, "{:?}", tube.cuts);
        let cut = &tube.cuts[0];
        assert!(cut.layer == 3 && cut.node == 11 && (cut.metres - 1.0).abs() < 1e-9, "{cut:?}");
        assert!((tube.fixed[10] - 3.0).abs() < 1e-12 && (tube.fixed[11] - 1.0).abs() < 1e-12, "{:?}", tube.fixed);
        assert!(inside(&tube, &profile.z));
    }

    #[test]
    fn a_thin_interval_is_a_fixed_node() {
        let l = even(5, 2.0);
        let case = Case {
            fit: ground(&l),
            target: vec![0.0; 5],
            slope: vec![0.0; 4],
            bend: bend(5, 15.0),
            l,
            rules: vec![Rule::Bounds(2, only(5, &[2], 1.00, -INF), only(5, &[2], 1.01, INF))],
        };
        let (tube, profile) = case.solve(None);
        assert!((tube.fixed[2] - 1.005).abs() < 1e-12, "{:?}", tube.fixed);
        assert!((profile.z[2] - 1.005).abs() < 1e-12, "{:?}", profile.z);
    }

    #[test]
    fn two_fixed_heights_that_contradict_the_grade_open_one_segment() {
        let (tube, profile) = contradicting_reads().solve(None);
        assert_eq!(tube.opened.len(), 1, "{:?}", tube.opened);
        assert!(tube.opened[0].segment == 10 && (tube.opened[0].excess - 1.38).abs() < 1e-6, "{:?}", tube.opened);
        let rise = rises(&profile.z);
        assert!((0..20).filter(|&s| s != 10).all(|s| rise[s].abs() <= 0.24 + 1e-9), "{rise:?}");
        assert!(profile.z[10] == 0.0 && profile.z[11] == 3.0, "{:?}", profile.z);
        assert!(inside(&tube, &profile.z));
    }

    #[test]
    fn at_the_step_cap_the_profile_is_in_the_tube() {
        let (tube, profile) = step_of_terrain(10.0).solve(Some(1));
        assert!(!profile.converged);
        assert!(inside(&tube, &profile.z));
    }

    /// A constraint as an equation: its gradient and its right side.
    type Row = (Vec<f64>, f64);

    /// The least of F in the tube over every choice of the bounds that hold: the equality problem of
    /// each choice by a dense solve of heights and multipliers, kept where it lies in the tube and its
    /// multipliers have the right signs.
    fn brute(chain: &Chain, tube: &Tube) -> Vec<Vec<f64>> {
        let n = chain.fit.len();
        // H and c of F = ½zᵀHz − cᵀz + F(0), from F itself.
        let unit = |pairs: &[(usize, f64)]| {
            let mut z = vec![0.0; n];
            pairs.iter().for_each(|&(i, v)| z[i] += v);
            value(chain, &z)
        };
        let f0 = unit(&[]);
        let c: Vec<f64> = (0..n).map(|i| (unit(&[(i, -1.0)]) - unit(&[(i, 1.0)])) / 2.0).collect();
        let h: Vec<Vec<f64>> = (0..n)
            .map(|i| {
                (0..n)
                    .map(|j| match i == j {
                        true => unit(&[(i, 1.0)]) + unit(&[(i, -1.0)]) - 2.0 * f0,
                        false => unit(&[(i, 1.0), (j, 1.0)]) - unit(&[(i, 1.0)]) - unit(&[(j, 1.0)]) + f0,
                    })
                    .collect()
            })
            .collect();
        // Per free node {free, lo, hi} and per segment {free, a, b}, where the bound exists.
        let mut items: Vec<Vec<Option<Row>>> = Vec::new();
        let e = |pairs: &[(usize, f64)]| {
            let mut g = vec![0.0; n];
            pairs.iter().for_each(|&(i, v)| g[i] += v);
            g
        };
        let mut fixed_rows = Vec::new();
        for i in 0..n {
            if !tube.fixed[i].is_nan() {
                fixed_rows.push((e(&[(i, 1.0)]), tube.fixed[i]));
                continue;
            }
            let mut choices = vec![None];
            if tube.lo[i].is_finite() {
                choices.push(Some((e(&[(i, 1.0)]), tube.lo[i])));
            }
            if tube.hi[i].is_finite() {
                choices.push(Some((e(&[(i, -1.0)]), -tube.hi[i])));
            }
            items.push(choices);
        }
        for s in 0..n - 1 {
            let mut choices = vec![None];
            if tube.a[s].is_finite() {
                choices.push(Some((e(&[(s + 1, 1.0), (s, -1.0)]), tube.a[s])));
            }
            if tube.b[s].is_finite() {
                choices.push(Some((e(&[(s + 1, -1.0), (s, 1.0)]), -tube.b[s])));
            }
            items.push(choices);
        }
        let total: usize = items.iter().map(|c| c.len()).product();
        let mut found: Vec<Vec<f64>> = Vec::new();
        for mut code in 0..total {
            let mut rows: Vec<Row> = Vec::new();
            for choices in &items {
                if let Some(row) = &choices[code % choices.len()] {
                    rows.push(row.clone());
                }
                code /= choices.len();
            }
            let signed = rows.len();
            rows.extend(fixed_rows.iter().cloned());
            // [H −Gᵀ; G 0]·[z; μ] = [c; r]: ∇F = Σ μ_j·g_j, μ_j ≥ 0 for a bound.
            let k = rows.len();
            let size = n + k;
            let mut a = vec![vec![0.0; size + 1]; size];
            for i in 0..n {
                a[i][..n].copy_from_slice(&h[i]);
                for (j, (g, _)) in rows.iter().enumerate() {
                    a[i][n + j] = -g[i];
                }
                a[i][size] = c[i];
            }
            for (j, (g, r)) in rows.iter().enumerate() {
                a[n + j][..n].copy_from_slice(g);
                a[n + j][size] = *r;
            }
            let Some(x) = dense(a) else { continue };
            let (z, mu) = (&x[..n], &x[n..]);
            if inside(tube, z)
                && mu[..signed].iter().all(|m| *m >= -1e-9)
                && !found.iter().any(|f| f.iter().zip(z).all(|(a, b)| (a - b).abs() < 1e-7))
            {
                found.push(z.to_vec());
            }
        }
        found
    }

    /// Gaussian elimination with partial pivoting of an augmented matrix; none if it is singular.
    fn dense(mut a: Vec<Vec<f64>>) -> Option<Vec<f64>> {
        let size = a.len();
        let scale = a.iter().flat_map(|r| r[..size].iter()).fold(0.0, |m: f64, v| m.max(v.abs()));
        for col in 0..size {
            let pivot = (col..size).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
            if a[pivot][col].abs() <= 1e-10 * scale {
                return None;
            }
            a.swap(col, pivot);
            let (above, below) = a.split_at_mut(col + 1);
            for row in below {
                let f = row[col] / above[col][col];
                for (v, p) in row[col..].iter_mut().zip(&above[col][col..]) {
                    *v -= f * p;
                }
            }
        }
        let mut x = vec![0.0; size];
        for row in (0..size).rev() {
            let sum: f64 = (row + 1..size).map(|k| a[row][k] * x[k]).sum();
            x[row] = (a[row][size] - sum) / a[row][row];
        }
        Some(x)
    }

    #[test]
    fn the_profile_is_the_least_of_f_in_the_tube() {
        let mut rng = Lcg(11);
        for number in 0..40 {
            let case = generated(&mut rng, 5, true);
            let (tube, profile) = case.solve(None);
            let found = brute(&case.chain(), &tube);
            assert_eq!(found.len(), 1, "chain {number}: {found:?}");
            assert!(profile.converged, "chain {number}");
            assert!(
                profile.z.iter().zip(&found[0]).all(|(a, b)| (a - b).abs() < 1e-8),
                "chain {number}: {:?} {:?}",
                profile.z,
                found[0]
            );
        }
    }

    #[test]
    fn no_curve_of_the_tube_near_the_profile_has_a_smaller_f() {
        let mut rng = Lcg(12);
        let mut cases = vec![step_of_terrain(10.0), step_of_terrain(20.0), contradicting_reads()];
        cases.extend((0..20).map(|_| generated(&mut rng, 200, false)));
        for (number, case) in cases.iter().enumerate() {
            let (tube, profile) = case.solve(None);
            assert!(profile.converged && inside(&tube, &profile.z), "case {number}");
            let least = value(&case.chain(), &profile.z);
            for _ in 0..200 {
                let near: Vec<f64> = profile.z.iter().map(|z| z + rng.next() - 0.5).collect();
                let y = start(&tube, &near);
                assert!(inside(&tube, &y), "case {number}");
                assert!(value(&case.chain(), &y) >= least - 1e-9, "case {number}");
            }
        }
    }

    #[test]
    fn the_same_input_gives_the_same_bits() {
        let (_, first) = step_of_terrain(20.0).solve(None);
        let (_, second) = step_of_terrain(20.0).solve(None);
        assert!(first.z == second.z);
    }

    /// The long climb at 12.4 % with noise against a grade of 12 % (run by name, release build).
    #[test]
    #[ignore]
    fn a_block_of_25000_nodes_on_a_long_climb() {
        let n = 25_000;
        let l = even(n, 2.0);
        let mut rng = Lcg(1);
        let target: Vec<f64> =
            along(&l).iter().map(|s| 0.124 * s + 2.0 * (s / 7.0).sin() + 2.0 * rng.next() - 1.0).collect();
        let rules = vec![
            Rule::Bounds(2, target.iter().map(|t| t - 5.0).collect(), target.iter().map(|t| t + 5.0).collect()),
            Rule::Slopes(4, vec![-0.12; n - 1], vec![0.12; n - 1]),
        ];
        let case = Case { fit: ground(&l), target, slope: vec![0.0; n - 1], bend: bend(n, 30.0), l, rules };
        let clock = std::time::Instant::now();
        let tube = tube(&case.chain(), &case.steps()).unwrap();
        let tube_s = clock.elapsed().as_secs_f64();
        let clock = std::time::Instant::now();
        let profile = profile(&case.chain(), &tube, None).unwrap();
        let profile_s = clock.elapsed().as_secs_f64();
        let held_nodes = (0..n).filter(|&i| profile.at_lo[i] || profile.at_hi[i]).count();
        let rise = rises(&profile.z);
        let held_slopes =
            (0..n - 1).filter(|&s| (rise[s] - tube.a[s]).abs() < 1e-9 || (tube.b[s] - rise[s]).abs() < 1e-9).count();
        let worst = profile.z.iter().zip(&case.target).fold(0.0, |m: f64, (z, t)| m.max((z - t).abs()));
        let fixed = tube.fixed.iter().filter(|v| !v.is_nan()).count();
        println!(
            "tube {tube_s:.3} s, profile {profile_s:.3} s, steps {}, held: {held_nodes} node bounds + {held_slopes} slopes at a bound, \
             converged {}, largest |z − t| {worst:.3} m; opened {}, cuts {}, fixed by the thin rule {fixed}, in the tube {}",
            profile.steps,
            profile.converged,
            tube.opened.len(),
            tube.cuts.len(),
            inside(&tube, &profile.z)
        );
        assert!(profile.converged && inside(&tube, &profile.z));
    }
}
