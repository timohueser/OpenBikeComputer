//! Retention: deleting the generation the manifest no longer names (WXR8 #1247).
//!
//! Until this module, the baker **provably never deleted anything** — three store methods, none of
//! them destructive — and that was worth a lot: no bug anywhere in it could destroy published data.
//! This is the module that gives that up, so every line of it is written to give up as little as
//! possible.
//!
//! ## Why the baker deletes at all
//!
//! R2 lifecycle rules cannot express short retention: the granularity is whole days, and expiry is
//! lazy ("typically removed within 24 hours of the expiration value"), so the shortest rule the
//! setting can state really means objects live up to ~2 days. That was never a retention decision;
//! it was the shortest thing the control offered. A reader always wants the newest generation, so
//! keeping a day of them is 96 full object sets to serve three.
//!
//! Retention therefore moves here: **after a successful publish, delete the generations the new
//! manifest no longer names**, which in steady state is exactly generation N-3. Resident storage
//! drops from ~3-24 GB to tens of MB. The 1-day lifecycle rule stays, demoted to what it is good
//! at: a backstop that eventually collects objects a *crashed* cycle leaked, which no sweep driven
//! by manifests can ever see.
//!
//! ## The four constraints, and where each one lives
//!
//! 1. **Never delete an object the just-published manifest references.** [`delete_set`] subtracts
//!    the published document's own `generation` and `previous_generations` from the candidates, and
//!    the candidates are generation *identifiers a manifest named* — never a timestamp, never a
//!    listing, never a prefix guess. There is deliberately no `list` on the store seam, because
//!    listing is the operation most likely to turn into a prefix-guess bug.
//! 2. **Delete only after the swap.** [`crate::canonical::run_cycle`] calls [`sweep`] after
//!    `store.put(manifest)` returns `Ok`, and nowhere else.
//! 3. **A failed delete is a warning, not a failed cycle.** [`SweepReport::warnings`] lands on
//!    `CycleReport::warnings`; the objects are already unreferenced, so a leak is cosmetic and the
//!    lifecycle backstop is what makes it self-healing.
//! 4. **The chain must have been carried, not invented** (`OBCG_Spec.md` §10.4). The sweep takes a
//!    [`Carried`], which only [`crate::manifest_v2::carried_generations`] can build and which is
//!    `had_predecessor: false` for anything it could not read as one of our manifests. A torn read
//!    never reaches here at all — it fails the cycle before a single object is published — so the
//!    check below is the invariant restated where the deletion happens rather than a second gate.
//!
//! ## What it costs
//!
//! `DeleteObject` is **free** on R2 — neither Class A nor Class B — so the number of objects a
//! sweep deletes is not a budget question at all. What it costs is one request per key: a full
//! generation is `shard_count x frames` = 216 keys. Until #1279 each of those was an `rclone`
//! process spawn at ~0.2 s, so ~45 s of wall-clock; they are now requests on the connection pool
//! the publish phase already warmed, which is round-trip time and little else. Either way it is
//! paid after the manifest is in place, with nothing waiting on it, on a 15-minute cadence, and it
//! is still not worth a `list` (which would need a new store capability) or a `head` per key
//! (which would double it) to shave further.
//!
//! It does pay for **one** extra read per retired generation: [`canary`], a bounded sample taken
//! *before* the deletes. That is not a census and is not on the way to becoming one — it exists
//! because S3's idempotent delete cannot say whether a key was there, so "this generation was
//! already empty", the signal that something else is sweeping this prefix, has to be asked for
//! while asking still means something.
//!
//! ## What it deliberately does not collect
//!
//! Keys are enumerated from the **current** lattice's shard grid, because that is the only grid
//! this process knows. Re-cutting the grid therefore strands the old generations' out-of-grid
//! objects, and a cycle that died between publishing objects and swapping the manifest strands all
//! of its own. Both are exactly what the 1-day lifecycle rule is still there for. Trying to be
//! cleverer here means listing the bucket, and listing the bucket is how a sweep learns to delete
//! something no manifest ever named.

use std::collections::BTreeSet;

use crate::canonical::{CycleTimes, Lattice};
use crate::manifest_v2::{self, Carried, Manifest};
use crate::publish::ObjectStore;

/// The most generations one cycle will retire, and it is a refusal rather than a truncation.
///
/// `manifest_v2::carried_generations` enforces §10.4's cap on the way in, so a delete set larger
/// than this is unreachable from any document this baker would accept. It is checked anyway
/// because the failure is not "a wrong object is deleted" — the keys are still all under
/// `wx/v2/<named generation>/` — it is *time*: each generation is `shard_count x frames` process
/// spawns held under the cycle's lock, so a document naming a few hundred is a baker that has
/// stopped baking.
pub const MAX_GENERATIONS_PER_SWEEP: usize = manifest_v2::RETAINED_PREVIOUS_GENERATIONS + 1;

