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
//! sweep deletes is not a budget question at all. What it does cost is one `rclone` process spawn
//! per key: a full generation is `shard_count x frames` = 216 keys, ~0.2 s each, so ~45 s of
//! wall-clock. That is paid after the manifest is already in place, with nothing waiting on it, on
//! a 15-minute cadence. It is not worth a `list` (which would need a new store capability) or a
//! `head` per key (which would double it) to shave.
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

/// What one [`sweep`] retired.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// The generations collected, newest first. Empty is the normal answer for the first three
    /// cycles after a bootstrap, and for any re-bake of a reference time.
    pub generations: Vec<String>,
    /// Objects that were there and are gone. A full generation is `shard_count x frames` minus its
    /// dry shards, so a number well under that is normal and a *zero* against a named generation is
    /// the interesting one — it means the generation was already collected, or never existed.
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
    for generation in delete_set(carried, published) {
        let mut failures = 0usize;
        let mut first_error: Option<String> = None;
        for key in generation_keys(lattice, times, &generation) {
            // The last guard, and it is cheap: a key this sweep deletes must be inside the
            // generation it is collecting, which is a generation the published manifest does not
            // name. `shard_key` composes it, so this can only fail if that composition changes
            // under us — which is exactly the change that should stop a delete.
            debug_assert!(key.starts_with(&format!("{}/{generation}/", manifest_v2::KEY_PREFIX)), "{key}");
            match store.delete(&key) {
                Ok(deleted) => {
                    if deleted.existed {
                        report.deleted_objects += 1;
                        report.accounted_bytes += deleted.bytes.unwrap_or(0);
                    }
                }
                Err(error) => {
                    failures += 1;
                    first_error.get_or_insert(format!("{key}: {error}"));
                }
            }
        }
        if let Some(error) = first_error {
            report.warnings.push(format!(
                "retention sweep: {failures} of generation {generation}'s objects could not be deleted \
                 (first: {error}). They are unreferenced, so nothing serves them; the bucket's 1-day \
                 lifecycle rule collects the leak. Next cycle will not retry them — this generation is \
                 already off the manifest chain"
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

    fn manifest(generation: &str, previous: &[&str]) -> Manifest {
        let reference_time = crate::timefmt::parse_rfc3339(&format!(
            "{}-{}-{}T{}:{}:00Z",
            &generation[0..4],
            &generation[4..6],
            &generation[6..8],
            &generation[9..11],
            &generation[11..13]
        ))
        .expect("a generation id is a timestamp");
        let mut document = Builder::new(
            &CANONICAL,
            CycleTimes { reference_time },
            reference_time,
            Vec::new(),
            previous.iter().map(|id| (*id).to_string()).collect(),
        )
        .finish();
        assert_eq!(document.generation, generation);
        document.frames.truncate(1);
        document
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
        fail_on: Option<&'static str>,
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
            Ok(Deleted { existed: self.present.remove(key), bytes: Some(9) })
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
