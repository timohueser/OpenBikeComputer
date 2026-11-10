//! The profile of one chain: the heights that minimise its soft terms inside a tube of node and slope
//! bounds, by a primal active-set method whose every iterate lies inside the tube.

use super::curve::LEAST_STEP_M;

/// A spacing under this counts as this (as `curve::LEAST_STEP_M`).
pub(crate) const MIN_STEP_M: f64 = LEAST_STEP_M;
/// A swept interval thinner than this is a fixed node at its middle.
pub(crate) const THIN_M: f64 = 0.02;
/// A step of the active set ends where no height moves more than this.
const STEP_M: f64 = 1e-9;
/// What an opened slope bound leaves past the string's rise, per metre: room that keeps the tube no sliver.
const OPEN: f64 = 1e-6;
/// An opened slope bound also leaves this share of the string's rise as room for the curvature term.
pub(crate) const EASE: f64 = 0.1;
/// A pivot at or under this share of the largest diagonal of H is not positive.
const PIVOT: f64 = 1e-12;
/// A bound this close to a height holds it; a cut under this is none; two step limits this close tie.
const NEAR: f64 = 1e-12;
/// A multiplier has a wrong sign only past this share of the sum of the sizes of its gradient terms:
/// 64 roundings.
const ROUNDING: f64 = 64.0 * (f64::EPSILON / 2.0);

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
    /// n − 1 each, rise per metre; a segment has both sides or none (−∞ and +∞). With `cap`, n − 1, an
    /// opened bound of a segment is at most its cap per metre, but never under the string's rise.
    Slopes { layer: u8, lo: &'a [f64], hi: &'a [f64], cap: Option<&'a [f64]> },
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
    let mut z = fixed.to_vec();
    for range in blocks(fixed, true) {
        let block = Block::new(chain, fixed, None, range.clone());
        let free = block.plain()?;
        for (k, i) in range.enumerate() {
            if fixed[i].is_nan() {
                z[i] = free[k] + block.base;
            }
        }
    }
    Ok(z)
}

/// The ranges of a chain that two fixed nodes in a row separate, each with the pairs at its ends; with
/// `split` false, the whole chain.
fn blocks(fixed: &[f64], split: bool) -> Vec<std::ops::Range<usize>> {
    let n = fixed.len();
    let mut out = Vec::new();
    let mut start = 0;
    for i in 0..n.saturating_sub(1) {
        if split && !fixed[i].is_nan() && !fixed[i + 1].is_nan() {
            out.push(start..i + 2);
            start = i;
        }
    }
    out.push(start..n);
    out.retain(|range| range.clone().any(|i| fixed[i].is_nan()));
    out
}

/// One block of a chain in heights relative to its base: the first fixed height of the block, else its
/// first target with a fit, else 0. Rounding then does not depend on the altitude.
struct Block {
    base: f64,
    l: Vec<f64>,
    fit: Vec<f64>,
    target: Vec<f64>,
    slope: Vec<f64>,
    bend: Vec<f64>,
    tube: Tube,
}

impl Block {
    fn new(chain: &Chain, fixed: &[f64], bounds: Option<&Tube>, range: std::ops::Range<usize>) -> Self {
        let r = range.clone();
        let base = r
            .clone()
            .map(|i| fixed[i])
            .find(|v| !v.is_nan())
            .or_else(|| r.clone().find(|&i| chain.fit[i] > 0.0).map(|i| chain.target[i]))
            .unwrap_or(0.0);
        let segments = r.start..r.end - 1;
        let shift = |v: &[f64]| v[r.clone()].iter().map(|v| v - base).collect::<Vec<f64>>();
        let infinite = |sign: f64| vec![sign * f64::INFINITY; r.len()];
        let tube = Tube {
            fixed: shift(fixed),
            lo: bounds.map_or_else(|| infinite(-1.0), |t| shift(&t.lo)),
            hi: bounds.map_or_else(|| infinite(1.0), |t| shift(&t.hi)),
            a: bounds.map_or_else(|| vec![f64::NEG_INFINITY; segments.len()], |t| t.a[segments.clone()].to_vec()),
            b: bounds.map_or_else(|| vec![f64::INFINITY; segments.len()], |t| t.b[segments.clone()].to_vec()),
            cuts: Vec::new(),
            opened: Vec::new(),
        };
        Block {
            base,
            l: chain.l[segments.clone()].to_vec(),
            fit: chain.fit[r.clone()].to_vec(),
            target: shift(chain.target),
            slope: chain.slope[segments].to_vec(),
            bend: chain.bend[r].to_vec(),
            tube,
        }
    }

