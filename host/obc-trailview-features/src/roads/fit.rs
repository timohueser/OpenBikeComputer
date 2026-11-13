//! The least squares of the road solver, its sparse rows, and the graph searches of its regions.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use faer::dyn_stack::{MemBuffer, MemStack};
use faer::linalg::cholesky::ldlt::factor::LdltRegularization;
use faer::sparse::linalg::cholesky::{factorize_symbolic_cholesky, SymmetricOrdering};
use faer::sparse::{SparseColMatRef, SymbolicSparseColMatRef};
use faer::{Conj, MatMut, Par, Side};

use super::{ARMIJO, BISECTIONS, REFINE_M, REFINE_STEPS, ROUNDS, SETTLE_M};

/// Sparse rows: per row, its (variable, coefficient) entries.
#[derive(Clone)]
pub(crate) struct Rows {
    ptr: Vec<usize>,
    col: Vec<usize>,
    val: Vec<f64>,
}

impl Default for Rows {
    fn default() -> Self {
        Rows { ptr: vec![0], col: Vec::new(), val: Vec::new() }
    }
}

impl Rows {
    pub fn push(&mut self, entries: impl IntoIterator<Item = (usize, f64)>) {
        for (col, val) in entries {
            self.col.push(col);
            self.val.push(val);
        }
        self.ptr.push(self.col.len());
    }

    pub fn len(&self) -> usize {
        self.ptr.len() - 1
    }

    pub fn row(&self, i: usize) -> impl Iterator<Item = (usize, f64)> + '_ {
        (self.ptr[i]..self.ptr[i + 1]).map(|e| (self.col[e], self.val[e]))
    }

    /// A · z.
    pub fn times(&self, z: &[f64]) -> Vec<f64> {
        (0..self.len()).map(|i| self.row(i).map(|(c, v)| v * z[c]).sum()).collect()
    }
}

/// The rows of a term: sparse rows, or a row (1 − t)·z_a + t·z_b per terrain sample on its segment.
#[derive(Clone, Copy)]
pub(crate) enum Block<'a> {
    Rows(&'a Rows),
    Samples { seg: &'a [usize], t: &'a [f64], a: &'a [usize], b: &'a [usize] },
}

impl Block<'_> {
    fn len(&self) -> usize {
        match self {
            Block::Rows(rows) => rows.len(),
            Block::Samples { seg, .. } => seg.len(),
        }
    }

    /// Its row i times z.
    fn at(&self, i: usize, z: &[f64]) -> f64 {
        let mut sum = 0.0;
        self.each(i, |v, c| sum += c * z[v]);
        sum
    }

    fn each(&self, i: usize, mut f: impl FnMut(usize, f64)) {
        match self {
            Block::Rows(rows) => rows.row(i).for_each(|(col, val)| f(col, val)),
            Block::Samples { seg, t, a, b } => {
                f(a[seg[i]], 1.0 - t[i]);
                f(b[seg[i]], t[i]);
            }
        }
    }
}

/// Rows (A, w, c) of the least squares Σ w·(A·z − c)².
pub(crate) struct Term<'a> {
    pub block: Block<'a>,
    pub w: &'a [f64],
    pub c: &'a [f64],
}

/// The rows of the terms that hold a variable that moves, as one matrix on those variables, numbered
/// from 0, and their targets.
struct Stack<'a> {
    terms: &'a [Term<'a>],
    /// Per row, its term and its row there.
    rows: Vec<(u32, u32)>,
    local: Vec<usize>,
    m: usize,
    c: Vec<f64>,
}

impl Stack<'_> {
    fn term(&self, r: usize) -> (&Term<'_>, usize) {
        let (k, i) = self.rows[r];
        (&self.terms[k as usize], i as usize)
    }

    fn each(&self, r: usize, mut f: impl FnMut(usize, f64)) {
        let (term, i) = self.term(r);
        term.block.each(i, |col, val| {
            if self.local[col] != usize::MAX {
                f(self.local[col], val);
            }
        });
    }

    /// A · v.
    fn times(&self, v: &[f64]) -> Vec<f64> {
        (0..self.rows.len())
            .map(|r| {
                let mut sum = 0.0;
                self.each(r, |col, val| sum += val * v[col]);
                sum
            })
            .collect()
    }

    /// Σ Aᵀ·w·r.
    fn normal(&self, r: &[f64]) -> Vec<f64> {
        let mut out = vec![0.0; self.m];
        for (row, &value) in r.iter().enumerate() {
            let (term, i) = self.term(row);
            let wr = term.w[i] * value;
            self.each(row, |col, val| out[col] += val * wr);
        }
        out
    }
}

