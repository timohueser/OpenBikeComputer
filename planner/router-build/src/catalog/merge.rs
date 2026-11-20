//! The merge of the routes of the origins: which jobs are pairs, which jobs and records leave, and
//! the records that take the place of a job that left. `specs/route-catalog.md#second-sources`
//! is the rule.
use super::{dense, sources::Sources, Reason};
use planner_router::{
    geometry::METRES_PER_UDEG,
    shape::{distance, Near, NEAR_M, P},
};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A pair by agreement needs this share of one match line along the other.
const AGREE_SHARE: f64 = 0.2;
/// A pair by the line alone needs this share both ways, and rule 1 this cover.
const SAME_SHARE: f64 = 0.8;

/// A job as the merge compares it.
pub(super) struct Probe {
    /// The rank of its origin: the place of its second source in the file, and OSM last.
    pub origin: usize,
    pub family: &'static str,
    pub rank: u8,
    pub ref_: String,
    pub name: String,
    /// The match line: pieces with no line between them.
    pub pieces: Vec<Vec<P>>,
    /// The points of the match line at each 10 m.
    samples: Vec<P>,
    bbox: [P; 2],
}

impl Probe {
    pub fn new(origin: usize, kind: &str, rank: u8, ref_: &str, name: &str, pieces: Vec<Vec<P>>) -> Self {
        let samples: Vec<P> = pieces.iter().flat_map(|piece| dense(piece)).map(|(p, _)| p).collect();
        let fold = |f: fn(i32, i32) -> i32, axis: usize, start: i32| samples.iter().fold(start, |m, p| f(m, p[axis]));
        let bbox = [
            [fold(i32::min, 0, i32::MAX), fold(i32::min, 1, i32::MAX)],
            [fold(i32::max, 0, i32::MIN), fold(i32::max, 1, i32::MIN)],
        ];
        let family = if matches!(kind, "hiking" | "foot") {
            "hiking"
        } else if kind == "mtb" {
            "mtb"
        } else {
            "bicycle"
        };
        Probe { origin, family, rank, ref_: ref_.into(), name: name.into(), pieces, samples, bbox }
    }

    /// Whether the boxes of two match lines lie within `NEAR_M` of each other.
    fn meets(&self, other: &Probe) -> bool {
        let lat = [self.bbox, other.bbox].iter().flatten().map(|p| (f64::from(p[1]) * 1e-6).abs()).fold(0.0, f64::max);
        let margin = [NEAR_M / METRES_PER_UDEG / lat.to_radians().cos().max(0.01), NEAR_M / METRES_PER_UDEG];
        let [a, b] = [self.bbox, other.bbox];
        (0..2).all(|axis| {
            let m = margin[axis].ceil() as i32;
            a[0][axis] <= b[1][axis] + m && b[0][axis] <= a[1][axis] + m
        })
    }
}

/// The share of the points of a match line, at each 10 m, that lie within `NEAR_M` of a piece of
/// `others`.
pub(super) fn share(probe: &Probe, others: &[&Probe]) -> f64 {
    let nears: Vec<Near> = others.iter().flat_map(|other| &other.pieces).map(|piece| Near::new(piece)).collect();
    let near = probe.samples.iter().filter(|&&p| nears.iter().any(|n| n.near(p))).count();
    near as f64 / probe.samples.len().max(1) as f64
}

/// The numbers of a `ref`: its runs of digits.
fn numbers(text: &str) -> Vec<u64> {
    text.split(|c: char| !c.is_ascii_digit()).filter_map(|run| run.parse().ok()).collect()
}

