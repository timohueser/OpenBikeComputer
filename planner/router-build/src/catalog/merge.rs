//! The merge of the routes of the origins: which jobs are pairs, which jobs and records leave, and
//! the records that take the place of a job that left. `specs/route-catalog.md#second-sources`
//! is the rule.
use super::dense;
use planner_router::shape::{distance, Near, P};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap, HashSet};

/// A pair by agreement needs this share of one match line along the other.
const AGREE_SHARE: f64 = 0.2;
/// A pair by the line alone needs this share both ways, and rule 1 this cover.
const SAME_SHARE: f64 = 0.8;
/// The side of the coarse cells that find the jobs whose lines can meet, in microdegrees: more
/// than `NEAR_M` in each direction.
const CELL_UDEG: i32 = 10_000;

/// A job as the merge compares it.
pub(super) struct Probe {
    /// The rank of its origin: the place of its second source in the file, and OSM last.
    pub origin: usize,
    pub family: &'static str,
    pub rank: u8,
    pub ref_: String,
    pub name: String,
    /// The pieces of the match line one after another, and the first point of each piece after the
    /// first: there is no line between two pieces.
    line: Vec<P>,
    breaks: Vec<usize>,
    /// The points of the match line at each 10 m, in the order of its pieces.
    samples: Vec<P>,
    /// The coarse cells of the samples.
    cells: HashSet<(i32, i32)>,
}

impl Probe {
    pub fn new(origin: usize, kind: &str, rank: u8, ref_: &str, name: &str, pieces: Vec<Vec<P>>) -> Self {
        let pieces = pieces.into_iter().filter(|piece| piece.len() >= 2);
        let (mut line, mut breaks) = (Vec::new(), Vec::new());
        for piece in pieces {
            if !line.is_empty() {
                breaks.push(line.len());
            }
            line.extend(piece);
        }
        let samples: Vec<P> = Self::runs(&line, &breaks).flat_map(dense).map(|(p, _)| p).collect();
        let cells = samples.iter().map(|p| (p[0].div_euclid(CELL_UDEG), p[1].div_euclid(CELL_UDEG))).collect();
        let family = if matches!(kind, "hiking" | "foot") {
            "hiking"
        } else if kind == "mtb" {
            "mtb"
        } else {
            "bicycle"
        };
        Probe { origin, family, rank, ref_: ref_.into(), name: name.into(), line, breaks, samples, cells }
    }

    fn runs<'a>(line: &'a [P], breaks: &'a [usize]) -> impl Iterator<Item = &'a [P]> {
        let starts = std::iter::once(0).chain(breaks.iter().copied());
        let ends = breaks.iter().copied().chain(std::iter::once(line.len()));
        starts.zip(ends).map(move |(a, b)| &line[a..b]).filter(|run| !run.is_empty())
    }

    /// The index of the match line.
    fn near(&self) -> Near<'_> {
        Near::pieces(&self.line, &self.breaks)
    }

    /// Whether a sample of one match line lies in a coarse cell of the other or next to one: else
    /// no point of one lies within `NEAR_M` of the other.
    fn meets(&self, other: &Probe) -> bool {
        let (small, large) = if self.cells.len() <= other.cells.len() { (self, other) } else { (other, self) };
        small
            .cells
            .iter()
            .any(|&(x, y)| (x - 1..=x + 1).any(|x| (y - 1..=y + 1).any(|y| large.cells.contains(&(x, y)))))
    }
}

/// The share of the points of a match line, at each 10 m, that lie within `NEAR_M` of a line of
/// `nears`.
pub(super) fn share(probe: &Probe, nears: &[&Near]) -> f64 {
    let near = probe.samples.iter().filter(|&&p| nears.iter().any(|n| n.near(p))).count();
    near as f64 / probe.samples.len().max(1) as f64
}

/// The numbers of a `ref`: its runs of digits and dots, each read as whole numbers between the
/// dots, so "4.1" is one number and agrees with neither 4 nor 1.
fn numbers(text: &str) -> Vec<Vec<u64>> {
    let runs = text.split(|c: char| !c.is_ascii_digit() && c != '.');
    let number =
        |run: &str| run.split('.').filter(|n| !n.is_empty()).map(|n| n.parse().ok()).collect::<Option<Vec<u64>>>();
    runs.filter_map(number).filter(|n| !n.is_empty()).collect()
}

/// The letters and digits of a text, in lower case.
fn plain(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

pub(super) fn refs_agree(a: &str, b: &str) -> bool {
    let (x, y) = (numbers(a), numbers(b));
    match x.is_empty() && y.is_empty() {
        true => plain(a) == plain(b),
        false => x.iter().any(|n| y.contains(n)),
    }
}

/// Whether two jobs agree: one kind family, the same `rank` when both have one, and refs that agree,
/// or, when one has no `ref`, names that agree.
pub(super) fn agree(a: &Probe, b: &Probe) -> bool {
    let words = match a.ref_.is_empty() || b.ref_.is_empty() {
        false => refs_agree(&a.ref_, &b.ref_),
        true => !plain(&a.name).is_empty() && plain(&a.name) == plain(&b.name),
    };
    a.family == b.family && (a.rank == 0 || b.rank == 0 || a.rank == b.rank) && words
}

/// How two jobs are a pair: (a) they agree, (b) they have the same line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Rule {
    A,
    B,
}

/// A pair of a job with a record: the record's id, the rule, the share of the job along the record
/// and the share of the record along the job.
#[derive(Clone, Copy, Debug)]
pub(super) struct Pair {
    pub id: i64,
    pub rule: Rule,
    pub shares: [f64; 2],
}