/// The heights at the least squares of the terms, or `z` with only its variables `moved` at it; and
/// whether the refinement converged. The factorization of the normal matrix loses a height that only
/// weak rows hold, such as the end of a tunnel that meets no way, in the round-off of the PIN rows
/// times the heights. GMRES with that factorization then solves against the residuals of the rows,
/// which keep it.
pub(crate) fn fit(terms: &[Term], n: usize, from: Option<(&[f64], &[usize])>) -> (Vec<f64>, bool) {
    let rows = (0..terms.len()).flat_map(|k| (0..terms[k].block.len()).map(move |i| (k as u32, i as u32)));
    fit_rows(terms, n, from, rows)
}

/// `fit` with the rows of the terms that may hold a variable that moves: (term, row).
fn fit_rows(
    terms: &[Term],
    n: usize,
    from: Option<(&[f64], &[usize])>,
    candidates: impl Iterator<Item = (u32, u32)>,
) -> (Vec<f64>, bool) {
    let (m, local) = match from {
        None => (n, (0..n).collect::<Vec<_>>()),
        Some((_, moved)) => {
            let mut local = vec![usize::MAX; n];
            for (i, &v) in moved.iter().enumerate() {
                local[v] = i;
            }
            (moved.len(), local)
        }
    };
    let (mut rows, mut c) = (Vec::new(), Vec::new());
    for (k, i) in candidates {
        let (term, i) = (&terms[k as usize], i as usize);
        let mut moves = false;
        term.block.each(i, |col, _| moves |= local[col] != usize::MAX);
        if !moves {
            continue;
        }
        let mut target = term.c[i];
        if let Some((z, _)) = from {
            term.block.each(i, |col, val| target -= val * z[col]);
        }
        rows.push((k, i as u32));
        c.push(target);
    }
    let stack = Stack { terms, rows, local, m, c };
    let solver = Solver::new(&stack);
    let residual = |x: &[f64]| stack.times(x).iter().zip(&stack.c).map(|(ax, c)| c - ax).collect::<Vec<_>>();
    let step = solver.solve(stack.normal(&stack.c));
    let rest = solver.solve(stack.normal(&residual(&step)));
    let apply = |v: &[f64]| solver.solve(stack.normal(&stack.times(v)));
    let (more, converged) = gmres(&apply, &rest, REFINE_M, REFINE_STEPS);
    let solved: Vec<f64> = step.iter().zip(&more).map(|(s, m)| s + m).collect();
    match from {
        None => (solved, converged),
        Some((z, moved)) => {
            let mut out = z.to_vec();
            for (i, &v) in moved.iter().enumerate() {
                out[v] += solved[i];
            }
            (out, converged)
        }
    }
}

/// Per variable, the rows of some blocks that hold it: (block, row).
struct Holders {
    ptr: Vec<usize>,
    rows: Vec<(u32, u32)>,
}

impl Holders {
    fn new(blocks: &[Block], n: usize) -> Holders {
        let mut ptr = vec![0usize; n + 1];
        for block in blocks {
            (0..block.len()).for_each(|i| block.each(i, |v, _| ptr[v + 1] += 1));
        }
        for v in 0..n {
            ptr[v + 1] += ptr[v];
        }
        let (mut fill, mut rows) = (ptr.clone(), vec![(0, 0); ptr[n]]);
        for (k, block) in blocks.iter().enumerate() {
            for i in 0..block.len() {
                block.each(i, |v, _| {
                    rows[fill[v]] = (k as u32, i as u32);
                    fill[v] += 1;
                });
            }
        }
        Holders { ptr, rows }
    }

