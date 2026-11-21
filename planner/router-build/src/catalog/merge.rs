//! The merge of the routes of the origins: which jobs are pairs, which jobs leave, and the records
//! that take the place of a job that left. `specs/route-catalog.md#second-sources` is the rule.
use super::dense;
use planner_router::shape::{distance, Near, P};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// A pair by agreement needs this share of one match line along the other.
const AGREE_SHARE: f64 = 0.2;
/// A pair by the line alone needs this share both ways, and a job that leaves this cover.
const SAME_SHARE: f64 = 0.8;
/// A pair by the words of the names needs `SAME_SHARE` of one match line along the other, and this
/// share of the other along the first.
const WORDS_SHARE: f64 = 0.5;
/// A job whose counted pairs all agree with it by name leaves at this cover: where the names agree
/// and the lines differ in part, the official route shows.
const NAME_COVER: f64 = 0.5;
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
    /// The names that its pairs compare, each with a word.
    names: Vec<Name>,
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
    pub fn new(origin: usize, kind: &str, rank: u8, ref_: &str, names: &[&str], pieces: Vec<Vec<P>>) -> Self {
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
        let names = names.iter().filter_map(|name| Name::new(name)).collect();
        Probe { origin, family, rank, ref_: ref_.into(), names, line, breaks, samples, cells }
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

/// How two jobs or two names agree, from no to the most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Agree {
    No,
    /// Only by the words of their names.
    Words,
    /// By their refs, or by names with equal letters and digits.
    Same,
}

/// A name as a pair compares it: its letters and digits, and its words, in lower case. A word is a
/// run of letters and digits.
struct Name {
    plain: String,
    words: BTreeSet<String>,
}

impl Name {
    /// `None` for a name with no word, which agrees with no name.
    fn new(text: &str) -> Option<Self> {
        let words: BTreeSet<String> =
            text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(plain).collect();
        (!words.is_empty()).then(|| Name { plain: plain(text), words })
    }

    /// The words with an ASCII digit.
    fn digits(&self) -> impl Iterator<Item = &String> {
        self.words.iter().filter(|w| w.bytes().any(|b| b.is_ascii_digit()))
    }

    /// `Same` for equal letters and digits; `Words` when the words of one are all words of the other
    /// and both have the same digit words.
    fn agree(&self, other: &Name) -> Agree {
        let within = self.words.is_subset(&other.words) || other.words.is_subset(&self.words);
        if self.plain == other.plain {
            Agree::Same
        } else if within && self.digits().eq(other.digits()) {
            Agree::Words
        } else {
            Agree::No
        }
    }
}

/// Whether two names agree, by either rule.
#[cfg(test)]
pub(super) fn names_agree(a: &str, b: &str) -> bool {
    Name::new(a).zip(Name::new(b)).is_some_and(|(a, b)| a.agree(&b) != Agree::No)
}

/// How the names of two jobs agree: the most of each name of one against each name of the other.
fn names(a: &Probe, b: &Probe) -> Agree {
    let each = a.names.iter().flat_map(|x| b.names.iter().map(|y| x.agree(y)));
    each.max().unwrap_or(Agree::No)
}

/// How two jobs agree: not at all when their kind families differ, or both have a `rank` and the
/// ranks differ; else by their refs when both have one, or by their names.
fn agree(a: &Probe, b: &Probe) -> Agree {
    if a.family != b.family || (a.rank != 0 && b.rank != 0 && a.rank != b.rank) {
        return Agree::No;
    }
    match a.ref_.is_empty() || b.ref_.is_empty() {
        false if refs_agree(&a.ref_, &b.ref_) => Agree::Same,
        false => Agree::No,
        true => names(a, b),
    }
}

/// Whether two jobs that agree so are a pair by (a) at their shares, each of one match line along
/// the other.
fn close(agree: Agree, [a, b]: [f64; 2]) -> bool {
    match agree {
        Agree::Same => a >= AGREE_SHARE || b >= AGREE_SHARE,
        Agree::Words => a.max(b) >= SAME_SHARE && a.min(b) >= WORDS_SHARE,
        Agree::No => false,
    }
}

/// How two jobs are a pair: (a) they agree, (b) they have the same line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Rule {
    A,
    B,
}

/// A pair of a job with a record: the record's id, the rule, and whether it is a pair by (a) whose
/// names agree at its shares, whatever the refs.
#[derive(Clone, Copy, Debug)]
pub(super) struct Pair {
    pub id: i64,
    pub rule: Rule,
    named: bool,
}