/// The letters and digits of a text, in lower case.
fn plain(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn refs_agree(a: &str, b: &str) -> bool {
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

/// The pair of two jobs, if they are one.
fn pair(job: &Probe, (id, record): (i64, &Probe)) -> Option<Pair> {
    if job.family != record.family || !job.meets(record) {
        return None;
    }
    let shares = [share(job, &[record]), share(record, &[job])];
    let rule = match (agree(job, record), shares) {
        (true, [a, b]) if a >= AGREE_SHARE || b >= AGREE_SHARE => Rule::A,
        (_, [a, b]) if a >= SAME_SHARE && b >= SAME_SHARE => Rule::B,
        _ => return None,
    };
    Some(Pair { id, rule, shares })
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
    pub fn replace(probes: &BTreeMap<i64, Probe>, kept: &dyn Fn(i64) -> bool) -> Self {
        let mut merge = Merge::default();
        let last = probes.values().map(|probe| probe.origin).max().unwrap_or(0);
        for origin in 1..=last {
            let records: Vec<(i64, &Probe)> = probes
                .iter()
                .filter(|(id, probe)| probe.origin < origin && kept(**id) && !merge.replaced.contains_key(id))
                .map(|(id, probe)| (*id, probe))
                .collect();
            for (&id, job) in probes.iter().filter(|(_, probe)| probe.origin == origin) {
                let pairs: Vec<Pair> = records.iter().filter_map(|&record| pair(job, record)).collect();
                if pairs.is_empty() {
                    continue;
                }
                let covering: Vec<&Probe> = pairs.iter().map(|pair| &probes[&pair.id]).collect();
                let cover = share(job, &covering);
                if cover >= SAME_SHARE {
                    merge.replaced.insert(id, cover);
                }
                merge.pairs.insert(id, pairs);
            }
        }
        merge
    }

    /// Rule 2: a kept record leaves when one of its pairs is a job of a later origin that stays:
    /// it did not leave by rule 1, and `stays` says that its plan kept a record or a section.
    pub fn hold(&mut self, stays: &dyn Fn(i64) -> bool) {
        for (&job, pairs) in &self.pairs {
            if self.replaced.contains_key(&job) || !stays(job) {
                continue;
            }
            for pair in pairs {
                // Of two jobs that hold a record, the lower id, so the report does not depend on the
                // order of a hash map.
                self.held.entry(pair.id).and_modify(|holder| *holder = (*holder).min(job)).or_insert(job);
            }
        }
    }

    /// The successors of a job that left by rule 1, in the order of their starts along it: its
    /// pairs by (a) that rule 2 kept, when they cover four fifths of its match line, else all its
    /// pairs that rule 2 kept. `starts` gives the first point of each kept record.
    pub fn successors(&self, job: i64, probes: &BTreeMap<i64, Probe>, starts: &dyn Fn(i64) -> Option<P>) -> Vec<i64> {
        let pairs = self.pairs.get(&job).into_iter().flatten().filter(|pair| !self.held.contains_key(&pair.id));
        let kept: Vec<&Pair> = pairs.filter(|pair| starts(pair.id).is_some()).collect();
        let agreeing: Vec<&Pair> = kept.iter().copied().filter(|pair| pair.rule == Rule::A).collect();
        let probe = &probes[&job];
        let covers = |pairs: &[&Pair]| {
            let records: Vec<&Probe> = pairs.iter().map(|pair| &probes[&pair.id]).collect();
            !pairs.is_empty() && share(probe, &records) >= SAME_SHARE
        };
        let chosen = if covers(&agreeing) { agreeing } else { kept };
        let mut placed: Vec<((usize, usize), i64)> =
            chosen.iter().map(|pair| (place(probe, starts(pair.id).expect("a kept record")), pair.id)).collect();
        placed.sort();
        placed.into_iter().map(|(_, id)| id).collect()
    }
}

/// The job that gives each record the words that it lacks: of the jobs whose successor it is, the
/// one whose match line covers the largest share of the record's; of two, the smaller absolute `id`.
pub(super) fn donors(successors: &BTreeMap<i64, Vec<i64>>, probes: &BTreeMap<i64, Probe>) -> BTreeMap<i64, i64> {
    let mut best = BTreeMap::<i64, (f64, i64, i64)>::new();
    for (&job, records) in successors {
        for &record in records {
            let rank = (share(&probes[&record], &[&probes[&job]]), -job.abs(), job);
            best.entry(record).and_modify(|best| *best = if rank > *best { rank } else { *best }).or_insert(rank);
        }
    }
    best.into_iter().map(|(record, (.., job))| (record, job)).collect()
}

/// The place along a match line of its point nearest to `p`: the piece in member order, then the
/// point along the piece.
fn place(probe: &Probe, p: P) -> (usize, usize) {
    let mut best = (f64::MAX, (0, 0));
    for (i, piece) in probe.pieces.iter().enumerate() {
        for (k, (q, _)) in dense(piece).into_iter().enumerate() {
            let d = distance(p, q);
            if d < best.0 {
                best = (d, (i, k));
            }
        }
    }
    best.1
}

/// What the report of the merge reads besides the merge.
pub(super) struct Facts<'a> {
    pub merge: &'a Merge,
    pub successors: &'a BTreeMap<i64, Vec<i64>>,
    pub drops: &'a BTreeMap<i64, Reason>,
    /// The records in the catalog.
    pub kept: &'a BTreeSet<i64>,
}

impl Facts<'_> {
    /// The report of each source by its id. `name` names a job by `[key, stage, name]`; `taken`
    /// counts the words that the records of each source took; `ascents` holds, for each kept
    /// record with a climb of its source, our climb over that one.
    pub fn report(
        &self,
        sources: &Sources,
        name: &dyn Fn(i64) -> Value,
        taken: &BTreeMap<(usize, &str), usize>,
        ascents: &[(usize, f64)],
    ) -> Value {
        let of: HashMap<i64, usize> = sources.jobs.iter().map(|job| (job.id(), job.source)).collect();
        let round = |share: f64| (share * 100.0).round() / 100.0;
        let rule = |rule: Rule| if rule == Rule::A { "a" } else { "b" };
        let mut jobs: Vec<(&i64, &Vec<Pair>)> = self.merge.pairs.iter().collect();
        jobs.sort_by_key(|(id, _)| **id);
        let mut report = Map::new();
        for (source, id) in sources.ids.iter().enumerate() {
            let mine = |id: &i64| of.get(id) == Some(&source);
            let mut dropped = BTreeMap::<String, usize>::new();
            let mut failed = Vec::new();
            for (id, reason) in self.drops.iter().filter(|(id, _)| mine(id)) {
                *dropped.entry(format!("{reason:?}")).or_default() += 1;
                if !matches!(reason, Reason::Replaced | Reason::Held) {
                    failed.push(json!([name(*id), format!("{reason:?}")]));
                }
            }
            let (mut replaced, mut uncertain, mut lost) = (Vec::new(), Vec::new(), Vec::new());
            for &(&job, pairs) in &jobs {
                let pairs: Vec<&Pair> = pairs.iter().filter(|pair| mine(&pair.id)).collect();
                for pair in pairs.iter().filter(|pair| pair.rule == Rule::A) {
                    if pair.shares.iter().all(|share| (AGREE_SHARE..SAME_SHARE).contains(share)) {
                        uncertain
                            .push(json!({"record": name(pair.id), "job": name(job), "shares": pair.shares.map(round)}));
                    }
                }
                let Some(cover) = self.merge.replaced.get(&job).filter(|_| !pairs.is_empty()) else { continue };
                let paired: Vec<Value> = pairs
                    .iter()
                    .map(|p| json!([name(p.id), rule(p.rule), round(p.shares[0]), round(p.shares[1])]))
                    .collect();
                let next: Vec<Value> = self.successors[&job].iter().map(|&id| name(id)).collect();
                replaced.push(json!({"job": name(job), "cover": round(*cover), "pairs": paired, "successors": next}));
                let gone: Vec<Value> =
                    pairs.iter().filter(|p| self.merge.held.contains_key(&p.id)).map(|p| name(p.id)).collect();
                if !gone.is_empty() {
                    lost.push(json!({"job": name(job), "for": gone}));
                }
            }
            let held: Vec<Value> = self
                .merge
                .held
                .iter()
                .filter(|(record, _)| mine(record))
                .map(|(&record, &job)| {
                    let pair = self.merge.pairs[&job].iter().find(|pair| pair.id == record).expect("a held record is a pair");
                    json!({"record": name(record), "by": name(job), "shares": [round(pair.shares[1]), round(pair.shares[0])]})
                })
                .collect();
            let words: BTreeMap<&str, usize> =
                taken.iter().filter(|((s, _), _)| *s == source).map(|((_, field), &n)| (*field, n)).collect();
            let mut ratios: Vec<f64> = ascents.iter().filter(|(s, _)| *s == source).map(|(_, r)| *r).collect();
            ratios.sort_by(f64::total_cmp);
            report.insert(
                id.clone(),
                json!({
                    "jobs": of.values().filter(|&&s| s == source).count(),
                    "kept": self.kept.iter().filter(|id| mine(id)).count(),
                    "failed": failed, "dropped": dropped, "replaced": replaced, "held": held,
                    "uncertain": uncertain, "words": words, "lost": lost,
                    "ascent_ratio": ratios.get(ratios.len() / 2).map(|r| round(*r)),
                }),
            );
        }
        Value::Object(report)
    }
}