    /// The rows that hold a variable of `region`, each once, in order.
    fn of(&self, region: &[usize]) -> Vec<(u32, u32)> {
        let mut out: Vec<(u32, u32)> =
            region.iter().flat_map(|&v| &self.rows[self.ptr[v]..self.ptr[v + 1]]).copied().collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Limits: rows A·z with a band from `lo(z)` to `hi` and a cost per square metre past it; a row of cost 0
/// does not act. A lower bound may follow heights that its row does not move.
pub(crate) struct Limits<'a> {
    pub rows: &'a Rows,
    pub lo: &'a dyn Fn(&[f64]) -> Vec<f64>,
    pub hi: &'a [f64],
    pub cost: &'a [f64],
}

/// The distance of x past the band from lo to hi, signed: positive above, negative below.
fn past(x: f64, lo: f64, hi: f64) -> f64 {
    x - x.max(lo).min(hi)
}

/// Moves z on the variables `moved` to the minimum of the cost: the least squares of `terms` plus, per limit,
/// its cost times the square of its distance past its band. Damped Newton rounds for each of `scales`, which
/// multiply the costs: soft costs find the limits that act in a few rounds, and each next scale starts from
/// there. A round solves the quadratic model of the cost at z, in which each limit past its band pulls to the
/// bound that it passed, and each piece of the network that the solve moves steps along it as far as its cost
/// falls. With fixed bands the cost is convex with a continuous slope, so it falls in every round; a lower
/// bound that follows other heights (`Limits::lo`) changes between rounds. The first round of a scale solves
/// all of `moved`; the next ones only within `reach` in `graph` of the limits that changed side and of the
/// pieces that did not take a full step. A scale ends where a Newton step of each variable alone, from the rows
/// that hold it, would move none by SETTLE_M (`unsettled`), or after ROUNDS.
pub(crate) fn settle(
    terms: &[Term],
    limits: &Limits,
    z: &mut [f64],
    moved: &[usize],
    (graph, reach): (&Graph, f64),
    scales: &[f64],
) -> Settled {
    let (mut rounds, mut unconverged, mut capped) = (0, 0, Vec::new());
    let n = z.len();
    let mut inside = vec![false; n];
    moved.iter().for_each(|&v| inside[v] = true);
    // The rows of each term and of the limits, last, that hold each variable.
    let mut blocks: Vec<Block> = terms.iter().map(|t| t.block).collect();
    blocks.push(Block::Rows(limits.rows));
    let holders = Holders::new(&blocks, n);
    let last = terms.len() as u32;
    let excess = |i: usize, x: f64, lo: &[f64]| (x - limits.hi[i]).max(lo[i] - x);
    for &scale in scales {
        let region_of = |sources: Vec<usize>| {
            let distance = graph.distances(sources.into_iter().map(|v| (v, 0.0)), reach);
            (0..n).filter(|&v| inside[v] && distance[v].is_finite()).collect::<Vec<usize>>()
        };
        let mut region = region_of(unsettled(terms, limits, z, scale, moved));
        let mut step: f64 = 0.0;
        for _ in 0..ROUNDS {
            if region.is_empty() {
                break;
            }
            let held = holders.of(&region);
            let lo = (limits.lo)(z);
            // The limits that hold a variable of the region, their values, and those past their band.
            let near: Vec<usize> =
                held.iter().filter(|r| r.0 == last).map(|r| r.1 as usize).filter(|&i| limits.cost[i] > 0.0).collect();
            let x: Vec<f64> = near.iter().map(|&i| blocks[last as usize].at(i, z)).collect();
            // The model holds each limit past its band at z at the bound that it passed.
            let chosen: Vec<usize> = (0..near.len()).filter(|&j| excess(near[j], x[j], &lo) > 0.0).collect();
            let mut rows = Rows::default();
            chosen.iter().for_each(|&j| rows.push(limits.rows.row(near[j])));
            let bound: Vec<f64> = chosen
                .iter()
                .map(|&j| if x[j] > limits.hi[near[j]] { limits.hi[near[j]] } else { lo[near[j]] })
                .collect();
            let w: Vec<f64> = chosen.iter().map(|&j| scale * limits.cost[near[j]]).collect();
            let mut model: Vec<Term> = terms.iter().map(|t| Term { block: t.block, w: t.w, c: t.c }).collect();
            model.push(Term { block: Block::Rows(&rows), w: &w, c: &bound });
            let candidates =
                held.iter().copied().filter(|r| r.0 != last).chain((0..rows.len()).map(|j| (last, j as u32)));
            let (next, converged) = fit_rows(&model, n, Some((&*z, &region[..])), candidates);
            unconverged += usize::from(!converged);
            let d: Vec<f64> = next.iter().zip(z.iter()).map(|(next, z)| next - z).collect();
            let (piece, alphas) = steps(terms, limits, scale, (z, &d), (&near, &x, &lo), (&held, &region));
            step = 0.0;
            for &v in &region {
                z[v] += alphas[piece[v]] * d[v];
                step = step.max((alphas[piece[v]] * d[v]).abs());
            }
            rounds += 1;
            // The limits that changed side, by more than a tenth of SETTLE_M in or out, and the pieces that
            // stopped short start the next region. A lower bound that follows other heights may change too.
            let lo_now = (limits.lo)(z);
            let mut changed: Vec<usize> = (0..near.len())
                .filter(|&j| {
                    let (was, is) =
                        (excess(near[j], x[j], &lo), excess(near[j], blocks[last as usize].at(near[j], z), &lo_now));
                    (was > 0.0) != (is > 0.0) && (is - was).abs() > SETTLE_M / 10.0
                })
                .map(|j| near[j])
                .collect();
            changed.extend((0..lo.len()).filter(|&i| limits.cost[i] > 0.0 && lo_now[i] != lo[i]).filter(|&i| {
                let x = blocks[last as usize].at(i, z);
                (excess(i, x, &lo) > 0.0) != (excess(i, x, &lo_now) > 0.0)
            }));
            let mut sources: Vec<usize> = changed.iter().flat_map(|&i| limits.rows.row(i).map(|(v, _)| v)).collect();
            // A piece where the model's step does not fall at all is settled within the solve's precision.
            sources.extend(
                region
                    .iter()
                    .filter(|&&v| alphas[piece[v]] > 0.0 && ((1.0 - alphas[piece[v]]) * d[v]).abs() >= SETTLE_M),
            );
            // Where the regions settle, the rest of the network may not have: the next region is around the
            // heights that a step would still move.
            if sources.is_empty() {
                sources = unsettled(terms, limits, z, scale, moved);
            }
            region = region_of(sources);
        }
        capped.push((!region.is_empty()).then_some((step, region.len())));
    }
    Settled { rounds, unconverged, capped }
}

/// What `settle` did: its rounds, the refinements that did not converge, and per scale, where it ended after
/// ROUNDS and not at its stop rule, the largest move of its last round and the variables still unsettled. The
/// last move does not bound how far the heights are from the minimum: a capped job can lie metres from it.
pub(crate) struct Settled {
    pub rounds: usize,
    pub unconverged: usize,
    pub capped: Vec<Option<(f64, usize)>>,
}

/// The variables of `moved` that a Newton step of each alone would move by SETTLE_M or more, for a fall of the
/// cost of SETTLE_M² or more: the slope of the cost at it over its curvature, both from the rows that hold it.
fn unsettled(terms: &[Term], limits: &Limits, z: &[f64], scale: f64, moved: &[usize]) -> Vec<usize> {
    let n = z.len();
    let (mut slope, mut curve) = (vec![0.0; n], vec![0.0; n]);
    for term in terms {
        for i in 0..term.block.len() {
            let p = term.block.at(i, z) - term.c[i];
            term.block.each(i, |v, c| {
                slope[v] += term.w[i] * p * c;
                curve[v] += term.w[i] * c * c;
            });
        }
    }
    let (lo, x) = ((limits.lo)(z), limits.rows.times(z));
    for i in (0..x.len()).filter(|&i| limits.cost[i] > 0.0) {
        let beyond = past(x[i], lo[i], limits.hi[i]);
        if beyond != 0.0 {
            let w = scale * limits.cost[i];
            limits.rows.row(i).for_each(|(v, c)| {
                slope[v] += w * beyond * c;
                curve[v] += w * c * c;
            });
        }
    }
    let far = |v: usize| {
        curve[v] > 0.0 && slope[v].abs() >= SETTLE_M * curve[v] && slope[v].powi(2) >= SETTLE_M.powi(2) * curve[v]
    };
    moved.iter().copied().filter(|&v| far(v)).collect()
}

/// The pieces of `region` that the rows `held` tie, per variable, and per piece how far along z + α·d,
/// 0 ≤ α ≤ 1, its cost falls: to where its slope turns positive, or all the way where that falls enough. No row
/// ties two pieces, so each steps alone.
/// Per piece, the slope is linear in α for the least squares and grows with α for the limits `near`.
fn steps(
    terms: &[Term],
    limits: &Limits,
    scale: f64,
    (z, d): (&[f64], &[f64]),
    (near, x, lo): (&[usize], &[f64], &[f64]),
    (held, region): (&[(u32, u32)], &[usize]),
) -> (Vec<usize>, Vec<f64>) {
    let n = z.len();
    let mut slot = vec![usize::MAX; n];
    region.iter().enumerate().for_each(|(k, &v)| slot[v] = k);
    let mut parent: Vec<usize> = (0..region.len()).collect();
    let last = Block::Rows(limits.rows);
    let block = |k: u32| if (k as usize) < terms.len() { terms[k as usize].block } else { last };
    for &(k, i) in held {
        let mut first = None;
        block(k).each(i as usize, |v, _| {
            if slot[v] != usize::MAX {
                match first {
                    None => first = Some(slot[v]),
                    Some(f) => super::rows::union(&mut parent, f, slot[v]),
                }
            }
        });
    }
    let mut number = vec![usize::MAX; region.len()];
    let mut count = 0;
    let mut piece = vec![0; n];
    for (k, &v) in region.iter().enumerate() {
        let root = super::rows::root(&mut parent, k);
        if number[root] == usize::MAX {
            number[root] = count;
            count += 1;
        }
        piece[v] = number[root];
    }
    // The piece of a row: that of a variable of the region that it holds.
    let of = |block: Block, i: usize| {
        let mut found = 0;
        block.each(i, |v, _| {
            if slot[v] != usize::MAX {
                found = piece[v];
            }
        });
        found
    };
    let (mut s0, mut s1) = (vec![0.0; count], vec![0.0; count]);
    for &(k, i) in held.iter().filter(|r| (r.0 as usize) < terms.len()) {
        let (term, i) = (&terms[k as usize], i as usize);
        let q = term.block.at(i, d);
        if q != 0.0 {
            let p = term.block.at(i, z) - term.c[i];
            let k = of(term.block, i);
            s0[k] += term.w[i] * p * q;
            s1[k] += term.w[i] * q * q;
        }
    }
    let q: Vec<f64> = near.iter().map(|&i| last.at(i, d)).collect();
    let mut acting: Vec<Vec<usize>> = vec![Vec::new(); count];
    for j in (0..near.len()).filter(|&j| q[j] != 0.0) {
        acting[of(last, near[j])].push(j);
    }
    let beyond = |j: usize, alpha: f64| past(x[j] + alpha * q[j], lo[near[j]], limits.hi[near[j]]);
    let alphas = (0..count)
        .map(|k| {
            let slope = |alpha: f64| {
                let pull: f64 = acting[k].iter().map(|&j| limits.cost[near[j]] * beyond(j, alpha) * q[j]).sum();
                s0[k] + alpha * s1[k] + scale * pull
            };
            // The fall of the cost at 1, from the change of each row.
            let fall = {
                let change: f64 = acting[k]
                    .iter()
                    .map(|&j| limits.cost[near[j]] * (beyond(j, 1.0).powi(2) - beyond(j, 0.0).powi(2)))
                    .sum();
                s0[k] + s1[k] / 2.0 + scale * change / 2.0
            };
            // A step that saves less than moving a metre of way by SETTLE_M is within the solve's precision.
            if -slope(0.0) < SETTLE_M * SETTLE_M {
                return 0.0;
            }
            // A full step that falls by ARMIJO of its slope is taken whole: a step short of 1 makes more rounds.
            if slope(1.0) <= 0.0 || fall <= ARMIJO * slope(0.0) {
                return 1.0;
            }
            let (mut low, mut high) = (0.0, 1.0);
            for _ in 0..BISECTIONS {
                let mid = (low + high) / 2.0;
                if slope(mid) > 0.0 {
                    high = mid;
                } else {
                    low = mid;
                }
            }
            low
        })
        .collect();
    (piece, alphas)
}

/// The LDLᵀ factorization of the normal matrix Aᵀ·W·A, in a fill-reducing order.
struct Solver {
    symbolic: faer::sparse::linalg::cholesky::SymbolicCholesky<usize>,
    values: Vec<f64>,
    m: usize,
}

impl Solver {
    fn new(stack: &Stack) -> Solver {
        let m = stack.m;
        let (ptr, row, val) = lower_normal(stack);
        let symbolic = SymbolicSparseColMatRef::new_checked(m, m, &ptr, None, &row);
        let matrix = SparseColMatRef::new(symbolic, &val);
        let symbolic =
            factorize_symbolic_cholesky(symbolic, Side::Lower, SymmetricOrdering::Amd, Default::default()).unwrap();
        let mut values = vec![0.0; symbolic.len_val()];
        let mut work = MemBuffer::new(symbolic.factorize_numeric_ldlt_scratch::<f64>(Par::Seq, Default::default()));
        symbolic
            .factorize_numeric_ldlt(
                &mut values,
                matrix,
                Side::Lower,
                LdltRegularization::default(),
                Par::Seq,
                MemStack::new(&mut work),
                Default::default(),
            )
            .expect("the normal matrix factorizes");
        Solver { symbolic, values, m }
    }