/// What one [`sweep`] retired.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// The generations collected, newest first. Empty is the normal answer for the first three
    /// cycles after a bootstrap, and for a re-bake of a reference time — the latter *only* because
    /// `Builder::new` filters the republished generation out before capping the chain, so the
    /// keep-set stays whole. Getting that order wrong makes a re-bake sweep a generation early;
    /// see [`Carried::named`](crate::manifest_v2::Carried::named).
    pub generations: Vec<String>,
    /// Objects retired. A full generation is `shard_count x frames` minus its dry shards.
    ///
    /// What this counts depends on what the store can see, and it says so rather than pretending:
    /// against [`DirStore`](crate::publish::DirStore) it is objects that were *there* and are gone;
    /// against R2 it is keys the endpoint accepted a delete for, because `DeleteObject` is
    /// idempotent and cannot report the difference. **Do not read emptiness off this number** — a
    /// zero here is meaningful only for a store that can tell. The signal that a generation was
    /// already empty is [`canary`]'s, and it arrives as a warning.
    pub deleted_objects: usize,
    /// Bytes the store could account for without a second round-trip: real against a directory
    /// store, `0` against R2 (see [`crate::publish::Deleted::bytes`]).
    pub accounted_bytes: u64,
    /// One line per generation that hit store errors — never per key, or a bucket-wide outage would
    /// write 216 lines into one cycle's report.
    pub warnings: Vec<String>,
}

/// The generations to delete: those the **predecessor** manifest named and the **published** one
/// does not.
///
/// In steady state this is a single generation, N-3, and the arithmetic that makes it so is worth
/// following once: the predecessor manifest named N-1 (itself), N-2 and N-3; the document just
/// published names N, N-1 and N-2; the difference is N-3. Nothing in it is derived from a clock or
/// from the shape of a key — both sides are identifiers a manifest stated.
///
/// It returns empty, meaning **delete nothing**, whenever the licence is missing:
///
/// * there was no readable predecessor ([`Carried::had_predecessor`]) — a bootstrap has promised
///   nothing and may take nothing away;
/// * the published document is not this baker's tree (`key_prefix`), which would mean the keys
///   below are computed for somewhere this sweep has no business deleting from;
/// * a candidate is not a well-formed generation identifier. `carried_generations` already refuses
///   those, so this is unreachable — and it is checked anyway, because it is the last thing between
///   a bad string and a delete.
pub fn delete_set(carried: &Carried, published: &Manifest) -> Vec<String> {
    if !carried.had_predecessor() || published.key_prefix != manifest_v2::KEY_PREFIX {
        return Vec::new();
    }
    let keep: BTreeSet<&str> = std::iter::once(published.generation.as_str())
        .chain(published.previous_generations.iter().map(String::as_str))
        .collect();
    carried
        .named()
        .iter()
        .filter(|generation| manifest_v2::is_generation_id(generation))
        .filter(|generation| !keep.contains(generation.as_str()))
        .cloned()
        .collect()
}

/// Every object key a generation of this lattice could hold, in publish order.
///
/// Which of them a given generation *actually* published is not knowable without a listing or a
/// manifest that no longer exists — dry shards are omitted (`OBCG_Spec.md` §10.3) — so the sweep
/// asks for all of them and takes "there was nothing there" as the success it is
/// ([`crate::publish::Deleted::existed`]). The keys are composed by the same
/// [`manifest_v2::shard_key`] the publisher and every client use, so they cannot drift into a
/// different spelling of the same object.
fn generation_keys(lattice: &Lattice, times: CycleTimes, generation: &str) -> Vec<String> {
    let mut keys = Vec::with_capacity((lattice.shard_count() as usize) * times.offsets_min().count());
    for offset_min in times.offsets_min() {
        for shard in 0..lattice.shard_count() {
            let (col, row) = lattice.shard_col_row(shard);
            keys.push(manifest_v2::shard_key(manifest_v2::KEY_PREFIX, generation, offset_min, col, row));
        }
    }
    keys
}

/// How many keys the canary samples before concluding a generation was already empty.
///
/// It is a *sample*, not a census — a census is a `head` per key, which doubles the sweep's
/// round-trips for a number only a report line reads, and the module has refused that from the
/// start. 24 covers the canonical 6x4 shard grid exactly once.
const CANARY_PROBES: usize = 24;

