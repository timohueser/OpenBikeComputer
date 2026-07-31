//! Which map on the card the renderer streams from (issue #927).
//!
//! Before the device could receive a map, a card held one and the loader took "the first `*.obcm`
//! the directory scan yields". With uploads that stops being an answer: directory order is not
//! something a rider can predict, let alone steer, and the one thing they *do* expect after sending
//! a map to a plugged-in device is to see that map.
//!
//! The rule is a pure function over the scanned catalog so it can be tested where tests actually
//! run. The board crate has no `test` harness in CI (bare metal), and its own storage scan is the
//! only thing that could exercise this — so the decision lives here and the board binds its
//! `MapSummary` list to [`MapChoice`] at the call site.

/// One candidate from the card's map catalog, reduced to the three facts the choice turns on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapChoice {
    /// The card's recorded selection (`MAP.SEL`) names this map.
    pub selected: bool,
    /// The durable object id, for a map the device received (`MP{id}.OBM`); `None` for a side-loaded
    /// file, which has no id. Higher = more recently uploaded, because ids are minted monotonically.
    pub uploaded_id: Option<u16>,
    /// The map's OBCM version is the one this firmware's reader parses. A `false` here is still a
    /// map on the card — it just cannot be rendered by this build.
    pub readable: bool,
    /// Whether this entry is an OBCA **volume set** (`MS{id}.OBS` + its shards, `OBCA_Spec.md` §5)
    /// rather than a single `MP{id}.OBM` / side-loaded file. A set is *one map* everywhere a rider
    /// can see it (§5.4) — this flag exists only so [`is_superseded_upload`] can refuse to compare
    /// ids across the two naming conventions, which number independently.
    pub set: bool,
}

/// The index of the map to load, or `None` for a card with no maps at all. In order:
///
/// 1. **The recorded selection**, if it is still on the card and readable. This is what makes the
///    choice durable and, later, steerable from outside.
/// 2. **The newest readable upload** — the highest `MP{id}.OBM` id. A rider who just sent a map
///    gets that map on the next boot without a second step, which is the entire point of the
///    one-click flow; and because ids are monotonic within a store epoch, "newest" is exactly
///    "highest id", with no timestamps involved (the card has none to offer).
/// 3. **The first readable map of any kind** — a side-loaded `.obcm`, the pre-#927 case.
/// 4. **The first map at all.** Deliberately not `None`: a card holding only a wrong-version map
///    must reach the *MAP UNREADABLE* fault screen, which names the actual problem, rather than the
///    *NO MAP* one, which would send the rider looking for a file that is right there.
///
/// A selection that names a map which is present but **unreadable** does not win outright — it
/// falls through to a readable one if the card has any, and only lands back on itself via clause 4
/// if it doesn't. Honouring it blindly would let one stale selection hide a perfectly good map.
pub fn choose_map(maps: &[MapChoice]) -> Option<usize> {
    if maps.is_empty() {
        return None;
    }
    if let Some(i) = maps.iter().position(|m| m.selected && m.readable) {
        return Some(i);
    }
    let newest_upload = maps
        .iter()
        .enumerate()
        .filter(|(_, m)| m.readable && m.uploaded_id.is_some())
        .max_by_key(|(_, m)| m.uploaded_id.unwrap_or(0))
        .map(|(i, _)| i);
    if let Some(i) = newest_upload {
        return Some(i);
    }
    if let Some(i) = maps.iter().position(|m| m.readable) {
        return Some(i);
    }
    Some(0)
}