/// The pair of a job with a record, if they are one, from the index of each.
fn pair((job, near): (&Probe, &Near), (id, record, index): &(i64, &Probe, Near)) -> Option<Pair> {
    let shares = [share(job, &[index]), share(record, &[near])];
    let rule = match (agree(job, record), shares) {
        (true, [a, b]) if a >= AGREE_SHARE || b >= AGREE_SHARE => Rule::A,
        (_, [a, b]) if a >= SAME_SHARE && b >= SAME_SHARE => Rule::B,
        _ => return None,
    };
    Some(Pair { id: *id, rule, shares })
}

/// What the two rules decide.
#[derive(Default)]
pub(super) struct Merge {
    /// The pairs of each job with the kept records of earlier origins, as rule 1 found them.
    pub pairs: HashMap<i64, Vec<Pair>>,
    /// Rule 1: each job that left, with the cover of its pairs.
    pub replaced: BTreeMap<i64, f64>,
    /// Rule 2: each kept record that left, with the job that holds it.
    pub held: BTreeMap<i64, i64>,
}

impl Merge {
    /// Rule 1 for the jobs of each origin after the first, in rank order: a job leaves when the kept
    /// records of earlier origins that are its pairs cover four fifths of its match line. `kept`
    /// says whether a job of a second source kept a record.
    pub fn replace(probes: &BTreeMap<i64, Probe>, kept: &(dyn Fn(i64) -> bool + Sync)) -> Self {
        let mut merge = Merge::default();
        let last = probes.values().map(|probe| probe.origin).max().unwrap_or(0);
        for origin in 1..=last {
            let records: Vec<(i64, &Probe, Near)> = probes
                .iter()
                .filter(|(id, probe)| probe.origin < origin && kept(**id) && !merge.replaced.contains_key(id))
                .map(|(id, probe)| (*id, probe, probe.near()))
                .collect();
            let jobs: Vec<(&i64, &Probe)> = probes.iter().filter(|(_, probe)| probe.origin == origin).collect();
            let found: Vec<(i64, Vec<Pair>, f64)> = jobs
                .par_iter()
                .filter_map(|&(&id, job)| {
                    let meets = |record: &&(i64, &Probe, Near)| record.1.family == job.family && job.meets(record.1);
                    let candidates: Vec<_> = records.iter().filter(meets).collect();
                    if candidates.is_empty() {
                        return None;
                    }
                    let near = job.near();
                    let pairs: Vec<Pair> =
                        candidates.into_iter().filter_map(|record| pair((job, &near), record)).collect();
                    let covering: Vec<&Near> =
                        records.iter().filter(|record| pairs.iter().any(|p| p.id == record.0)).map(|r| &r.2).collect();
                    (!pairs.is_empty()).then(|| (id, pairs, share(job, &covering)))
                })
                .collect();
            for (id, pairs, cover) in found {
                if cover >= SAME_SHARE {
                    merge.replaced.insert(id, cover);
                }
                merge.pairs.insert(id, pairs);
            }
        }
        merge
    }

    /// Rule 2: a kept record leaves when one of its pairs is a job of a later origin that stays and
    /// holds four fifths of the record's match line. The job stays when it did not leave by rule 1
    /// and `stays` says that it is in the catalog.
    pub fn hold(&mut self, stays: &dyn Fn(i64) -> bool) {
        for (&job, pairs) in &self.pairs {
            if self.replaced.contains_key(&job) || !stays(job) {
                continue;
            }
            for pair in pairs.iter().filter(|pair| pair.shares[1] >= SAME_SHARE) {
                // Of two jobs that hold a record, the lower id, so the report does not depend on the
                // order of a hash map.
                self.held.entry(pair.id).and_modify(|holder| *holder = (*holder).min(job)).or_insert(job);
            }
        }
    }

    /// The successors of a job that left by rule 1, in the order of their starts along it: its pairs
    /// by (a) that rule 2 kept. A pair by (b) is another route on the same path. `starts` gives the
    /// first point of each kept record.
    pub fn successors(&self, job: i64, probes: &BTreeMap<i64, Probe>, starts: &dyn Fn(i64) -> Option<P>) -> Vec<i64> {
        let pairs = self.pairs.get(&job).into_iter().flatten();
        let pairs = pairs.filter(|pair| pair.rule == Rule::A && !self.held.contains_key(&pair.id));
        let probe = &probes[&job];
        let mut placed: Vec<(usize, i64)> =
            pairs.filter_map(|pair| Some((place(probe, starts(pair.id)?), pair.id))).collect();
        placed.sort();
        placed.into_iter().map(|(_, id)| id).collect()
    }
}

/// The job that gives each record the words that it lacks: of the jobs whose successor it is, the
/// one whose match line covers the largest share of the record's; of two, the smaller absolute `id`.
pub(super) fn donors(successors: &BTreeMap<i64, Vec<i64>>, probes: &BTreeMap<i64, Probe>) -> BTreeMap<i64, i64> {
    let mut best = BTreeMap::<i64, (f64, i64, i64)>::new();
    for (&job, records) in successors.iter().filter(|(_, records)| !records.is_empty()) {
        let near = probes[&job].near();
        for &record in records {
            let rank = (share(&probes[&record], &[&near]), -job.abs(), job);
            best.entry(record).and_modify(|best| *best = if rank > *best { rank } else { *best }).or_insert(rank);
        }
    }
    best.into_iter().map(|(record, (.., job))| (record, job)).collect()
}

/// The place along a match line of its point nearest to `p`: the sample in the order of the pieces,
/// which is the member order, and then along the piece.
fn place(probe: &Probe, p: P) -> usize {
    let distances = probe.samples.iter().map(|&q| distance(p, q));
    distances.enumerate().min_by(|a, b| a.1.total_cmp(&b.1)).map_or(0, |(k, _)| k)
}