/// Does this generation still hold anything? Asked **before** any delete, because afterwards the
/// question is unanswerable.
///
/// `Some(true)` — a probe found an object, so the generation was there and the sweep is doing real
/// work. `Some(false)` — every probe came back absent. `None` — the store could not answer, which
/// is not evidence of anything and must not be reported as if it were.
///
/// It stops at the first hit, so the steady-state cost is **one request per retired generation**.
/// Only a generation that really is gone pays all [`CANARY_PROBES`], once, and that is the case
/// worth paying for.
/// The keys the canary probes: **every shard the budget can reach, enumerated**, each at a
/// different frame in rotation.
///
/// Round 2 of #1282's review caught the previous version, and the bug is worth stating because it
/// is the kind that hides in an innocuous-looking `step_by`. That version strided the flat key list
/// by `keys.len() / CANARY_PROBES` — 216 / 24 = **9** — and the list is offset-major, so the shard
/// a probe landed on was `index % 24`. `gcd(9, 24) = 3`, so the probes only ever touched shards
/// `{0, 3, 6, ... 21}`: eight of twenty-four, and since `col = shard % 6`, only **columns 0 and 3
/// of six**. A generation whose objects all sat in the other four columns read as empty, which
/// raises a "something else is sweeping this prefix" alarm about a generation that is simply there.
///
/// The fix deliberately is not "pick a stride coprime with the shard count". That works, but it
/// stays correct only while someone re-checks the arithmetic every time the shard grid changes, and
/// the grid is `lattice` configuration precisely so it *can* change. Enumerating shards has no
/// arithmetic to get wrong: probe `n` asks about shard `n`. The one invariant left is that the
/// budget must cover at least a full row, so every column is represented — asserted below and
/// pinned by `the_canary_samples_every_shard_column`.
fn canary_keys(lattice: &Lattice, times: CycleTimes, generation: &str) -> Vec<String> {
    let offsets: Vec<u32> = times.offsets_min().collect();
    let shards = lattice.shard_count();
    if shards == 0 || offsets.is_empty() {
        return Vec::new();
    }
    debug_assert!(
        CANARY_PROBES >= lattice.shard_cols() as usize,
        "the probe budget must cover a whole shard row, or some column is never asked about"
    );
    let mut keys = Vec::with_capacity(CANARY_PROBES.min(shards as usize));
    for probe in 0..CANARY_PROBES.min(shards as usize) {
        let (col, row) = lattice.shard_col_row(probe as u32);
        // Rotate the frame as well: asking every shard about `f0` would miss a generation whose
        // early frames happen to be dry, and the frames cost nothing extra to spread across.
        let offset = offsets[probe % offsets.len()];
        keys.push(manifest_v2::shard_key(manifest_v2::KEY_PREFIX, generation, offset, col, row));
    }
    keys
}

fn canary(
    store: &mut dyn ObjectStore,
    lattice: &Lattice,
    times: CycleTimes,
    warnings: &mut Vec<String>,
    generation: &str,
) -> Option<bool> {
    let keys = canary_keys(lattice, times, generation);
    if keys.is_empty() {
        return None;
    }
    for key in &keys {
        match store.head(key) {
            Ok(Some(_)) => return Some(true),
            Ok(None) => {}
            Err(error) => {
                // A store that cannot answer has told us nothing about the generation, and guessing
                // "empty" here would turn a transient error into a false alarm about a second
                // sweeper. Say what happened and decline to conclude.
                warnings.push(format!(
                    "retention sweep: could not check whether generation {generation} still held objects \
                     ({key}: {error}) — deleting it anyway, which is safe and idempotent, but this cycle \
                     cannot say whether it was already empty"
                ));
                return None;
            }
        }
    }
    Some(false)
}