    fn chain(&self) -> Chain<'_> {
        Chain { l: &self.l, fit: &self.fit, target: &self.target, slope: &self.slope, bend: &self.bend }
    }

    fn plain(&self) -> Result<Vec<f64>, Pivot> {
        Quadratic::new(&self.chain()).least(&Map::fixed(&self.tube.fixed))
    }
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
pub(crate) fn tube(chain: &Chain, steps: &[Step]) -> Tube {
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
            Step::Slopes { layer, lo, hi, cap } => {
                debug_assert!(!sloped, "at most one Slopes step");
                sloped = true;
                slope(chain, &mut tube, layer, (lo, hi), cap);
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
            return tube;
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
/// to the string's rise and a tenth more.
fn slope(chain: &Chain, tube: &mut Tube, layer: u8, (lo, hi): (&[f64], &[f64]), cap: Option<&[f64]>) {
    let n = tube.lo.len();
    let l = |s: usize| chain.l[s].max(MIN_STEP_M);
    for s in 0..n.saturating_sub(1) {
        debug_assert!(lo[s].is_finite() == hi[s].is_finite(), "a slope bound has two sides or none");
        (tube.a[s], tube.b[s]) = (lo[s] * l(s), hi[s] * l(s));
    }
    let (lower, upper) = sweep(tube);
    if (0..n).all(|i| lower[i] <= upper[i]) {
        return;
    }
    let gate = |i: usize, side: &[f64]| if tube.fixed[i].is_nan() { side[i] } else { tube.fixed[i] };
    let (gate_lo, gate_hi): (Vec<f64>, Vec<f64>) = (0..n).map(|i| (gate(i, &tube.lo), gate(i, &tube.hi))).unzip();
    // The string in heights about the centre of each slope bound, over an abscissa of metres times the
    // bound's half width: each bounded segment then allows the same slope of 1, so the string passes a bound
    // only where no curve of the gates holds it, whatever its centre and width. A segment without a bound
    // takes any rise, so the pieces between such segments are strings of their own.
    let mut rises = vec![0.0; n.saturating_sub(1)];
    let mut start = 0;
    while start + 1 < n {
        let mut end = start;
        while end + 1 < n && lo[end].is_finite() {
            end += 1;
        }
        if end > start {
            let (mut x, mut base) = (vec![0.0], vec![0.0]);
            for s in start..end {
                let (centre, half) = ((lo[s] + hi[s]) / 2.0, ((hi[s] - lo[s]) / 2.0).max(f64::MIN_POSITIVE));
                x.push(x[x.len() - 1] + l(s) * half);
                base.push(base[base.len() - 1] + centre * l(s));
            }
            let about = |side: &[f64]| (start..=end).map(|i| side[i] - base[i - start]).collect::<Vec<f64>>();
            for (k, rise) in string(&x, &about(&gate_lo), &about(&gate_hi)).into_iter().enumerate() {
                rises[start + k] = rise + (base[k + 1] - base[k]);
            }
        }
        start = end + 1;
    }
    let most = |s: usize| cap.map_or(f64::INFINITY, |cap| cap[s] * l(s));
    for (s, rise) in rises.into_iter().enumerate() {
        if rise > tube.b[s] {
            tube.opened.push(Opened { layer, segment: s, excess: (rise - tube.b[s]) / l(s) });
            tube.b[s] = (rise + EASE * rise.abs() + OPEN * l(s)).min(most(s)).max(rise + OPEN * l(s));
        } else if rise < tube.a[s] {
            tube.opened.push(Opened { layer, segment: s, excess: (tube.a[s] - rise) / l(s) });
            tube.a[s] = (rise - EASE * rise.abs() - OPEN * l(s)).max(-most(s)).min(rise - OPEN * l(s));
        }
    }
}

/// A corner of the funnel: (node, height).
type Corner = (usize, f64);

/// The rises of the tight string through the gates [lo_i, hi_i] at the abscissae x_i: the curve of least
/// Σ rise²/Δx in the gates. A gate side can be infinite; lo_i ≤ hi_i; a fixed node is a gate of one point.
/// The funnel method: one pass, each corner enters and leaves a chain once; slopes are compared by cross
/// products, so no division decides a contact.
pub(crate) fn string(x: &[f64], lo: &[f64], hi: &[f64]) -> Vec<f64> {
    let n = x.len();
    let mut rises = vec![0.0; n.saturating_sub(1)];
    let Some((p, zp)) = contact(lo, hi, 0..n) else { return rises };
    let (q, zq) = contact(lo, hi, (0..n).rev()).expect("a string that leaves the level has a last contact");
    debug_assert!(q > p);
    // slope(a→b) against slope(c→d), as the two cross products.
    let cross =
        |a: Corner, b: Corner, c: Corner, d: Corner| ((b.1 - a.1) * (x[d.0] - x[c.0]), (d.1 - c.1) * (x[b.0] - x[a.0]));
    let mut apex = (p, zp);
    let mut out = vec![apex];
    let (mut upper, mut lower) = (std::collections::VecDeque::new(), std::collections::VecDeque::new());
    for g in p + 1..=q {
        let (t, b) = if g == q { (zq, zq) } else { (hi[g], lo[g]) };
        if t.is_finite() {
            let corner = (g, t);
            while upper.len() >= 2 {
                let (l, r) = cross(upper[upper.len() - 2], upper[upper.len() - 1], upper[upper.len() - 1], corner);
                if l < r {
                    break;
                }
                upper.pop_back();
            }
            if upper.len() == 1 {
                let (l, r) = cross(apex, upper[0], upper[0], corner);
                if l >= r {
                    upper.pop_back();
                }
            }
            if upper.is_empty() {
                while let Some(&first) = lower.front() {
                    let (l, r) = cross(apex, corner, apex, first);
                    if l >= r {
                        break;
                    }
                    apex = first;
                    lower.pop_front();
                    out.push(apex);
                }
            }
            upper.push_back(corner);
        }
        if b.is_finite() {
            let corner = (g, b);
            while lower.len() >= 2 {
                let (l, r) = cross(lower[lower.len() - 2], lower[lower.len() - 1], lower[lower.len() - 1], corner);
                if l > r {
                    break;
                }
                lower.pop_back();
            }
            if lower.len() == 1 {
                let (l, r) = cross(apex, lower[0], lower[0], corner);
                if l <= r {
                    lower.pop_back();
                }
            }
            if lower.is_empty() {
                while let Some(&first) = upper.front() {
                    let (l, r) = cross(apex, corner, apex, first);
                    if l <= r {
                        break;
                    }
                    apex = first;
                    upper.pop_front();
                    out.push(apex);
                }
            }
            lower.push_back(corner);
        }
    }
    if out.last() != Some(&(q, zq)) {
        out.push((q, zq));
    }
    for pair in out.windows(2) {
        let ((i, zi), (k, zk)) = (pair[0], pair[1]);
        for (s, rise) in rises.iter_mut().enumerate().take(k).skip(i) {
            *rise = (zk - zi) * (x[s + 1] - x[s]) / (x[k] - x[i]);
        }
    }
    rises
}

/// The first gate, in `order`, that the heights of a level line from the first gate of `order` touch when
/// the next gate leaves them: (its node, its height); none if a level line runs through every gate.
fn contact(lo: &[f64], hi: &[f64], mut order: impl Iterator<Item = usize>) -> Option<Corner> {
    let first = order.next()?;
    let (mut bottom, mut top, mut at_bottom, mut at_top) = (lo[first], hi[first], first, first);
    for j in order {
        if lo[j] > top {
            return Some((at_top, top));
        }
        if hi[j] < bottom {
            return Some((at_bottom, bottom));
        }
        if lo[j] >= bottom {
            (bottom, at_bottom) = (lo[j], j);
        }
        if hi[j] <= top {
            (top, at_top) = (hi[j], j);
        }
    }
    None
}

/// The least of F in the tube. Every iterate lies in the tube, so the step cap returns a valid profile.
/// Two fixed nodes in a row separate the chain: each block between them is solved alone.
pub(crate) fn profile(chain: &Chain, tube: &Tube, cap: Option<usize>) -> Result<Profile, Pivot> {
    blockwise(chain, tube, cap, true)
}

/// `profile`, by blocks or, with `split` false, of the whole chain at once.
fn blockwise(chain: &Chain, tube: &Tube, cap: Option<usize>, split: bool) -> Result<Profile, Pivot> {
    let n = chain.fit.len();
    let mut out = Profile {
        z: tube.fixed.clone(),
        plain: true,
        converged: true,
        steps: 0,
        at_lo: vec![false; n],
        at_hi: vec![false; n],
    };
    for range in blocks(&tube.fixed, split) {
        let block = Block::new(chain, &tube.fixed, Some(tube), range.clone());
        let part = solve(&block, cap)?;
        for (k, i) in range.enumerate() {
            if tube.fixed[i].is_nan() {
                (out.z[i], out.at_lo[i], out.at_hi[i]) = (part.z[k] + block.base, part.at_lo[k], part.at_hi[k]);
            }
        }
        out.steps += part.steps;
        out.plain &= part.plain;
        out.converged &= part.converged;
    }
    Ok(out)
}

/// The primal active set on one block.
fn solve(block: &Block, cap: Option<usize>) -> Result<Profile, Pivot> {
    let (chain, tube) = (block.chain(), &block.tube);
    let n = chain.fit.len();
    let h = Quadratic::new(&chain);
    let z0 = block.plain()?;
    let mut work = Work { node: vec![Held::No; n], segment: vec![Held::No; n.saturating_sub(1)] };
    if inside(tube, &z0) {
        return Ok(work.profile(z0, true, true, 0));
    }
    let mut x = start(tube, &z0);
    work.take_active(tube, &x);
    let count = (0..4 * n.saturating_sub(1) + 2).filter(|&c| exists(tube, c)).count();
    let cap = cap.unwrap_or((3 * count + 100).min(20_000));
    for step in 1..=cap {
        let runs = work.runs(tube);
        let ideal = h.least(&runs.map)?;
        let p: Vec<f64> = (0..n).map(|i| if runs.map.unknown[i].is_some() { ideal[i] - x[i] } else { 0.0 }).collect();
        if p.iter().all(|v| v.abs() <= STEP_M) {
            match work.wrong_sign(&chain, &ideal, tube, &runs) {
                None => return Ok(work.profile(ideal, false, true, step)),
                Some(c) => work.release(c, n),
            }
            x = ideal;
            continue;
        }
        let (alpha, stop) = blocking(tube, &work, &x, &p);
        for i in 0..n {
            x[i] = if alpha == 1.0 && runs.map.unknown[i].is_some() { ideal[i] } else { x[i] + alpha * p[i] };
        }
        if let Some(c) = stop {
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

/// The gradient of F at z from its terms in difference form, and per node the sum of the sizes of the
/// terms: what rounding can move the gradient by.
fn gradient(chain: &Chain, z: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let n = z.len();
    let l = |s: usize| chain.l[s].max(MIN_STEP_M);
    let (mut g, mut size) = (vec![0.0; n], vec![0.0; n]);
    let mut add = |i: usize, v: f64| {
        g[i] += v;
        size[i] += v.abs();
    };
    for (i, z) in z.iter().enumerate() {
        add(i, 2.0 * chain.fit[i] * (z - chain.target[i]));
    }
    for s in 0..n.saturating_sub(1) {
        let e = 2.0 * chain.slope[s] * (z[s + 1] - z[s]) / l(s);
        add(s + 1, e);
        add(s, -e);
    }
    for k in 1..n.saturating_sub(1) {
        let (l0, l1) = (l(k - 1), l(k));
        let kappa = (z[k + 1] - z[k]) / l1 - (z[k] - z[k - 1]) / l0;
        let m = 2.0 * (chain.bend[k] / ((l0 + l1) / 2.0)) * kappa;
        add(k - 1, m / l0);
        add(k, -m * (1.0 / l0 + 1.0 / l1));
        add(k + 1, m / l1);
    }
    (g, size)
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

    /// The lowest numbered constraint of the working set whose multiplier at `z` has a wrong sign past its
    /// rounding. The multipliers are running sums of the gradient along each run: from its first node, and
    /// from its last node past its known node.
    fn wrong_sign(&self, chain: &Chain, z: &[f64], tube: &Tube, runs: &Runs) -> Option<usize> {
        let n = z.len();
        let (g, size) = gradient(chain, z);
        let mut wrong: Option<usize> = None;
        let mut check = |c: usize, held: Held, multiplier: f64, sizes: f64| {
            let tol = ROUNDING * sizes;
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
            let (mut sum, mut sizes) = (0.0, 0.0);
            for s in j..k {
                (sum, sizes) = (sum + g[s], sizes + size[s]);
                if known.is_none_or(|q| s < q) {
                    check(2 * n + 2 * s + usize::from(self.segment[s] == Held::Hi), self.segment[s], -sum, sizes);
                }
            }
            if let Some(q) = known {
                let (mut sum, mut sizes) = (0.0, 0.0);
                for s in (q..k).rev() {
                    (sum, sizes) = (sum + g[s + 1], sizes + size[s + 1]);
                    check(2 * n + 2 * s + usize::from(self.segment[s] == Held::Hi), self.segment[s], sum, sizes);
                }
                if tube.fixed[q].is_nan() {
                    let (beta, sizes) = (j..=k).fold((0.0, 0.0), |(b, a), i| (b + g[i], a + size[i]));
                    check(2 * q + usize::from(self.node[q] == Held::Hi), self.node[q], beta, sizes);
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
                    Rule::Slopes(layer, lo, hi) => Step::Slopes { layer: *layer, lo, hi, cap: None },
                })
                .collect()
        }

        fn solve(&self, cap: Option<usize>) -> (Tube, Profile) {
            let tube = tube(&self.chain(), &self.steps());
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
        assert!(rise[9] >= 10.0 - 1e-9 && rise[9] <= 11.0 + 1e-5, "{rise:?}");
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
        assert_eq!(tube.fixed.iter().filter(|v| !v.is_nan()).count(), 2, "{:?}", tube.fixed);
        assert!(profile.plain);
        assert!((0..16).all(|i| (profile.z[i] + 4.0 * i as f64).abs() < 1e-9), "{:?}", profile.z);
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
        assert!((tube.b[10] - (3.0 + 0.3 + 2e-6)).abs() < 1e-9, "{}", tube.b[10]);
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

    /// The rises of the string of the old method: the least of Σ rise²/l plus a pull of 1e-8 per metre to
    /// `target`, by the active set, in the gates.
    fn oracle(l: &[f64], lo: &[f64], hi: &[f64], fixed: &[f64], target: &[f64]) -> Vec<f64> {
        let n = lo.len();
        let fit: Vec<f64> = ground(l).iter().map(|m| 1e-8 * m).collect();
        let target: Vec<f64> = (0..n).map(|i| target[i].max(lo[i]).min(hi[i])).collect();
        let (slope, bend) = (vec![1.0; n - 1], vec![0.0; n]);
        let chain = Chain { l, fit: &fit, target: &target, slope: &slope, bend: &bend };
        let tube = Tube {
            fixed: fixed.to_vec(),
            lo: lo.to_vec(),
            hi: hi.to_vec(),
            a: vec![-INF; n - 1],
            b: vec![INF; n - 1],
            cuts: Vec::new(),
            opened: Vec::new(),
        };
        let profile = profile(&chain, &tube, None).unwrap();
        assert!(profile.converged);
        rises(&profile.z)
    }

    /// x_i = 2·i.
    fn every_two(n: usize) -> Vec<f64> {
        (0..n).map(|i| 2.0 * i as f64).collect()
    }

    fn near(a: &[f64], b: &[f64], tolerance: f64) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(a, b)| (a - b).abs() <= tolerance)
    }

    #[test]
    fn a_string_that_fits_level_is_level() {
        let lo: Vec<f64> = (0..11).map(|i| 2.0 - 0.1 * i as f64).collect();
        let hi: Vec<f64> = (0..11).map(|i| 4.0 + 0.1 * i as f64).collect();
        assert!(string(&every_two(11), &lo, &hi).iter().all(|&r| r == 0.0));
    }

    #[test]
    fn the_string_crosses_a_step_of_the_gates_in_one_segment() {
        let lo: Vec<f64> = (0..20).map(|i| if i <= 9 { -5.0 } else { 15.0 }).collect();
        let hi: Vec<f64> = lo.iter().map(|v| v + 10.0).collect();
        let rise = string(&every_two(20), &lo, &hi);
        assert!(rise[9] == 10.0 && (0..19).filter(|&s| s != 9).all(|s| rise[s] == 0.0), "{rise:?}");
    }

    /// Gates of n nodes with no sides, and the nodes `fixed` at their heights.
    fn gates(n: usize, fixed: &[(usize, f64)]) -> (Vec<f64>, Vec<f64>) {
        let (mut lo, mut hi) = (vec![-INF; n], vec![INF; n]);
        for &(i, v) in fixed {
            (lo[i], hi[i]) = (v, v);
        }
        (lo, hi)
    }

    #[test]
    fn the_string_between_two_fixed_heights_is_straight() {
        let (lo, hi) = gates(11, &[(0, 0.0), (10, 10.0)]);
        assert!(string(&every_two(11), &lo, &hi).iter().all(|r| (r - 1.0).abs() < 1e-12));
    }

    #[test]
    fn the_string_bends_at_a_corner() {
        let (mut lo, hi) = gates(21, &[(0, 0.0), (20, 0.0)]);
        lo[10] = 5.0;
        let rise = string(&every_two(21), &lo, &hi);
        assert!((0..20).all(|s| (rise[s] - if s < 10 { 0.5 } else { -0.5 }).abs() < 1e-12), "{rise:?}");
    }

    #[test]
    fn free_ends_take_the_nearest_heights() {
        let (mut lo, mut hi) = gates(21, &[]);
        (hi[0], lo[10], hi[20]) = (0.0, 5.0, 0.0);
        let rise = string(&every_two(21), &lo, &hi);
        assert!((0..20).all(|s| (rise[s] - if s < 10 { 0.5 } else { -0.5 }).abs() < 1e-12), "{rise:?}");
    }

    #[test]
    fn a_corner_on_the_straight_line_is_no_contact() {
        let (lo, mut hi) = gates(11, &[(0, 0.0), (10, 10.0)]);
        hi[5] = 5.0;
        assert!(string(&every_two(11), &lo, &hi).iter().all(|r| (r - 1.0).abs() < 1e-12));
        let (lo, hi) = gates(11, &[(0, 0.0), (5, 5.0), (10, 10.0)]);
        assert!(string(&every_two(11), &lo, &hi).iter().all(|r| (r - 1.0).abs() < 1e-12));
    }

    #[test]
    fn the_string_wraps_a_curved_top() {
        let (lo, mut hi) = gates(21, &[(0, 0.0), (20, 0.0)]);
        for (i, h) in hi.iter_mut().enumerate().take(17).skip(4) {
            *h = 0.02 * ((i as f64 - 10.0) * (i as f64 - 10.0)) - 1.0;
        }
        let rise = string(&every_two(21), &lo, &hi);
        let z: Vec<f64> = (0..21).map(|i| rise[..i].iter().sum::<f64>()).collect();
        assert!((0..21).all(|i| z[i] <= hi[i] + 1e-12), "{z:?}");
        assert!(z[20].abs() < 1e-9);
        assert!((0..21).filter(|&i| hi[i].is_finite() && (z[i] - hi[i]).abs() < 1e-9).count() >= 3, "{z:?}");
        let oracle = oracle(&[2.0; 20], &lo, &hi, &at(21, &[(0, 0.0), (20, 0.0)]), &[0.0; 21]);
        assert!(near(&rise, &oracle, 1e-6), "{rise:?}\n{oracle:?}");
    }

    #[test]
    fn the_string_is_the_least_of_the_sum_of_rise_squared_over_length() {
        let mut rng = Lcg(8);
        for case in 0..60 {
            let n = 8;
            let l: Vec<f64> = (0..n - 1).map(|_| 1.0 + 2.0 * rng.next()).collect();
            let mut t = vec![0.0; n];
            for i in 1..n {
                t[i] = t[i - 1] + 6.0 * rng.next() - 3.0;
            }
            let w: Vec<f64> = (0..n).map(|_| 0.3 + 1.7 * rng.next()).collect();
            let (mut lo, mut hi): (Vec<f64>, Vec<f64>) = (0..n).map(|i| (t[i] - w[i], t[i] + w[i])).unzip();
            let mut fixed = vec![f64::NAN; n];
            if case % 2 == 0 {
                let i = ((rng.next() * n as f64) as usize).min(n - 1);
                fixed[i] = lo[i] + rng.next() * (hi[i] - lo[i]);
                (lo[i], hi[i]) = (fixed[i], fixed[i]);
            }
            let x: Vec<f64> = std::iter::once(0.0)
                .chain(l.iter().scan(0.0, |x, l| {
                    *x += l;
                    Some(*x)
                }))
                .collect();
            let rise = string(&x, &lo, &hi);
            let oracle = oracle(&l, &lo, &hi, &fixed, &t);
            assert!(near(&rise, &oracle, 1e-5), "case {case}: {rise:?}\n{oracle:?}");
        }
    }

    #[test]
    fn a_ramp_far_from_a_contradiction_is_not_opened() {
        // Nodes 10 and 11 contradict the grade; segments 30..40 are a ramp [0.05, 0.55] on 30 % terrain, which
        // the terrain itself holds.
        let n = 41;
        let l = even(n, 2.0);
        let target: Vec<f64> = (0..n).map(|i| if i <= 25 { 0.0 } else { 0.6 * (i as f64 - 25.0) }).collect();
        let (mut slo, mut shi) = (vec![-0.12; n - 1], vec![0.12; n - 1]);
        for s in 30..40 {
            (slo[s], shi[s]) = (0.05, 0.55);
        }
        let rules = vec![
            Rule::Fixed(at(n, &[(10, 0.0), (11, 3.0)])),
            Rule::Bounds(2, target.iter().map(|t| t - 5.0).collect(), target.iter().map(|t| t + 5.0).collect()),
            Rule::Slopes(4, slo, shi),
        ];
        let case = Case { fit: ground(&l), target, slope: vec![0.0; n - 1], bend: bend(n, 3.0), l, rules };
        let (tube, profile) = case.solve(None);
        assert_eq!(tube.opened.iter().map(|o| o.segment).collect::<Vec<_>>(), [10]);
        assert!(inside(&tube, &profile.z));
    }

    #[test]
    fn the_opened_slopes_are_those_of_the_least_string_about_the_bound_centres() {
        let mut rng = Lcg(31);
        let mut opened = 0;
        for case in 0..200 {
            let n = 7;
            let l: Vec<f64> = (0..n - 1).map(|_| 0.5 + 2.5 * rng.next()).collect();
            let mut t = vec![0.0; n];
            for i in 1..n {
                t[i] = t[i - 1] + 3.0 * rng.next() - 1.5;
            }
            let w: Vec<f64> = (0..n).map(|_| 0.2 + 1.5 * rng.next()).collect();
            let (lo, hi): (Vec<f64>, Vec<f64>) = (0..n).map(|i| (t[i] - w[i], t[i] + w[i])).unzip();
            // Ramps of any centre and width, segments without a bound, and two fixed nodes that may contradict.
            let (mut slo, mut shi) = (vec![-INF; n - 1], vec![INF; n - 1]);
            for s in 0..n - 1 {
                if rng.next() < 0.8 {
                    let (centre, half) = (1.2 * rng.next() - 0.6, 0.05 + 0.3 * rng.next());
                    (slo[s], shi[s]) = (centre - half, centre + half);
                }
            }
            let i = ((rng.next() * (n - 2) as f64) as usize).min(n - 3);
            let fixed = at(n, &[(i, t[i]), (i + 2, t[i] + 4.0 * rng.next() - 2.0)]);
            let (fit, flat, curve) = (ground(&l), vec![0.0; n - 1], bend(n, 2.0));
            let chain = Chain { l: &l, fit: &fit, target: &t, slope: &flat, bend: &curve };
            let nodes = || [Step::Fixed(&fixed), Step::Bounds { layer: 2, lo: &lo, hi: &hi }];
            let before = tube(&chain, &nodes());
            let [first, second] = nodes();
            let after = tube(&chain, &[first, second, Step::Slopes { layer: 4, lo: &slo, hi: &shi, cap: None }]);
            // The oracle: per piece of bounded segments, the least Σ rise²/x of the heights about the centres,
            // over metres times half widths, in the gates of the tube before the slopes.
            let gate = |i: usize, side: &[f64]| if before.fixed[i].is_nan() { side[i] } else { before.fixed[i] };
            let mut want = vec![(-INF, INF); n - 1];
            let mut start = 0;
            while start + 1 < n {
                let mut end = start;
                while end + 1 < n && slo[end].is_finite() {
                    end += 1;
                }
                if end > start {
                    let segments: Vec<usize> = (start..end).collect();
                    // A common scale keeps every length of the oracle at MIN_STEP_M or more; it does not move a string.
                    let stretched: Vec<f64> = segments.iter().map(|&s| l[s] * (shi[s] - slo[s]) / 2.0).collect();
                    let scale = MIN_STEP_M / stretched.iter().copied().fold(INF, f64::min);
                    let stretched: Vec<f64> = stretched.iter().map(|x| x * scale).collect();
                    let mut base = vec![0.0];
                    for &s in &segments {
                        base.push(base[base.len() - 1] + (slo[s] + shi[s]) / 2.0 * l[s]);
                    }
                    let side = |v: &[f64]| (start..=end).map(|i| gate(i, v) - base[i - start]).collect::<Vec<_>>();
                    let (glo, ghi) = (side(&before.lo), side(&before.hi));
                    let pinned: Vec<f64> =
                        (0..glo.len()).map(|k| if glo[k] == ghi[k] { glo[k] } else { f64::NAN }).collect();
                    let middle: Vec<f64> = (0..glo.len()).map(|k| t[start + k] - base[k]).collect();
                    for (k, r) in oracle(&stretched, &glo, &ghi, &pinned, &middle).into_iter().enumerate() {
                        let s = start + k;
                        let rise = r + (base[k + 1] - base[k]);
                        let (a, b) = (slo[s] * l[s], shi[s] * l[s]);
                        want[s] = if rise > b + 1e-6 {
                            (a, rise + EASE * rise.abs() + OPEN * l[s])
                        } else if rise < a - 1e-6 {
                            (rise - EASE * rise.abs() - OPEN * l[s], b)
                        } else {
                            (a, b)
                        };
                    }
                }
                start = end + 1;
            }
            let near = |x: f64, y: f64| x == y || (x - y).abs() < 1e-4;
            for (s, &(a, b)) in want.iter().enumerate() {
                let got = (after.a[s], after.b[s]);
                assert!(near(got.0, a) && near(got.1, b), "case {case} segment {s}: {got:?} {:?}", (a, b));
            }
            opened += after.opened.len();
        }
        assert!(opened > 50, "{opened}");
    }

    #[test]
    fn fixed_nodes_in_a_row_cut_a_profile_into_blocks() {
        let mut rng = Lcg(12);
        for number in 0..20 {
            let mut case = generated(&mut rng, 200, false);
            let fixed = at(200, &[60, 61, 140, 141].map(|i| (i, case.target[i])));
            case.rules.insert(0, Rule::Fixed(fixed));
            let tube = tube(&case.chain(), &case.steps());
            let split = profile(&case.chain(), &tube, None).unwrap();
            let whole = blockwise(&case.chain(), &tube, None, false).unwrap();
            assert!(split.converged && whole.converged, "case {number}");
            assert!(near(&split.z, &whole.z, 1e-9), "case {number}");
            let steps: usize = blocks(&tube.fixed, true)
                .into_iter()
                .map(|range| solve(&Block::new(&case.chain(), &tube.fixed, Some(&tube), range), None).unwrap().steps)
                .sum();
            assert_eq!(split.steps, steps, "case {number}");
        }
    }

    #[test]
    fn the_sign_test_holds_at_altitude() {
        let at_height = |offset: f64| {
            let mut case = step_of_terrain(10.0);
            case.target.iter_mut().for_each(|t| *t += offset);
            case.bend = bend(20, 80.0);
            case.rules = vec![
                Rule::Bounds(
                    2,
                    case.target.iter().map(|t| t - 5.0).collect(),
                    case.target.iter().map(|t| t + 5.0).collect(),
                ),
                Rule::Slopes(4, vec![-0.05; 19], vec![0.05; 19]),
            ];
            case.solve(None).1
        };
        let (high, low) = (at_height(2500.0), at_height(0.0));
        assert!(high.converged && low.converged);
        assert!(high.z.iter().zip(&low.z).all(|(h, l)| (h - 2500.0 - l).abs() < 1e-9), "{:?}\n{:?}", high.z, low.z);
    }

    /// The climb of the review with noise: n nodes 2 m apart, t = 0.124·s + 2·sin(s/7) + u, bend(30), the band
    /// ±5 m and the grade ±0.12; (seconds of `tube`, seconds of `profile`, the profile, opened, thin).
    fn climb(n: usize, seed: u64) -> (f64, f64, Profile, usize, usize) {
        let l = even(n, 2.0);
        let mut rng = Lcg(seed);
        let target: Vec<f64> =
            along(&l).iter().map(|s| 0.124 * s + 2.0 * (s / 7.0).sin() + 2.0 * rng.next() - 1.0).collect();
        let rules = vec![
            Rule::Bounds(2, target.iter().map(|t| t - 5.0).collect(), target.iter().map(|t| t + 5.0).collect()),
            Rule::Slopes(4, vec![-0.12; n - 1], vec![0.12; n - 1]),
        ];
        let case = Case { fit: ground(&l), target, slope: vec![0.0; n - 1], bend: bend(n, 30.0), l, rules };
        let clock = std::time::Instant::now();
        let tube = tube(&case.chain(), &case.steps());
        let tube_s = clock.elapsed().as_secs_f64();
        let clock = std::time::Instant::now();
        let profile = profile(&case.chain(), &tube, None).unwrap();
        let profile_s = clock.elapsed().as_secs_f64();
        assert!(inside(&tube, &profile.z));
        let thin = tube.fixed.iter().filter(|v| !v.is_nan()).count();
        assert!(thin * 100 < n, "the thin rule fixes {thin} of {n} nodes");
        (tube_s, profile_s, profile, tube.opened.len(), thin)
    }

    fn report(name: &str, (tube_s, profile_s, profile, opened, thin): &(f64, f64, Profile, usize, usize)) {
        println!(
            "{name}: tube {tube_s:.4} s, profile {profile_s:.4} s, steps {}, opened {opened}, fixed by the thin rule {thin}, converged {}",
            profile.steps, profile.converged
        );
    }

    #[test]
    #[ignore]
    fn benchmark_the_long_climb_of_the_review() {
        let run = climb(1_250, 1);
        report("1,250 nodes", &run);
        assert!(run.2.converged);
    }

    #[test]
    #[ignore]
    fn benchmark_the_longest_chain_of_box_b() {
        let run = climb(6_000, 2);
        report("6,000 nodes", &run);
        assert!(run.2.converged);
    }

    #[test]
    #[ignore]
    fn benchmark_a_thousand_chains_of_the_99th_percentile() {
        let clock = std::time::Instant::now();
        let (mut tube_s, mut profile_s, mut steps, mut opened, mut thin, mut unconverged) = (0.0, 0.0, 0, 0, 0, 0);
        for seed in 1..=1000 {
            let run = climb(950, seed);
            (tube_s, profile_s, steps) = (tube_s + run.0, profile_s + run.1, steps + run.2.steps);
            (opened, thin, unconverged) = (opened + run.3, thin + run.4, unconverged + usize::from(!run.2.converged));
        }
        println!(
            "1,000 x 950 nodes: all {:.3} s (tube {tube_s:.3} s, profile {profile_s:.3} s), steps {steps}, opened {opened}, \
             fixed by the thin rule {thin}, unconverged {unconverged}",
            clock.elapsed().as_secs_f64()
        );
        assert_eq!(unconverged, 0);
    }

    #[test]
    #[ignore]
    fn benchmark_stress_of_25000_nodes() {
        report("25,000 nodes", &climb(25_000, 1));
    }
}