    fn solve(&self, mut rhs: Vec<f64>) -> Vec<f64> {
        let ldlt = faer::sparse::linalg::cholesky::LdltRef::new(&self.symbolic, &self.values);
        let mut work = MemBuffer::new(self.symbolic.solve_in_place_scratch::<f64>(1, Par::Seq));
        let matrix = MatMut::from_column_major_slice_mut(&mut rhs, self.m, 1);
        ldlt.solve_in_place_with_conj(Conj::No, matrix, Par::Seq, MemStack::new(&mut work));
        rhs
    }
}

/// The lower triangle of Aᵀ·W·A in compressed columns, rows sorted (Gustavson's product).
fn lower_normal(stack: &Stack) -> (Vec<usize>, Vec<usize>, Vec<f64>) {
    let m = stack.m;
    // The columns of A: per variable, the rows that hold it, in order, a row once per entry.
    let mut count = vec![0usize; m + 1];
    for r in 0..stack.rows.len() {
        stack.each(r, |col, _| count[col + 1] += 1);
    }
    for j in 0..m {
        count[j + 1] += count[j];
    }
    let mut rows = vec![0u32; count[m]];
    let mut fill = count.clone();
    for r in 0..stack.rows.len() {
        stack.each(r, |col, _| {
            rows[fill[col]] = r as u32;
            fill[col] += 1;
        });
    }
    drop(fill);
    let (mut ptr, mut row, mut val) = (vec![0usize], Vec::new(), Vec::new());
    let mut sum = vec![0.0; m];
    let mut mark = vec![usize::MAX; m];
    let (mut found, mut entries) = (Vec::new(), Vec::with_capacity(4));
    for j in 0..m {
        found.clear();
        let mut last = None;
        for &r in &rows[count[j]..count[j + 1]] {
            if last.replace(r) == Some(r) {
                continue;
            }
            let (term, i) = stack.term(r as usize);
            entries.clear();
            stack.each(r as usize, |col, value| entries.push((col, value)));
            let arj: f64 = entries.iter().filter(|e| e.0 == j).map(|e| e.1).sum();
            for &(k, ark) in &entries {
                if k < j {
                    continue;
                }
                if mark[k] != j {
                    mark[k] = j;
                    sum[k] = 0.0;
                    found.push(k);
                }
                sum[k] += term.w[i] * arj * ark;
            }
        }
        found.sort_unstable();
        for &k in &found {
            row.push(k);
            val.push(sum[k]);
        }
        ptr.push(row.len());
    }
    (ptr, row, val)
}

fn norm(x: &[f64]) -> f64 {
    x.iter().map(|v| v * v).sum::<f64>().sqrt()
}

fn dot(x: &[f64], y: &[f64]) -> f64 {
    x.iter().zip(y).map(|(a, b)| a * b).sum()
}

/// GMRES: one cycle of at most `restart` steps from 0; and whether the residual ends within `atol`.
fn gmres(apply: &dyn Fn(&[f64]) -> Vec<f64>, b: &[f64], atol: f64, restart: usize) -> (Vec<f64>, bool) {
    let n = b.len();
    let bnorm = norm(b);
    if bnorm == 0.0 {
        return (b.to_vec(), true);
    }
    if bnorm < atol {
        return (vec![0.0; n], true);
    }
    let restart = restart.min(n);
    let ptol = bnorm.min(atol);
    let mut v: Vec<Vec<f64>> = vec![b.iter().map(|x| x * (1.0 / bnorm)).collect()];
    let mut h = vec![vec![0.0; restart + 1]; restart];
    let mut givens = vec![(0.0, 0.0); restart];
    let mut s = vec![0.0; restart + 1];
    s[0] = bnorm;
    let mut col = 0;
    loop {
        let mut w = apply(&v[col]);
        let h0 = norm(&w);
        for k in 0..=col {
            let t = dot(&v[k], &w);
            h[col][k] = t;
            for (wi, vi) in w.iter_mut().zip(&v[k]) {
                *wi -= t * vi;
            }
        }
        let h1 = norm(&w);
        h[col][col + 1] = h1;
        let breakdown = h1 <= f64::EPSILON * h0;
        if breakdown {
            h[col][col + 1] = 0.0;
        } else {
            w.iter_mut().for_each(|x| *x *= 1.0 / h1);
        }
        v.push(w);
        for k in 0..col {
            let (c, sn) = givens[k];
            let (n0, n1) = (h[col][k], h[col][k + 1]);
            h[col][k] = c * n0 + sn * n1;
            h[col][k + 1] = -sn * n0 + c * n1;
        }
        let (c, sn, r) = lartg(h[col][col], h[col][col + 1]);
        givens[col] = (c, sn);
        h[col][col] = r;
        h[col][col + 1] = 0.0;
        let t = -sn * s[col];
        s[col] *= c;
        s[col + 1] = t;
        if t.abs() <= ptol || breakdown || col + 1 == restart {
            break;
        }
        col += 1;
    }
    if h[col][col] == 0.0 {
        s[col] = 0.0;
    }
    let mut y = s[..=col].to_vec();
    for k in (1..=col).rev() {
        if y[k] != 0.0 {
            y[k] /= h[k][k];
            let t = y[k];
            for i in 0..k {
                y[i] -= t * h[k][i];
            }
        }
    }
    if y[0] != 0.0 {
        y[0] /= h[0][0];
    }
    let mut x = vec![0.0; n];
    for (k, yk) in y.iter().enumerate() {
        for (xi, vi) in x.iter_mut().zip(&v[k]) {
            *xi += yk * vi;
        }
    }
    let ax = apply(&x);
    let rnorm = norm(&b.iter().zip(&ax).map(|(b, a)| b - a).collect::<Vec<_>>());
    (x, rnorm <= atol)
}

/// A plane rotation (c, s, r) with c·f + s·g = r and −s·f + c·g = 0, as LAPACK's dlartg.
fn lartg(f: f64, g: f64) -> (f64, f64, f64) {
    if g == 0.0 {
        return (1.0, 0.0, f);
    }
    if f == 0.0 {
        return (0.0, g.signum(), g.abs());
    }
    // The entries of the Hessenberg matrix are near 1: f² + g² neither overflows nor underflows.
    let d = (f * f + g * g).sqrt();
    let r = d.copysign(f);
    (f.abs() / d, g / r, r)
}

/// An undirected graph of weighted edges.
pub(crate) struct Graph {
    ptr: Vec<usize>,
    to: Vec<usize>,
    weight: Vec<f64>,
}

impl Graph {
    /// The graph of n nodes and the edges (a, b, weight); edges between one pair add up.
    pub fn new(n: usize, edges: &[(usize, usize, f64)]) -> Graph {
        let mut pairs: Vec<(usize, usize, f64)> = edges.iter().map(|&(a, b, w)| (a.min(b), a.max(b), w)).collect();
        pairs.sort_by_key(|p| (p.0, p.1));
        let mut summed: Vec<(usize, usize, f64)> = Vec::with_capacity(pairs.len());
        for (a, b, w) in pairs {
            match summed.last_mut() {
                Some(last) if (last.0, last.1) == (a, b) => last.2 += w,
                _ => summed.push((a, b, w)),
            }
        }
        Graph::symmetric(n, &summed)
    }

