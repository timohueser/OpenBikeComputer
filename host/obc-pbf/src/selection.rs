//! Completed source object IDs for an explicit crop.

/// A grow-then-freeze set of OSM ids, backed by a sorted `Vec`: 8 flat bytes and a binary search
/// instead of a `HashSet`'s per-entry overhead, because these sets are the memory floor of a
/// `--bbox` run over a large source. Each set is filled in one pass and read in a later one, and
/// `contains` on an unfrozen set would silently lie, so freezing is the type's one rule.
#[derive(Default)]
pub struct IdSet(Vec<i64>);

impl IdSet {
    pub fn push(&mut self, id: i64) {
        self.0.push(id);
    }

    /// Take another source's ids wholesale, moving the first batch instead of copying it.
    pub fn absorb(&mut self, mut ids: Vec<i64>) {
        if self.0.is_empty() {
            self.0 = ids;
        } else {
            self.0.append(&mut ids);
        }
    }

    /// End the fill phase. Idempotent, so pass 0 can freeze the node set early (the first way
    /// needs it) and freeze the rest at the end.
    pub fn freeze(&mut self) {
        self.0.sort_unstable();
        self.0.dedup();
        self.0.shrink_to_fit();
    }

    #[inline]
    pub fn contains(&self, id: i64) -> bool {
        self.0.binary_search(&id).is_ok()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_ids(self) -> Vec<i64> {
        self.0
    }
}

/// The id sets that define a `--bbox` crop.
pub struct Crop {
    /// Nodes whose location falls inside the box.
    inside: IdSet,
    /// Nodes outside the box that a kept way still references — the halo that keeps
    /// boundary-crossing ways whole.
    halo: IdSet,
    /// Ways with at least one node inside the box, plus every member way of a renderable area
    /// relation touched by one of those ways.
    ways: IdSet,
    /// Renderable area relations reached from a way touching the box.
    relations: IdSet,
}

impl Crop {
    pub fn new(inside: IdSet, halo: IdSet, ways: IdSet, relations: IdSet) -> Self {
        Self { inside, halo, ways, relations }
    }

    /// Nodes the extract would contain: inside the box, or needed by a kept way.
    #[inline]
    pub fn keeps_node(&self, id: i64) -> bool {
        self.inside.contains(id) || self.halo.contains(id)
    }

    #[inline]
    pub fn keeps_way(&self, id: i64) -> bool {
        self.ways.contains(id)
    }

    #[inline]
    pub fn keeps_relation(&self, id: i64) -> bool {
        self.relations.contains(id)
    }

    /// Nothing inside the box and no way reaching into it — the caller should fail loudly rather
    /// than pack an empty map.
    pub fn is_empty(&self) -> bool {
        self.inside.is_empty() && self.ways.is_empty()
    }
}