/// Retire every generation [`delete_set`] names. **Call this only after the new manifest is
/// durably in place.**
///
/// It never returns `Err`: a store that cannot delete has left objects nothing references, which
/// costs storage and nothing else, and the caller's job at this point is to report a successful
/// cycle. Errors become [`SweepReport::warnings`].
pub fn sweep(
    store: &mut dyn ObjectStore,
    lattice: &Lattice,
    times: CycleTimes,
    carried: &Carried,
    published: &Manifest,
) -> SweepReport {
    let mut report = SweepReport::default();
    let doomed = delete_set(carried, published);
    // Bounded work, refused rather than truncated. `carried_generations` already enforces §10.4's
    // cap, so this is unreachable — but the cost of being wrong is a baker that stops baking:
    // every extra generation is another `shard_count x frames` process spawns while holding the
    // cycle's lock. Refusing keeps the leak visible; truncating would hide it, which is the trade
    // `Carried::named` exists to refuse.
    if doomed.len() > MAX_GENERATIONS_PER_SWEEP {
        report.warnings.push(format!(
            "retention sweep: the delete set names {} generations, over the {MAX_GENERATIONS_PER_SWEEP} \
             a single cycle can be asked to retire — sweeping nothing rather than spending the cycle's \
             lock on it. The generations are unreferenced and the bucket's 1-day lifecycle rule collects \
             them",
            doomed.len()
        ));
        return report;
    }
    for generation in doomed {
        let mut failures = 0usize;
        let mut first_error: Option<String> = None;
        let mut deleted_here = 0usize;
        let prefix = format!("{}/{generation}/", manifest_v2::KEY_PREFIX);
        // **The canary, before a single delete** (#1282 review M4). Everything below this loses the
        // ability to tell an already-empty generation from a full one the moment the store is R2,
        // because `DeleteObject` answers the same either way — so ask *first*, while asking still
        // means something.
        let found_something = canary(store, lattice, times, &mut report.warnings, &generation);
        for key in generation_keys(lattice, times, &generation) {
            // The last guard, and it is a **runtime** one: a key this sweep deletes must be inside
            // the generation it is collecting, which is a generation the published manifest does
            // not name. `shard_key` composes it, so this can only fail if that composition changes
            // under us — which is exactly the change that should stop a delete. It was a
            // `debug_assert!` until round 1 of #1274's review pointed out that `install.sh` builds
            // `--release`, so on the box the "last thing between a bad string and a delete" was
            // nothing at all. A string compare against a process spawn is free.
            if !key.starts_with(&prefix) {
                failures += 1;
                first_error.get_or_insert(format!("{key}: not under {prefix} — refusing to delete it"));
                continue;
            }
            match store.delete(&key) {
                Ok(deleted) => match deleted.existed {
                    // The store knows there was an object and it is gone.
                    Some(true) => {
                        deleted_here += 1;
                        report.accounted_bytes += deleted.bytes.unwrap_or(0);
                    }
                    // The store knows there was nothing. Not an error — see `Deleted::existed`.
                    Some(false) => {}
                    // The store cannot tell, because `DeleteObject` is idempotent and answers the
                    // same either way. Count the key as retired: it is, and the alternative is a
                    // report that says every R2 sweep deleted nothing. Whether the generation held
                    // anything at all is the canary's question, asked above.
                    None => deleted_here += 1,
                },
                Err(error) => {
                    failures += 1;
                    first_error.get_or_insert(format!("{key}: {error}"));
                }
            }
        }
        report.deleted_objects += deleted_here;
        if let Some(error) = first_error {
            report.warnings.push(format!(
                "retention sweep: {failures} of generation {generation}'s objects could not be deleted \
                 (first: {error}). They are unreferenced, so nothing serves them; the bucket's 1-day \
                 lifecycle rule collects the leak. Next cycle will not retry them — this generation is \
                 already off the manifest chain"
            ));
        } else if found_something == Some(false) {
            // A generation the chain named should have had objects, so finding none is the
            // interesting answer — and since #1282's review it is the *canary's* answer rather than
            // a delete count, which on R2 can no longer say anything (`Deleted::existed` is `None`
            // there, so `deleted_here` is the key count every time and this line could never fire).
            //
            // What it means: the generation was already collected, which in steady state means a
            // second sweeper is running — the anomaly this warning exists to surface.
            report.warnings.push(format!(
                "retention sweep: generation {generation} was already empty before this sweep touched it \
                 ({CANARY_PROBES} keys sampled across its frames, none present). It was collected by \
                 something else — check nothing is running a second baker against this prefix"
            ));
        }
        report.generations.push(generation);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::CANONICAL;
    use crate::manifest_v2::Builder;
    use crate::publish::{Deleted, PlannedObject};

    /// A document with a **hand-written** chain. Useful for stating a starting state, but note it
    /// is *not* the composition the cycle runs — that is [`republish`], and the difference is where
    /// round 1's blocker hid.
    fn manifest(generation: &str, previous: &[&str]) -> Manifest {
        let mut document = Builder::new(
            &CANONICAL,
            CycleTimes { reference_time: reference_time(generation) },
            0,
            Vec::new(),
            previous.iter().map(|id| (*id).to_string()).collect(),
        )
        .finish();
        assert_eq!(document.generation, generation);
        document.frames.truncate(1);
        document
    }

    fn reference_time(generation: &str) -> i64 {
        crate::timefmt::parse_key_timestamp(generation).expect("a generation id is a timestamp")
    }

    /// **The real composition**, end to end: read the predecessor back the way `run_cycle` does,
    /// hand its *uncapped* candidate list to the builder, finish. Every retention assertion that
    /// matters has to go through this — a test that hand-builds `previous_generations` is asserting
    /// about a manifest the baker cannot produce.
    fn republish(predecessor: &Manifest, generation: &str) -> (Carried, Manifest) {
        let carried = carried(predecessor);
        let mut document = Builder::new(
            &CANONICAL,
            CycleTimes { reference_time: reference_time(generation) },
            0,
            Vec::new(),
            carried.named().to_vec(),
        )
        .finish();
        assert_eq!(document.generation, generation);
        document.frames.truncate(1);
        (carried, document)
    }

    /// Build a `Carried` the only way anything can: by parsing a document a publisher wrote.
    fn carried(document: &Manifest) -> Carried {
        let mut warnings = Vec::new();
        let carried = manifest_v2::carried_generations(Some(manifest_v2::to_json(document).as_bytes()), &mut warnings)
            .expect("a document this baker wrote parses");
        assert!(warnings.is_empty());
        carried
    }

    /// A store that answers every delete, records what it was asked to delete, and can be told to
    /// fail — the three things the sweep's contract is about.
    #[derive(Default)]
    struct RecordingStore {
        present: BTreeSet<String>,
        deleted: Vec<String>,
        /// Every key the canary probed, in order — the measure of a sample that became a census.
        headed: Vec<String>,
        fail_on: Option<&'static str>,
        /// Answer every delete `existed: None`, the way S3 `DeleteObject` does.
        cannot_tell: bool,
        fail_head: bool,
    }

    impl ObjectStore for RecordingStore {
        fn describe(&self) -> String {
            "recording".into()
        }
        fn put(&mut self, object: &PlannedObject) -> Result<(), String> {
            self.present.insert(object.key.clone());
            Ok(())
        }
        fn head(&mut self, key: &str) -> Result<Option<u64>, String> {
            self.headed.push(key.to_string());
            if self.fail_head {
                return Err("503 slow down".into());
            }
            Ok(self.present.contains(key).then_some(1))
        }
        fn get(&mut self, _key: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }
        fn delete(&mut self, key: &str) -> Result<Deleted, String> {
            if self.fail_on.is_some_and(|needle| key.contains(needle)) {
                return Err("503 slow down".into());
            }
            self.deleted.push(key.to_string());
            let existed = self.present.remove(key);
            if self.cannot_tell {
                return Ok(Deleted { existed: None, bytes: None });
            }
            Ok(Deleted { existed: Some(existed), bytes: Some(9) })
        }
    }

    /// The steady-state answer, and the whole point of `Carried::named` being uncapped: publishing
    /// N retires N-3 and nothing else.
    #[test]
    fn the_delete_set_is_the_one_generation_that_fell_off_the_chain() {
        let predecessor = manifest("20260810T1445Z", &["20260810T1430Z", "20260810T1415Z"]);
        let published = manifest("20260810T1500Z", &["20260810T1445Z", "20260810T1430Z"]);
        assert_eq!(delete_set(&carried(&predecessor), &published), vec!["20260810T1415Z"]);
    }

    /// **A re-bake retires nothing, and it promises the same two generations it did before.**
    ///
    /// Round 1 of #1274's review reproduced the opposite, and the reason it got past the first
    /// version of this suite is instructive: the old re-bake case hand-built the published document
    /// with an explicit two-entry chain, which is a manifest the baker could not produce. Driven
    /// through the real composition — `carried_generations` -> `Builder::new(named())` -> `finish`
    /// -> `delete_set` — a capped-then-filtered chain publishes `previous = [N-1]` and sweeps N-2 a
    /// full cycle early, halving §10.4's overlap for exactly the clients it exists for.
    #[test]
    fn a_rebake_keeps_its_whole_chain_and_sweeps_nothing() {
        let predecessor = manifest("20260810T1545Z", &["20260810T1530Z", "20260810T1515Z"]);
        let (carried, republished) = republish(&predecessor, "20260810T1545Z");
        assert_eq!(
            republished.previous_generations,
            vec!["20260810T1530Z", "20260810T1515Z"],
            "a republished manifest must promise the same two generations, not one"
        );
        assert!(
            delete_set(&carried, &republished).is_empty(),
            "a re-bake retired a generation the previous publish had promised to keep"
        );

        // And the ordinary step, through the same composition, still retires exactly N-3.
        let (carried, next) = republish(&predecessor, "20260810T1600Z");
        assert_eq!(next.previous_generations, vec!["20260810T1545Z", "20260810T1530Z"]);
        assert_eq!(delete_set(&carried, &next), vec!["20260810T1515Z"]);
    }

    /// **The invariant.** Whatever the predecessor said, nothing the published manifest names may
    /// end up in the delete set — including the pathological case where the predecessor named the
    /// generation being published (a re-bake) and the one where the chains fully overlap.
    #[test]
    fn a_generation_the_published_manifest_names_is_never_in_the_delete_set() {
        let published = manifest("20260810T1500Z", &["20260810T1445Z", "20260810T1430Z"]);
        let named: BTreeSet<&str> = std::iter::once(published.generation.as_str())
            .chain(published.previous_generations.iter().map(String::as_str))
            .collect();
        for predecessor in [
            manifest("20260810T1445Z", &["20260810T1430Z", "20260810T1415Z"]),
            // A re-bake: the predecessor *is* the generation now being published.
            manifest("20260810T1500Z", &["20260810T1445Z", "20260810T1430Z"]),
            manifest("20260810T1445Z", &["20260810T1430Z"]),
            manifest("20260810T1430Z", &[]),
        ] {
            let set = delete_set(&carried(&predecessor), &published);
            for generation in &set {
                assert!(!named.contains(generation.as_str()), "{generation} is still referenced by the new manifest");
            }
        }
        // The same property over the composition the cycle actually runs, for every predecessor
        // shape: a document built from `named()` can never name something its own sweep collects.
        for (predecessor, generation) in [
            (manifest("20260810T1445Z", &["20260810T1430Z", "20260810T1415Z"]), "20260810T1500Z"),
            (manifest("20260810T1500Z", &["20260810T1445Z", "20260810T1430Z"]), "20260810T1500Z"),
            (manifest("20260810T1430Z", &[]), "20260810T1445Z"),
        ] {
            let (carried, document) = republish(&predecessor, generation);
            let keep: BTreeSet<&str> = std::iter::once(document.generation.as_str())
                .chain(document.previous_generations.iter().map(String::as_str))
                .collect();
            for doomed in delete_set(&carried, &document) {
                assert!(!keep.contains(doomed.as_str()), "{doomed} is still referenced by the new manifest");
            }
        }
        // …and the same, at the level the deletion actually happens: not one key the sweep issues
        // may belong to a generation the published document names.
        let mut store = RecordingStore::default();
        let predecessor = manifest("20260810T1445Z", &["20260810T1430Z", "20260810T1415Z"]);
        sweep(&mut store, &CANONICAL, CycleTimes { reference_time: 0 }, &carried(&predecessor), &published);
        assert!(!store.deleted.is_empty());
        for key in &store.deleted {
            for generation in &named {
                assert!(!key.contains(*generation), "{key} belongs to a generation still on the chain");
            }
        }
    }

    /// Bootstrap: nothing was ever promised, so nothing may be taken away. This is the case that
    /// separates "there is no manifest" from "I could not read the manifest" — the latter never
    /// reaches the sweep, because it fails the cycle before anything is published.
    #[test]
    fn a_bootstrap_sweeps_nothing() {
        let published = manifest("20260810T1500Z", &[]);
        let mut store = RecordingStore::default();
        let report = sweep(&mut store, &CANONICAL, CycleTimes { reference_time: 0 }, &Carried::default(), &published);
        assert_eq!(report, SweepReport::default());
        assert!(store.deleted.is_empty(), "a first publish issued a delete");

        // A document that names no generation is not a predecessor either (WXR3's placeholder).
        let mut warnings = Vec::new();
        let placeholder =
            manifest_v2::carried_generations(Some(br#"{"version":2,"note":"placeholder"}"#), &mut warnings)
                .expect("bootstrap");
        assert!(delete_set(&placeholder, &published).is_empty());
    }

    /// A generation is swept whole: every key of the grid is asked for, and the ones that were not
    /// there (dry shards, omitted by §10.3) are successes rather than errors.
    #[test]
    fn a_swept_generation_is_asked_for_key_by_key_and_absence_is_success() {
        let times = CycleTimes { reference_time: 0 };
        let mut store = RecordingStore::default();
        // Publish two objects of the doomed generation; the other 214 keys were dry or never made.
        for key in ["wx/v2/20260810T1415Z/f0/s0-0.obcg", "wx/v2/20260810T1415Z/f45/s5-3.obcg"] {
            store.present.insert(key.to_string());
        }
        let predecessor = manifest("20260810T1445Z", &["20260810T1430Z", "20260810T1415Z"]);
        let published = manifest("20260810T1500Z", &["20260810T1445Z", "20260810T1430Z"]);
        let report = sweep(&mut store, &CANONICAL, times, &carried(&predecessor), &published);

        let expected_keys = CANONICAL.shard_count() as usize * times.offsets_min().count();
        assert_eq!(store.deleted.len(), expected_keys, "every key of the grid is asked for exactly once");
        assert_eq!(report.generations, vec!["20260810T1415Z"]);
        assert_eq!(report.deleted_objects, 2, "only the two that existed count as deleted");
        assert_eq!(report.accounted_bytes, 18);
        assert!(report.warnings.is_empty(), "absence is not a warning");
        assert!(store.present.is_empty());
    }

    /// **A failed delete is a warning, not a failed cycle** — and it is *one* warning, because a
    /// bucket-wide outage must not write a line per key into one cycle's report.
    #[test]
    fn a_failed_delete_warns_once_and_the_cycle_still_succeeded() {
        let mut store = RecordingStore { fail_on: Some("f0/"), ..RecordingStore::default() };
        let predecessor = manifest("20260810T1445Z", &["20260810T1430Z", "20260810T1415Z"]);
        let published = manifest("20260810T1500Z", &["20260810T1445Z", "20260810T1430Z"]);
        let report =
            sweep(&mut store, &CANONICAL, CycleTimes { reference_time: 0 }, &carried(&predecessor), &published);

        assert_eq!(report.generations, vec!["20260810T1415Z"], "the generation is still reported as collected");
        assert_eq!(report.warnings.len(), 1, "one line per generation, never one per key");
        let warning = &report.warnings[0];
        assert!(warning.contains("24 of generation 20260810T1415Z"), "{warning}");
        assert!(warning.contains("lifecycle rule collects the leak"), "{warning}");
        // The keys that did not fail were still collected: a partial store failure is partial.
        assert!(store.deleted.iter().all(|key| !key.contains("f0/")));
        assert_eq!(store.deleted.len(), CANONICAL.shard_count() as usize * 8);
    }

    /// A named generation that was **already empty** is reported rather than passed over: in steady
    /// state it means something else collected it, which is a second sweeper against this prefix.
    ///
    /// The signal is the canary's, taken before any delete. It has to be, because on R2
    /// `Deleted::existed` is `None` and a delete count can no longer distinguish anything.
    #[test]
    fn a_generation_that_was_already_empty_says_so() {
        let mut store = RecordingStore::default();
        let predecessor = manifest("20260810T1445Z", &["20260810T1430Z", "20260810T1415Z"]);
        let published = manifest("20260810T1500Z", &["20260810T1445Z", "20260810T1430Z"]);
        let report =
            sweep(&mut store, &CANONICAL, CycleTimes { reference_time: 0 }, &carried(&predecessor), &published);
        assert_eq!(report.deleted_objects, 0);
        assert_eq!(report.generations, vec!["20260810T1415Z"]);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("was already empty"), "{}", report.warnings[0]);
        // The canary is a sample and it stops at the first hit, so an empty generation costs
        // exactly its probe budget — never a `head` per key.
        assert_eq!(store.headed.len(), CANARY_PROBES);
    }

    /// **The blind spot, pinned** (#1282 review round 2, F1).
    ///
    /// The canary's probe set must reach every shard column. The version this replaces strided the
    /// flat key list by 9 over a 24-shard grid — `gcd(9, 24) = 3` — and so only ever asked about
    /// columns 0 and 3 of six, calling a generation whose objects lived in the other four "already
    /// empty" and raising a second-sweeper alarm about a healthy one.
    ///
    /// The assertions are on *coverage*, not on the mechanism, so a future change to how the probe
    /// set is built has to keep the property rather than the implementation.
    #[test]
    fn the_canary_samples_every_shard_column() {
        let keys = canary_keys(&CANONICAL, CycleTimes { reference_time: 0 }, "20260810T1415Z");
        assert_eq!(keys.len(), CANARY_PROBES.min(CANONICAL.shard_count() as usize));

        let mut columns = BTreeSet::new();
        let mut shards = BTreeSet::new();
        for key in &keys {
            let name = key.rsplit('/').next().expect("a key ends in the object name");
            let coords = name.trim_start_matches('s').trim_end_matches(".obcg");
            let (col, row) = coords.split_once('-').expect("s<col>-<row>.obcg");
            columns.insert(col.parse::<u32>().expect("a column"));
            shards.insert((col.parse::<u32>().expect("col"), row.parse::<u32>().expect("row")));
        }
        assert_eq!(
            columns.len() as u32,
            CANONICAL.shard_cols(),
            "the canary never asks about columns {:?} — a generation living there reads as empty",
            (0..CANONICAL.shard_cols()).filter(|col| !columns.contains(col)).collect::<Vec<_>>()
        );
        assert_eq!(shards.len(), keys.len(), "a probe was spent asking about a shard already covered");

        // Frames are rotated rather than every probe landing on f0, so a generation whose earliest
        // frames are dry is still found.
        let frames: BTreeSet<&str> = keys.iter().filter_map(|key| key.split('/').nth(3)).collect();
        assert!(frames.len() > 1, "every probe asked about the same frame: {frames:?}");

        // And every probe is a key this sweep would have deleted — the canary must never invent a
        // key shape of its own.
        let all: BTreeSet<String> =
            generation_keys(&CANONICAL, CycleTimes { reference_time: 0 }, "20260810T1415Z").into_iter().collect();
        for key in &keys {
            assert!(all.contains(key), "{key} is not a key of this generation");
        }
    }

    /// **The R2 shape** (#1282 review M4/M5): a store that cannot tell whether a key existed.
    ///
    /// Every delete answers `existed: None`, exactly as `DeleteObject` does. The sweep must still
    /// report the objects as retired — the alternative is a report claiming every R2 sweep deleted
    /// nothing — and must **not** raise the already-empty warning, because the canary found the
    /// generation present.
    #[test]
    fn a_store_that_cannot_tell_still_reports_what_it_retired() {
        let mut store = RecordingStore { cannot_tell: true, ..RecordingStore::default() };
        let predecessor = manifest("20260810T1445Z", &["20260810T1430Z", "20260810T1415Z"]);
        let published = manifest("20260810T1500Z", &["20260810T1445Z", "20260810T1430Z"]);
        let doomed = "20260810T1415Z";
        let times = CycleTimes { reference_time: 0 };
        let keys = generation_keys(&CANONICAL, times, doomed);
        store.present = keys.iter().cloned().collect();

        let report = sweep(&mut store, &CANONICAL, times, &carried(&predecessor), &published);
        assert_eq!(report.generations, vec![doomed]);
        assert_eq!(report.deleted_objects, keys.len(), "a store that cannot tell must not report zero");
        assert_eq!(report.accounted_bytes, 0, "and it cannot account bytes it was never told");
        assert!(report.warnings.is_empty(), "the canary found it present: {:?}", report.warnings);
        // One probe: the canary stops at the first hit, so a present generation costs one request.
        assert_eq!(store.headed.len(), 1);
        assert_eq!(store.deleted.len(), keys.len());
    }

    /// The same store, against a generation that is genuinely gone: `existed: None` everywhere, so
    /// only the canary can raise the alarm — and it does.
    #[test]
    fn a_store_that_cannot_tell_still_detects_an_already_empty_generation() {
        let mut store = RecordingStore { cannot_tell: true, ..RecordingStore::default() };
        let predecessor = manifest("20260810T1445Z", &["20260810T1430Z", "20260810T1415Z"]);
        let published = manifest("20260810T1500Z", &["20260810T1445Z", "20260810T1430Z"]);
        let report =
            sweep(&mut store, &CANONICAL, CycleTimes { reference_time: 0 }, &carried(&predecessor), &published);
        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        assert!(report.warnings[0].contains("was already empty"), "{}", report.warnings[0]);
        assert!(report.warnings[0].contains("second baker"), "{}", report.warnings[0]);
    }

    /// A canary that cannot get an answer must not invent one. A `head` error is reported and the
    /// sweep proceeds — deleting is idempotent and safe — but no already-empty alarm is raised off
    /// a store that never said anything.
    #[test]
    fn a_canary_that_errors_declines_to_conclude() {
        let mut store = RecordingStore { fail_head: true, ..RecordingStore::default() };
        let predecessor = manifest("20260810T1445Z", &["20260810T1430Z", "20260810T1415Z"]);
        let published = manifest("20260810T1500Z", &["20260810T1445Z", "20260810T1430Z"]);
        let report =
            sweep(&mut store, &CANONICAL, CycleTimes { reference_time: 0 }, &carried(&predecessor), &published);
        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        assert!(report.warnings[0].contains("cannot say whether it was already empty"), "{}", report.warnings[0]);
        assert!(!report.warnings[0].contains("second baker"), "an error is not evidence of a second sweeper");
        // It still swept: the deletes are idempotent and the generation is off the chain either way.
        assert_eq!(store.deleted.len(), generation_keys(&CANONICAL, CycleTimes { reference_time: 0 }, "x").len());
    }

    /// The work bound. `carried_generations` already refuses a chain this long, so the constant is
    /// a backstop — but it must be a refusal and never a truncation, because truncating is how the
    /// leak `Carried::named` exists to prevent gets back in.
    #[test]
    fn the_sweep_refuses_an_implausibly_large_delete_set_rather_than_truncating_it() {
        assert_eq!(MAX_GENERATIONS_PER_SWEEP, manifest_v2::RETAINED_PREVIOUS_GENERATIONS + 1);
        // The only way to a longer chain is a document `carried_generations` would refuse, which is
        // where the real enforcement lives; pinned there by
        // `a_chain_longer_than_the_cap_is_refused_rather_than_truncated`.
        let mut warnings = Vec::new();
        let long = format!(
            r#"{{"version":2,"generation":"20260810T1500Z","previous_generations":{}}}"#,
            serde_json::to_string(&["20260810T1445Z", "20260810T1430Z", "20260810T1415Z"]).unwrap()
        );
        assert!(manifest_v2::carried_generations(Some(long.as_bytes()), &mut warnings).is_err());
    }

    /// The sweep computes keys for **this baker's** tree and refuses anything else, so a document
    /// that somehow arrived with a foreign `key_prefix` cannot aim a delete at it.
    #[test]
    fn a_foreign_key_prefix_sweeps_nothing() {
        let predecessor = manifest("20260810T1445Z", &["20260810T1430Z", "20260810T1415Z"]);
        let mut published = manifest("20260810T1500Z", &["20260810T1445Z", "20260810T1430Z"]);
        published.key_prefix = "wx/v3".to_string();
        assert!(delete_set(&carried(&predecessor), &published).is_empty());
    }
}
