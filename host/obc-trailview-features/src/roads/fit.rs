//! The least squares of the road solver, its sparse rows, and the graph searches of its regions.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use faer::dyn_stack::{MemBuffer, MemStack};
use faer::linalg::cholesky::ldlt::factor::LdltRegularization;
use faer::sparse::linalg::cholesky::{factorize_symbolic_cholesky, SymmetricOrdering};
use faer::sparse::{SparseColMatRef, SymbolicSparseColMatRef};
use faer::{Conj, MatMut, Par, Side};

use super::{REFINE_M, REFINE_STEPS};

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

    /// The rows that `keep` marks.
    pub fn select(&self, keep: &[bool]) -> Rows {
        let mut out = Rows::default();
        for i in (0..self.len()).filter(|&i| keep[i]) {
            out.push(self.row(i));
        }
        out
    }

    /// A · z.
    pub fn times(&self, z: &[f64]) -> Vec<f64> {
        (0..self.len()).map(|i| self.row(i).map(|(c, v)| v * z[c]).sum()).collect()
    }
}

/// The rows of a term: sparse rows, or a row (1 − t)·z_a + t·z_b per terrain sample on its segment.
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
    for (k, term) in terms.iter().enumerate() {
        for i in 0..term.block.len() {
            let mut moves = false;
            term.block.each(i, |col, _| moves |= local[col] != usize::MAX);
            if !moves {
                continue;
            }
            let mut target = term.c[i];
            if let Some((z, _)) = from {
                term.block.each(i, |col, val| target -= val * z[col]);
            }
            rows.push((k as u32, i as u32));
            c.push(target);
        }
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

/// GMRES as SciPy's `gmres(A, b, rtol=0, atol=atol, restart=restart, maxiter=1)`: one cycle of at most
/// `restart` steps from 0; and whether the residual ends within `atol`.
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