/// Whether the map at `i` is an **upload superseded** by the one at `keep` — the card-side half of
/// the one-map rule (#992), and the companion to [`choose_map`]: that picks the survivor, this names
/// the casualties.
///
/// The device loads one map and never switches: [`choose_map`] runs once at startup and the handle
/// is held for the session. A second `MP{id}.OBM` is therefore not a feature without a UI, it is
/// hundreds of megabytes the renderer will never open — and, before this, unreclaimable, because a
/// map upload commits a fresh id and *nothing* deleted its predecessor: not the device (no picker,
/// no delete), and not a host (§4.4's `deleteObject` takes routes and trips, and no map enumeration
/// crosses the wire at all).
///
/// Two things it deliberately does **not** claim:
///
/// - **A side-loaded map is never superseded.** It has no `uploaded_id`, and a rider who copied a
///   file onto the card by hand did something deliberate that no automatic sweep should undo. The
///   rule is "one *uploaded* map", not "one file".
/// - **Nothing is superseded while a side-loaded map is the one loaded.** `keep` having no
///   `uploaded_id` disarms this entirely. That state is only reachable when no upload is readable
///   (clauses 3–4 of [`choose_map`]), so the uploads present are ones this build cannot open — and
///   "I can't read it" is a much weaker claim than "it is redundant". Left alone, they stay
///   diagnosable; a firmware that can read them again would find them.
/// - **Nothing is superseded across the two naming conventions.** `MP{id}.OBM` and the volume
///   set's `MS{id}` (`OBCA_Spec.md` §5.2) number *independently*, so `MS7` is not "newer than"
///   `MP4` — the comparison has no meaning, and the casualty here is deleted. A set supersedes a
///   set and a single map supersedes a single map; a card holding one of each keeps both until a
///   later upload of the same kind replaces one. Erring toward a lingering file is the only
///   direction that is recoverable.
pub fn is_superseded_upload(maps: &[MapChoice], keep: usize, i: usize) -> bool {
    if i == keep {
        return false;
    }
    let (Some(keeper), Some(candidate)) = (maps.get(keep), maps.get(i)) else { return false };
    keeper.set == candidate.set && keeper.uploaded_id.is_some() && candidate.uploaded_id.is_some()
}

/// The index of the **newest volume set** on the card, or `None` if it holds none — the survivor of
/// the `MS{id}` namespace, and the `keep` [`is_superseded_upload`] must be given when it is asked
/// about a set.
///
/// Sets need a keeper of their own because in a build that cannot *mount* one they can never be the
/// loaded map: [`choose_map`]'s clauses 1–3 all filter on `readable`, and a set reports
/// `readable: false`. Keying set retirement on the loaded map would therefore make it unreachable —
/// a card holding a replaced set plus its replacement would carry both forever, and a set is
/// gigabytes, not megabytes, with no device surface that can delete it.
///
/// Naming a survivor without loading it is safe here in a way it would not be for a single map,
/// and the difference is not readability but **proof**: a set is listed only after the scan
/// validated the whole thing (`OBCA_Spec.md` §5.3 — every shard present, at the recorded size, with
/// the recorded header bbox), so the newest set is known to be a complete map. A half-uploaded set
/// has no manifest and is invisible (§5.4), so it can never take this role and can never retire the
/// map it was going to replace.
pub fn newest_set(maps: &[MapChoice]) -> Option<usize> {
    maps.iter()
        .enumerate()
        .filter(|(_, m)| m.set && m.uploaded_id.is_some())
        .max_by_key(|(_, m)| m.uploaded_id.unwrap_or(0))
        .map(|(i, _)| i)
}

/// The boot fault to show when the loop ends up with no map to stream from.
///
/// **NO MAP is only honest when the card holds none.** [`choose_map`]'s clause 4 exists precisely to
/// return a map this build cannot open, so that the rider sees *MAP UNREADABLE* — the screen that
/// says "the file is there and I can't use it" — rather than being sent looking for a file that is
/// sitting in the card root. A volume set is the sharp case: this build lists it, sizes it, and
/// declines to mount it, so a rider with 8 GB of set on the card must never be told there is no map.
pub fn boot_fault(maps: &[MapChoice]) -> crate::BootFault {
    if maps.is_empty() {
        crate::BootFault::NoMap
    } else {
        crate::BootFault::BadMap
    }
}

/// What a card-root directory entry is, as far as the map catalog is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapEntry {
    /// Not part of the catalog: a directory, dot-clutter, a wrong extension — or a volume-set
    /// **shard**, which is a valid OBCM file that is nonetheless not a map (§5.4).
    Other,
    /// A single map: a side-loaded `.obcm` (long-filename arm) or a received `MP{id}.OBM`.
    Map,
    /// A volume-set manifest, `MS{id}.OBS` — the one file that says the shards beside it are one
    /// map (`OBCA_Spec.md` §5.2/§5.4).
    SetManifest,
}