/// The pair of a job with a record, if they are one, from the index of each.
fn pair((job, near): (&Probe, &Near), (id, record, index): &(i64, &Probe, Near)) -> Option<Pair> {
    let shares = [share(job, &[index]), share(record, &[near])];
    let rule = match close(agree(job, record), shares) {
        true => Rule::A,
        false if shares.iter().all(|&s| s >= SAME_SHARE) => Rule::B,
        false => return None,
    };
    let named = rule == Rule::A && close(names(job, record), shares);
    Some(Pair { id: *id, rule, named })
}

/// What the merge decides.
#[derive(Default)]
pub(super) struct Merge {
    /// The pairs of each job with the kept records of earlier origins.
    pub pairs: HashMap<i64, Vec<Pair>>,
    /// Each job that left.
    pub replaced: BTreeSet<i64>,
}

impl Merge {
    /// The merge of the jobs of each origin after the first, in rank order: a job leaves when the kept
    /// records of earlier origins that are its pairs cover four fifths of its match line, or a half
    /// when they are all pairs by (a) whose names agree with it; for a part of a long relation
    /// (`part`), only its pairs by (a) count. `kept` says whether a job of a second source kept a
    /// record.
    pub fn replace(
        probes: &BTreeMap<i64, Probe>,
        kept: &(dyn Fn(i64) -> bool + Sync),
        part: &(dyn Fn(i64) -> bool + Sync),
    ) -> Self {
        let mut merge = Merge::default();
        let last = probes.values().map(|probe| probe.origin).max().unwrap_or(0);
        for origin in 1..=last {
            let records: Vec<(i64, &Probe, Near)> = probes
                .iter()
                .filter(|(id, probe)| probe.origin < origin && kept(**id) && !merge.replaced.contains(id))
                .map(|(id, probe)| (*id, probe, probe.near()))
                .collect();
            let jobs: Vec<(&i64, &Probe)> = probes.iter().filter(|(_, probe)| probe.origin == origin).collect();
            let found: Vec<(i64, Vec<Pair>, bool)> = jobs
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
                    let counted: Vec<&Pair> = pairs.iter().filter(|p| p.rule == Rule::A || !part(id)).collect();
                    let covering: Vec<&Near> = records
                        .iter()
                        .filter(|record| counted.iter().any(|p| p.id == record.0))
                        .map(|r| &r.2)
                        .collect();
                    let limit = if counted.iter().all(|p| p.named) { NAME_COVER } else { SAME_SHARE };
                    (!pairs.is_empty()).then(|| (id, pairs, share(job, &covering) >= limit))
                })
                .collect();
            for (id, pairs, leaves) in found {
                if leaves {
                    merge.replaced.insert(id);
                }
                merge.pairs.insert(id, pairs);
            }
        }
        merge
    }

    /// The successors of a job that left, in the order of their starts along it: its pairs by (a). A
    /// pair by (b) is another route on the same path. `starts` gives the first point of each kept
    /// record.
    pub fn successors(&self, job: i64, probes: &BTreeMap<i64, Probe>, starts: &dyn Fn(i64) -> Option<P>) -> Vec<i64> {
        let pairs = self.pairs.get(&job).into_iter().flatten();
        let pairs = pairs.filter(|pair| pair.rule == Rule::A);
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

/// The record that takes the place of a failed official stage: of the `candidates` that agree with the stage and whose
/// match line and the stage's each lie at least four fifths along the other, the one with the lower `id`.
pub(super) fn fill<'a>(stage: &Probe, candidates: impl IntoIterator<Item = (i64, &'a Probe)>) -> Option<i64> {
    let near = stage.near();
    let both = |probe: &Probe| share(probe, &[&near]) >= SAME_SHARE && share(stage, &[&probe.near()]) >= SAME_SHARE;
    let agrees = |probe: &Probe| agree(stage, probe) != Agree::No;
    let fills = candidates.into_iter().filter(|(_, probe)| agrees(probe) && stage.meets(probe) && both(probe));
    fills.map(|(id, _)| id).min()
}

/// The place along a match line of its point nearest to `p`: the sample in the order of the pieces,
/// which is the member order, and then along the piece.
fn place(probe: &Probe, p: P) -> usize {
    let distances = probe.samples.iter().map(|&q| distance(p, q));
    distances.enumerate().min_by(|a, b| a.1.total_cmp(&b.1)).map_or(0, |(k, _)| k)
}
