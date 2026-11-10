//! The sparse rows of the road solver, and the graph searches of its regions.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

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

/// An undirected graph of weighted edges.
pub(crate) struct Graph {
    ptr: Vec<usize>,
    to: Vec<usize>,
    weight: Vec<f64>,
}

impl Graph {
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