/// Classify one card-root entry for the map scan. Pure over the three facts the decision turns on,
/// so the board's `is_map_entry`/`is_set_manifest_entry` are one-line bindings and the rule itself
/// is tested where tests run (the board crate has no CI harness — see the module docs).
///
/// `short` is the 8.3 name as `BASE.EXT`; `long` the entry's long filename if it has one.
///
/// The safety-critical clause is the shard exclusion. `MS{id}S{kk}.OBM` shares the `.OBM` extension
/// with a received single map — deliberately, so the transfer path needs no new file type — and each
/// shard *is* a valid OBCM file, which is why the exclusion has to live in the name test and cannot
/// fall out of a header read. §5.4: a reader "MUST NOT mount a shard individually as a standalone
/// map", because a geometry shard is a map with no roads and no POIs and the core is a map that
/// draws nothing at all.
pub fn classify_map_entry(short: &[u8], long: Option<&str>, is_directory: bool) -> MapEntry {
    if is_directory || long.is_some_and(|n| n.starts_with('.')) {
        return MapEntry::Other;
    }
    // A shard is never a map, and never a manifest either — it is part of one.
    if obc_formats::obcs::parse_shard_name(short).is_some() {
        return MapEntry::Other;
    }
    // Pure 8.3, no long-name arm: unlike `.obcm`, `.OBS` already fits an 8.3 name, so the device
    // creates it directly and there is no 4-character twin to accept. The id must parse too —
    // `SET.OBS` is clutter, not a manifest.
    if obc_formats::obcs::parse_manifest_name(short).is_some() {
        return MapEntry::SetManifest;
    }
    // The 8.3 arm is the whole answer to "the firmware reads long filenames but cannot create
    // them": `OBM` is the 3-char twin of `.obcm`, unambiguous where a shortened `OBC` would also
    // match a route.
    let long_is_obcm = long.is_some_and(|n| {
        let b = n.as_bytes();
        b.len() >= 5 && b[b.len() - 5..].eq_ignore_ascii_case(b".obcm")
    });
    if long_is_obcm || short.ends_with(b".OBM") {
        return MapEntry::Map;
    }
    MapEntry::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side_loaded(readable: bool) -> MapChoice {
        MapChoice { selected: false, uploaded_id: None, readable, set: false }
    }
    fn uploaded(id: u16, readable: bool) -> MapChoice {
        MapChoice { selected: false, uploaded_id: Some(id), readable, set: false }
    }
    /// An uploaded **volume set** (`MS{id}.OBS` + shards) — one map, `OBCA_Spec.md` §5.4.
    ///
    /// `readable` is deliberately not a parameter: the board's `map_choices` cannot produce a
    /// readable set (this build streams from one open file), so a test that constructed one would
    /// be asserting over a state the device never reaches.
    fn uploaded_set(id: u16) -> MapChoice {
        MapChoice { selected: false, uploaded_id: Some(id), readable: false, set: true }
    }

    #[test]
    fn an_empty_card_chooses_nothing() {
        assert_eq!(choose_map(&[]), None);
    }

    /// The pre-#927 case is preserved exactly: one side-loaded map, no selection, and it loads.
    #[test]
    fn a_single_side_loaded_map_still_loads() {
        assert_eq!(choose_map(&[side_loaded(true)]), Some(0));
    }

    /// The recorded selection wins over both a newer upload and directory order.
    #[test]
    fn the_recorded_selection_wins() {
        let maps = [uploaded(9, true), MapChoice { selected: true, uploaded_id: Some(2), readable: true, set: false }];
        assert_eq!(choose_map(&maps), Some(1), "the selection beats a higher id");

        let maps = [side_loaded(true), MapChoice { selected: true, uploaded_id: None, readable: true, set: false }];
        assert_eq!(choose_map(&maps), Some(1), "a side-loaded map can be the selection too");
    }

    /// With no selection, the newest upload wins — the one-click flow's whole promise. Note it beats
    /// a side-loaded map regardless of scan order.
    #[test]
    fn the_newest_upload_wins_when_nothing_is_selected() {
        let maps = [uploaded(3, true), uploaded(11, true), uploaded(7, true)];
        assert_eq!(choose_map(&maps), Some(1), "highest id = most recently minted");

        let maps = [side_loaded(true), uploaded(1, true)];
        assert_eq!(choose_map(&maps), Some(1), "an upload outranks a side-loaded map");
    }

    /// Version-readability filters every preference tier, so one wrong-version map can never hide a
    /// good one — but a card with nothing readable still yields a map, so the fault screen can be
    /// the specific one.
    #[test]
    fn unreadable_maps_never_hide_a_readable_one() {
        let maps = [uploaded(99, false), uploaded(2, true)];
        assert_eq!(choose_map(&maps), Some(1), "a newer but unreadable upload loses to a readable one");

        let maps = [MapChoice { selected: true, uploaded_id: Some(5), readable: false, set: false }, side_loaded(true)];
        assert_eq!(choose_map(&maps), Some(1), "an unreadable selection falls through to a readable map");

        let maps = [uploaded(5, false), side_loaded(false)];
        assert_eq!(choose_map(&maps), Some(0), "nothing readable → the first map, so the fault names the version");
    }

    /// The rule the card is swept by: after an upload, the map it replaced is the one that goes.
    #[test]
    fn a_newer_upload_supersedes_the_one_it_replaced() {
        let maps = [uploaded(1, true), uploaded(2, true)];
        let keep = choose_map(&maps).expect("a card with maps chooses one");
        assert_eq!(keep, 1, "the newest upload is the survivor");
        assert!(is_superseded_upload(&maps, keep, 0));
        assert!(!is_superseded_upload(&maps, keep, keep), "the survivor is never its own casualty");
    }

    /// A hand-copied `.obcm` is not the device's to reclaim, even when an upload is what loaded.
    #[test]
    fn a_side_loaded_map_is_never_superseded() {
        let maps = [side_loaded(true), uploaded(4, true)];
        let keep = choose_map(&maps).expect("a card with maps chooses one");
        assert_eq!(keep, 1);
        assert!(!is_superseded_upload(&maps, keep, 0), "the rule is one *uploaded* map, not one file");
    }

    /// Running a side-loaded map disarms the sweep: the uploads present are ones this build cannot
    /// read, and unreadable is not redundant.
    #[test]
    fn nothing_is_swept_while_a_side_loaded_map_is_loaded() {
        let maps = [uploaded(7, false), uploaded(8, false), side_loaded(true)];
        let keep = choose_map(&maps).expect("a card with maps chooses one");
        assert_eq!(keep, 2, "nothing readable is uploaded, so the side-loaded map loads");
        assert!(!is_superseded_upload(&maps, keep, 0));
        assert!(!is_superseded_upload(&maps, keep, 1));
    }

    /// An unreadable *upload* still supersedes older uploads once one of them is what loaded —
    /// clause 4's fault-screen case must not also mean "keep every wrong-version map forever".
    #[test]
    fn an_unreadable_upload_still_supersedes_older_uploads() {
        let maps = [uploaded(3, false), uploaded(9, false)];
        let keep = choose_map(&maps).expect("a card with maps chooses one");
        assert_eq!(keep, 0, "nothing readable → the first map");
        assert!(is_superseded_upload(&maps, keep, 1));
    }

    /// A volume set is one map like any other: a newer set supersedes the set it replaced. Its
    /// keeper is [`newest_set`], not the loaded map — a set is never loaded in this build, so
    /// keying its retirement on `choose_map`'s answer would make the whole pass unreachable.
    #[test]
    fn a_newer_set_supersedes_the_set_it_replaced() {
        // The reachable card: two sets plus a single map, which is the thing that actually loads.
        let maps = [uploaded_set(1), uploaded_set(2), uploaded(5, true)];
        let keep = choose_map(&maps).expect("a card with maps chooses one");
        assert_eq!(keep, 2, "the readable single map loads — a set reports `readable: false`");
        let survivor = newest_set(&maps).expect("a card with sets names a set survivor");
        assert_eq!(survivor, 1, "the highest MS id is the survivor");
        assert!(is_superseded_upload(&maps, survivor, 0), "MS1 was replaced by MS2");
        assert!(!is_superseded_upload(&maps, survivor, 1), "the survivor is not its own casualty");
        assert!(!is_superseded_upload(&maps, survivor, 2), "and a single map is never a set's casualty");

        // …and on a card holding *only* sets, where `choose_map` lands on clause 4 (index 0, the
        // older one). The set keeper is still the newest, so the retirement is right side up.
        let maps = [uploaded_set(1), uploaded_set(2)];
        assert_eq!(choose_map(&maps), Some(0), "clause 4 returns the first map, readable or not");
        let survivor = newest_set(&maps).expect("a card with sets names a set survivor");
        assert_eq!(survivor, 1);
        assert!(is_superseded_upload(&maps, survivor, 0));
        assert!(!is_superseded_upload(&maps, survivor, 1));

        // A card with no sets names no set survivor, so the pass does nothing at all.
        assert_eq!(newest_set(&[uploaded(1, true), side_loaded(true)]), None);
    }

    /// …but `MP{id}` and `MS{id}` number independently, so neither convention may sweep the other.
    /// The casualty here would be a deleted map the rider never replaced.
    #[test]
    fn a_set_and_a_single_map_never_supersede_each_other() {
        let maps = [uploaded(4, true), uploaded_set(7)];
        let keep = choose_map(&maps).expect("a card with maps chooses one");
        assert_eq!(keep, 0, "the readable single map loads");
        assert!(!is_superseded_upload(&maps, keep, 1), "MP4 does not retire MS7 — the ids are unrelated");
        // And the set's own keeper is itself, so it retires nothing either.
        let survivor = newest_set(&maps).expect("one set is its own survivor");
        assert_eq!(survivor, 1);
        assert!(!is_superseded_upload(&maps, survivor, 0), "and not in the other direction either");
    }

    /// The fault a card with no streamable map deserves. Clause 4's whole purpose is to reach the
    /// **MAP UNREADABLE** screen; only a card that really holds nothing gets **NO MAP**.
    #[test]
    fn a_card_holding_an_unusable_map_says_unreadable_not_absent() {
        assert_eq!(boot_fault(&[]), crate::BootFault::NoMap, "an empty catalog is the only NO MAP");

        // A card holding only a volume set: 8 GB that this build lists, sizes, and cannot mount.
        let maps = [uploaded_set(7)];
        assert_eq!(choose_map(&maps), Some(0), "clause 4 still returns it");
        assert_eq!(boot_fault(&maps), crate::BootFault::BadMap);

        // The pre-existing case the same rule already covered: a wrong-version single map.
        assert_eq!(boot_fault(&[uploaded(3, false)]), crate::BootFault::BadMap);
    }

    /// The safety-critical classification: a shard is a valid OBCM file and must still never be
    /// listed as a map (§5.4). Pure, so it is asserted here rather than only on a device.
    #[test]
    fn shards_are_never_maps_and_only_a_manifest_is_a_set() {
        use MapEntry::*;
        // Received single map, side-loaded long name, and the 8.3 twin.
        assert_eq!(classify_map_entry(b"MP7.OBM", None, false), Map);
        assert_eq!(classify_map_entry(b"BADENW~1.OBM", Some("baden-wuerttemberg.obcm"), false), Map);
        assert_eq!(classify_map_entry(b"ALPS.OBM", Some("Alps.OBM"), false), Map);

        // The manifest is the one file that says "these are one map".
        assert_eq!(classify_map_entry(b"MS7.OBS", None, false), SetManifest);
        assert_eq!(classify_map_entry(b"MS999.OBS", None, false), SetManifest);

        // Every shard of every set, at both ends of the id and index ranges.
        for name in [&b"MS7S00.OBM"[..], b"MS7S31.OBM", b"MS0S05.OBM", b"MS999S31.OBM"] {
            assert_eq!(classify_map_entry(name, None, false), Other, "{:?} is a shard, not a map", name);
        }

        // Clutter that only looks like one of the two conventions.
        assert_eq!(classify_map_entry(b"SET.OBS", None, false), Other, "an unparseable id is not a manifest");
        assert_eq!(classify_map_entry(b"MS07.OBS", None, false), Other, "leading zeros are not the §5.2 name");
        assert_eq!(
            classify_map_entry(b"MS7.OBM", None, false),
            Map,
            "the manifest extension is .OBS, so this is a map"
        );
        assert_eq!(classify_map_entry(b"NOTES.TXT", None, false), Other);
        assert_eq!(classify_map_entry(b"MAPS.OBM", None, true), Other, "a directory is never an entry");
        assert_eq!(classify_map_entry(b"_MP7.OBM", Some("._MP7.OBM"), false), Other, "dot-clutter is excluded");
        assert_eq!(classify_map_entry(b"MS7.OBS", Some(".MS7.OBS"), false), Other);
    }

    /// Out-of-range indices answer `false` rather than panicking: the board feeds this a live
    /// directory scan, and a card that changed under it must not fault the boot.
    #[test]
    fn an_index_off_the_end_supersedes_nothing() {
        let maps = [uploaded(1, true)];
        assert!(!is_superseded_upload(&maps, 0, 7));
        assert!(!is_superseded_upload(&maps, 7, 0));
        assert!(!is_superseded_upload(&[], 0, 0));
    }
}
