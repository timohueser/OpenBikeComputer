use crate::model::Cost;
use crate::storage::{Cache, EdgeRef, Seed, NODES_PER_PAGE};
use serde::Serialize;
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap, VecDeque};

#[derive(Clone, Copy)]
struct Label {
    cost: Cost,
    parent: Option<(u32, EdgeRef)>,
    road: u32,
}

#[derive(Default)]
struct Frontier {
    heap: BinaryHeap<Reverse<(Cost, u32)>>,
    labels: HashMap<u32, Label>,
}

impl Frontier {
    fn new(seeds: &[Seed]) -> Self {
        let mut f = Self::default();
        for s in seeds {
            if f.labels.get(&s.node).is_none_or(|old| s.cost < old.cost) {
                f.labels.insert(s.node, Label { cost: s.cost, parent: None, road: s.road });
                f.heap.push(Reverse((s.cost, s.node)));
            }
        }
        f
    }

    fn peek(&mut self) -> Option<(Cost, u32)> {
        while let Some(&Reverse((cost, node))) = self.heap.peek() {
            if self.labels[&node].cost == cost {
                return Some((cost, node));
            }
            self.heap.pop();
        }
        None
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Progress {
    Working { labels: usize },
    NeedPages { pages: Vec<u32> },
    Done { cost: Cost, roads: Vec<u32>, labels: usize },
    NoPath,
    Limit,
    Cancelled,
    Invalid { message: String },
}

/// The host supplies immutable pages and calls poll between UI events or from a worker.
/// The label budget also bounds unpacked roads and the shortcut stack.
pub struct Search {
    frontiers: [Frontier; 2],
    best: Cost,
    meeting: Option<u32>,
    unpack: Option<Vec<EdgeRef>>,
    roads: Vec<u32>,
    next_side: usize,
    max_labels: usize,
    cancelled: bool,
    terminal: Option<Progress>,
}

impl Search {
    pub fn new(starts: &[Seed], ends: &[Seed], max_labels: usize) -> Self {
        Self {
            frontiers: [Frontier::new(starts), Frontier::new(ends)],
            best: Cost::MAX,
            meeting: None,
            unpack: None,
            roads: Vec::new(),
            next_side: 0,
            max_labels,
            cancelled: false,
            terminal: None,
        }
    }

    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    fn labels(&self) -> usize {
        self.frontiers.iter().map(|f| f.labels.len()).sum()
    }

    fn needed(&self, cache: &Cache, required: u32) -> Progress {
        if cache.pages.contains_key(&(required / NODES_PER_PAGE)) {
            return Progress::Invalid { message: "Loaded page does not contain the requested node".into() };
        }
        let mut pages = BTreeSet::from([required / NODES_PER_PAGE]);
        if let Some(stack) = &self.unpack {
            // Inspect cached descendants across the selected path, not just its next leaf.
            // Bound planning to one normal poll's work and at most 64 page requests.
            let work = self.max_labels.min(4096);
            let mut pending: VecDeque<_> = stack.iter().rev().take(work).copied().collect();
            for _ in 0..work {
                let Some(edge) = pending.pop_front() else { break };
                let page = edge.node / NODES_PER_PAGE;
                if !cache.pages.contains_key(&page) {
                    pages.insert(page);
                    if pages.len() >= 64 {
                        break;
                    }
                    continue;
                }
                let Some(arc) = cache.arc(edge) else {
                    return Progress::Invalid { message: "Shortcut references an absent edge in a loaded page".into() };
                };
                if let Some([first, second]) = arc.children {
                    if first.node >= edge.node || second.node >= edge.node {
                        return Progress::Invalid { message: "Invalid shortcut rank".into() };
                    }
                    pending.extend([first, second]);
                }
            }
        } else {
            // Search-stage speculation stays bounded separately from path reconstruction.
            for f in &self.frontiers {
                for &Reverse((_, node)) in f.heap.iter().take(64) {
                    if !cache.pages.contains_key(&(node / NODES_PER_PAGE)) && pages.len() < 16 {
                        pages.insert(node / NODES_PER_PAGE);
                    }
                }
            }
        }
        Progress::NeedPages { pages: pages.into_iter().collect() }
    }

    pub fn poll(&mut self, cache: &Cache, work: usize) -> Progress {
        if let Some(result) = &self.terminal {
            return result.clone();
        }
        let result = if self.cancelled { Progress::Cancelled } else { self.advance(cache, work) };
        if matches!(
            result,
            Progress::Done { .. } | Progress::NoPath | Progress::Limit | Progress::Cancelled | Progress::Invalid { .. }
        ) {
            self.terminal = Some(result.clone());
        }
        result
    }

    fn advance(&mut self, cache: &Cache, work: usize) -> Progress {
        for _ in 0..work {
            if self.cancelled {
                return Progress::Cancelled;
            }
            if self.labels() > self.max_labels
                || self.frontiers.iter().map(|f| f.heap.len()).sum::<usize>() > self.max_labels.saturating_mul(4)
            {
                return Progress::Limit;
            }
            if let Some(stack) = &mut self.unpack {
                if let Some(edge) = stack.last().copied() {
                    let Some(arc) = cache.arc(edge) else {
                        return if cache.node(edge.node).is_some() {
                            Progress::Invalid { message: "Shortcut references an absent edge".into() }
                        } else {
                            self.needed(cache, edge.node)
                        };
                    };
                    if stack.len() > self.max_labels {
                        return Progress::Limit;
                    }
                    stack.pop();
                    if let Some([first, second]) = arc.children {
                        // Both children descend to a lower-rank node. This bounds recursion.
                        if first.node >= edge.node || second.node >= edge.node {
                            return Progress::Invalid { message: "Invalid shortcut rank".into() };
                        }
                        if stack.len().saturating_add(2) > self.max_labels {
                            return Progress::Limit;
                        }
                        stack.push(second);
                        stack.push(first);
                    } else {
                        if self.roads.len() >= self.max_labels {
                            return Progress::Limit;
                        }
                        self.roads.push(arc.road);
                    }
                    continue;
                }
                return Progress::Done { cost: self.best, roads: self.roads.clone(), labels: self.labels() };
            }

            let tops = [self.frontiers[0].peek(), self.frontiers[1].peek()];
            if tops.iter().all(|top| top.is_none_or(|(cost, _)| cost >= self.best)) {
                let Some(meet) = self.meeting else {
                    return Progress::NoPath;
                };
                let mut edges = Vec::new();
                let mut node = meet;
                while let Some((parent, edge)) = self.frontiers[0].labels[&node].parent {
                    edges.push(edge);
                    node = parent;
                }
                self.roads.push(self.frontiers[0].labels[&node].road);
                edges.reverse();
                node = meet;
                while let Some((parent, edge)) = self.frontiers[1].labels[&node].parent {
                    edges.push(edge);
                    node = parent;
                }
                edges.reverse();
                self.unpack = Some(edges);
                continue;
            }
            let mut side = self.next_side;
            if tops[side].is_none_or(|(cost, _)| cost >= self.best) {
                side = 1 - side;
            }
            self.next_side = 1 - side;
            let (cost, node_id) = tops[side].unwrap();
            let Some(node) = cache.node(node_id) else {
                return self.needed(cache, node_id);
            };
            self.frontiers[side].heap.pop();
            if let Some(other) = self.frontiers[1 - side].labels.get(&node_id) {
                let Some(joined) = cost.checked_add(other.cost).filter(|&c| c != Cost::MAX) else {
                    return Progress::Invalid { message: "Route cost overflow".into() };
                };
                if joined < self.best {
                    self.best = joined;
                    self.meeting = Some(node_id);
                }
            }
            let arcs = if side == 0 { &node.forward } else { &node.backward };
            let other_labels = self.frontiers[1 - side].labels.len();
            let other_heap = self.frontiers[1 - side].heap.len();
            let frontier = &mut self.frontiers[side];
            for (index, arc) in arcs.iter().enumerate() {
                if arc.to <= node_id || arc.cost == 0 {
                    return Progress::Invalid {
                        message: "Summary arcs must rise in rank and have positive cost".into(),
                    };
                }
                let Some(next) = cost.checked_add(arc.cost).filter(|&c| c != Cost::MAX) else {
                    return Progress::Invalid { message: "Route cost overflow".into() };
                };
                if next < self.best && frontier.labels.get(&arc.to).is_none_or(|l| next < l.cost) {
                    if (!frontier.labels.contains_key(&arc.to)
                        && frontier.labels.len() + other_labels >= self.max_labels)
                        || frontier.heap.len() + other_heap >= self.max_labels.saturating_mul(4)
                    {
                        return Progress::Limit;
                    }
                    frontier.labels.insert(
                        arc.to,
                        Label {
                            cost: next,
                            parent: Some((
                                node_id,
                                EdgeRef { node: node_id, index: index as u32, backward: side == 1 },
                            )),
                            road: 0,
                        },
                    );
                    frontier.heap.push(Reverse((next, arc.to)));
                }
            }
        }
        Progress::Working { labels: self.labels() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{encode, Arc, Node, Page};

    fn cached(nodes: Vec<Node>) -> Cache {
        let mut cache = Cache::default();
        cache.insert_page(0, &encode(&Page { first: 0, nodes }).unwrap(), usize::MAX).unwrap();
        cache
    }

    fn seed(node: u32) -> Seed {
        Seed { node, cost: 0, road: node }
    }

    fn leaf(to: u32, road: u32) -> Arc {
        Arc { to, cost: 1, children: None, road }
    }

    #[test]
    fn missing_pages_resume_but_incomplete_loaded_pages_are_invalid() {
        let mut search = Search::new(&[seed(0)], &[seed(1)], 3);
        let cache = Cache::default();
        assert!(matches!(search.poll(&cache, 10), Progress::NeedPages { pages } if pages == [0]));
        let cache = cached(vec![Node { forward: vec![leaf(1, 1)], backward: vec![] }, Node::default()]);
        assert!(matches!(search.poll(&cache, 20), Progress::Done { cost: 1, roads, .. } if roads == [0, 1]));
        let mut malformed = Search::new(&[seed(1)], &[seed(2)], 3);
        let short_page = cached(vec![Node::default()]);
        assert!(matches!(malformed.poll(&short_page, 10), Progress::Invalid { .. }));
        assert!(matches!(malformed.poll(&cache, 10), Progress::Invalid { .. }));
    }

    #[test]
    fn unpack_prefetch_exposes_independent_descendants_without_mutating_the_path() {
        let edge = |page, offset| EdgeRef { node: page * NODES_PER_PAGE + offset, index: 0, backward: false };
        let branch = |children| Node {
            forward: vec![Arc { to: 2000, cost: 2, children: Some(children), road: 0 }],
            backward: vec![],
        };
        let mut cache = Cache::default();
        cache
            .insert_page(
                10,
                &encode(&Page {
                    first: 10 * NODES_PER_PAGE,
                    nodes: vec![
                        branch([edge(1, 0), edge(2, 0)]),
                        branch([edge(3, 0), edge(4, 0)]),
                        branch([edge(10, 0), edge(10, 1)]),
                    ],
                })
                .unwrap(),
                usize::MAX,
            )
            .unwrap();
        let mut search = Search::new(&[seed(99 * NODES_PER_PAGE)], &[], 100);
        search.unpack = Some(vec![edge(10, 2), edge(0, 0)]);
        assert!(matches!(search.needed(&cache, 0), Progress::NeedPages { pages } if pages == [0, 1, 2, 3, 4]));
        assert_eq!(search.unpack.as_ref().unwrap().len(), 2);
        assert!(search.roads.is_empty());
        search.unpack = Some((0..100).map(|page| edge(page, 0)).collect());
        assert!(matches!(search.needed(&Cache::default(), 0), Progress::NeedPages { pages } if pages.len() == 64));
        cache.pages.get_mut(&10).unwrap().nodes[2] = branch([edge(10, 2), edge(1, 0)]);
        search.unpack = Some(vec![edge(10, 2), edge(0, 0)]);
        assert!(matches!(search.needed(&cache, 0), Progress::Invalid { .. }));
    }

    #[test]
    fn cancellation_and_label_budget_are_terminal() {
        let cache = cached(vec![Node { forward: vec![leaf(1, 1)], backward: vec![] }, Node::default()]);
        let mut cancelled = Search::new(&[seed(0)], &[seed(1)], 3);
        cancelled.cancel();
        assert!(matches!(cancelled.poll(&cache, 0), Progress::Cancelled));
        assert!(matches!(cancelled.poll(&cache, 10), Progress::Cancelled));
        let mut limited = Search::new(&[seed(0)], &[seed(1)], 2);
        assert!(matches!(limited.poll(&cache, 10), Progress::Limit));
        assert_eq!(limited.labels(), 2);
        let mut enough = Search::new(&[seed(0)], &[seed(1)], 3);
        assert!(matches!(enough.poll(&cache, 20), Progress::Done { .. }));
    }

    #[test]
    fn shortcut_unpack_yields_and_rejects_cycles_and_excess_output() {
        let child = EdgeRef { node: 0, index: 0, backward: false };
        let shortcut = EdgeRef { node: 1, index: 0, backward: false };
        let cache = cached(vec![
            Node { forward: vec![leaf(2, 7)], backward: vec![] },
            Node { forward: vec![Arc { to: 2, cost: 2, children: Some([child, child]), road: 0 }], backward: vec![] },
        ]);
        let mut search = Search::new(&[], &[], 2);
        search.unpack = Some(vec![shortcut]);
        search.best = 2;
        for _ in 0..3 {
            assert!(matches!(search.poll(&cache, 1), Progress::Working { .. }));
        }
        assert!(matches!(search.poll(&cache, 1), Progress::Done { roads, .. } if roads == [7, 7]));
        let mut limited = Search::new(&[], &[], 2);
        limited.unpack = Some(vec![shortcut]);
        limited.roads.push(9);
        assert!(matches!(limited.poll(&cache, 10), Progress::Limit));
        let cycle = cached(vec![Node {
            forward: vec![Arc { to: 1, cost: 2, children: Some([child, child]), road: 0 }],
            backward: vec![],
        }]);
        let mut invalid = Search::new(&[], &[], 10);
        invalid.unpack = Some(vec![child]);
        assert!(matches!(invalid.poll(&cycle, 10), Progress::Invalid { .. }));
        let mut absent = Search::new(&[], &[], 10);
        absent.unpack = Some(vec![EdgeRef { index: 1, ..child }]);
        assert!(matches!(absent.poll(&cache, 10), Progress::Invalid { .. }));
    }
}