    /// The graph of n nodes and the edges (a, b, weight), each pair once.
    pub fn symmetric(n: usize, edges: &[(usize, usize, f64)]) -> Graph {
        let mut ptr = vec![0usize; n + 1];
        for &(a, b, _) in edges {
            ptr[a + 1] += 1;
            ptr[b + 1] += 1;
        }
        for i in 0..n {
            ptr[i + 1] += ptr[i];
        }
        let mut fill = ptr.clone();
        let (mut to, mut weight) = (vec![0; ptr[n]], vec![0.0; ptr[n]]);
        for &(a, b, w) in edges {
            for (x, y) in [(a, b), (b, a)] {
                to[fill[x]] = y;
                weight[fill[x]] = w;
                fill[x] += 1;
            }
        }
        Graph { ptr, to, weight }
    }

    /// The distances from the nearest of the sources (node, its start distance), infinite past `limit`.
    pub fn distances(&self, sources: impl IntoIterator<Item = (usize, f64)>, limit: f64) -> Vec<f64> {
        let mut distance = vec![f64::INFINITY; self.ptr.len() - 1];
        // Distances are not negative, so their bits order as they do.
        let mut heap = BinaryHeap::new();
        for (node, d) in sources {
            if d <= limit && d < distance[node] {
                distance[node] = d;
                heap.push(Reverse((d.to_bits(), node)));
            }
        }
        while let Some(Reverse((bits, node))) = heap.pop() {
            let d = f64::from_bits(bits);
            if d > distance[node] {
                continue;
            }
            for e in self.ptr[node]..self.ptr[node + 1] {
                let next = d + self.weight[e];
                if next <= limit && next < distance[self.to[e]] {
                    distance[self.to[e]] = next;
                    heap.push(Reverse((next.to_bits(), self.to[e])));
                }
            }
        }
        distance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Heights 1 m apart pulled to a terrain with a spike of 20 m, grades of 10 % and a ground band of ±5 m: at the
    /// spike the grades and the band cannot both hold. From any start, the rounds end at one minimum, where the
    /// slope of the cost is 0 and a limit pushes only where its row lies past its band.
    #[test]
    fn settle_ends_at_the_minimum_of_the_cost_from_any_start() {
        let h = [0.0, 0.0, 0.0, 0.0, 20.0, 0.0, 0.0, 0.0, 0.0];
        let n = h.len();
        let mut pull = Rows::default();
        (0..n).for_each(|v| pull.push([(v, 1.0)]));
        let w = vec![1.0; n];
        let terms = [Term { block: Block::Rows(&pull), w: &w, c: &h }];
        let (mut rows, mut lo, mut hi, mut cost) = (Rows::default(), Vec::new(), Vec::new(), Vec::new());
        for v in 0..n - 1 {
            rows.push([(v, -1.0), (v + 1, 1.0)]);
            lo.push(-0.1);
            hi.push(0.1);
            cost.push(1e4);
        }
        for (v, &hv) in h.iter().enumerate() {
            rows.push([(v, 1.0)]);
            lo.push(hv - 5.0);
            hi.push(hv + 5.0);
            cost.push(1e2);
        }
        let lows = |_: &[f64]| lo.clone();
        let limits = Limits { rows: &rows, lo: &lows, hi: &hi, cost: &cost };
        let graph = Graph::new(n, &(0..n - 1).map(|v| (v, v + 1, 1.0)).collect::<Vec<_>>());
        let all: Vec<usize> = (0..n).collect();
        let mut ends = Vec::new();
        for start in [h.to_vec(), vec![-30.0; n], vec![50.0; n], vec![5.0; n]] {
            let mut z = start;
            settle(&terms, &limits, &mut z, &all, (&graph, 300.0), &[1e-2, 1.0]);
            let x = rows.times(&z);
            let mut slope: Vec<f64> = (0..n).map(|v| 2.0 * (z[v] - h[v])).collect();
            for i in 0..rows.len() {
                rows.row(i).for_each(|(v, c)| slope[v] += 2.0 * cost[i] * past(x[i], lo[i], hi[i]) * c);
            }
            assert!(slope.iter().all(|s| s.abs() < 1e-6), "{slope:?}");
            ends.push(z);
        }
        assert!(ends.iter().all(|z| z.iter().zip(&ends[0]).all(|(a, b)| (a - b).abs() < 1e-6)), "{ends:?}");
        // The band, which costs less, gives way to the grades: the spike passes it by more than a metre.
        let x = rows.times(&ends[0]);
        assert!(past(x[n - 1 + 4], lo[n - 1 + 4], hi[n - 1 + 4]) < -1.0);
    }
}
