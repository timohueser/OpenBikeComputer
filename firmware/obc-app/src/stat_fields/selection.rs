/// The rider's ordered field selection — a fixed-capacity list (no alloc) that is the POD persisted
/// in [`Settings`](crate::Settings). `Copy + Eq` so a settings edit is caught by one `==` (the same
/// trick the rest of [`Settings`](crate::Settings) uses). Slots past `len` are unused padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatFieldList {
    ids: [StatField; MAX_STAT_FIELDS],
    len: u8,
}

impl StatFieldList {
    /// The [`Default`] grid as a `const` — one link in the const [`Settings::DEFAULT`]
    /// (`crate::settings`) chain, which exists so the board can build its object store from a
    /// `.rodata` image instead of a stack temporary (the #1197 boot-chain fix). Byte-identical to
    /// the old push-built default: the unused tail slots keep the `Speed` fill the seed array had
    /// (pinned by test).
    ///
    /// [`Settings::DEFAULT`]: crate::settings::Settings::DEFAULT
    pub const DEFAULT: StatFieldList = StatFieldList {
        ids: {
            let mut ids = [StatField::Speed; MAX_STAT_FIELDS];
            ids[1] = StatField::AvgSpeed;
            ids[2] = StatField::DistDone;
            ids[3] = StatField::DistToGo;
            ids[4] = StatField::Climbed;
            ids[5] = StatField::ToClimb;
            ids
        },
        len: 6,
    };
}

impl Default for StatFieldList {
    /// The classic six single-column tiles, in their original order — so an un-customized device
    /// (and a settings reset) shows exactly today's grid.
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl StatFieldList {
    /// The selected fields, in display order.
    pub fn as_slice(&self) -> &[StatField] {
        &self.ids[..self.len as usize]
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether `f` is already shown (so the picker can offer only the rest).
    pub fn contains(&self, f: StatField) -> bool {
        self.as_slice().contains(&f)
    }

    /// Append `f` to the end of the selection, or do nothing when full / already present. Returns
    /// whether it was added.
    pub fn push(&mut self, f: StatField) -> bool {
        if self.len as usize >= MAX_STAT_FIELDS || self.contains(f) {
            return false;
        }
        self.ids[self.len as usize] = f;
        self.len += 1;
        true
    }

    /// Remove the field at `i`, shifting the rest down.
    pub fn remove(&mut self, i: usize) {
        if i >= self.len as usize {
            return;
        }
        for k in i..self.len as usize - 1 {
            self.ids[k] = self.ids[k + 1];
        }
        self.len -= 1;
    }

    /// Pack into a length byte + [`MAX_STAT_FIELDS`] discriminant bytes (unused slots filled with
    /// the padding discriminant) — the fixed-width form the settings codec embeds.
    pub fn encode(&self) -> (u8, [u8; MAX_STAT_FIELDS]) {
        let mut ids = [0u8; MAX_STAT_FIELDS];
        for (b, f) in ids.iter_mut().zip(self.ids.iter()) {
            *b = *f as u8;
        }
        (self.len, ids)
    }

    /// Rebuild from a length byte + discriminant bytes, **sanitising** as it goes: the length is
    /// clamped, unknown discriminants are dropped, and duplicates are coalesced (via [`push`]) — so a
    /// valid-CRC-but-stale blob can never load a garbage or contradictory selection.
    pub fn decode(len: u8, ids: &[u8]) -> StatFieldList {
        let mut list = StatFieldList { ids: [StatField::Speed; MAX_STAT_FIELDS], len: 0 };
        let n = (len as usize).min(MAX_STAT_FIELDS).min(ids.len());
        for &b in &ids[..n] {
            if let Some(f) = StatField::from_u8(b) {
                let _ = list.push(f);
            }
        }
        list
    }

    /// Move the field at `i` one valid step in `dir` (`+1` down / `-1` up), returning its new index
    /// (unchanged if it can't move further). A single-span field moves one slot at a time; a
    /// **two-span** field only lands where it begins a row, and the page-sized **panel** only where
    /// it begins a page — so a wide tile hops over a pair of singles (or one wide tile) per step, and
    /// the panel hops a whole page. The rule is one slot-simulation: for each candidate insertion
    /// index, [`landing_slot`](Self::landing_slot) walks the *other* fields (with their own bumps) to
    /// the slot the moved field would start at, and the step is valid iff the field needs no bump of
    /// its own there ([`placed_slot`] is the identity) — subsuming the old even-singles-before rule.
    pub fn move_item(&mut self, i: usize, dir: i32) -> usize {
        let len = self.len as usize;
        if len == 0 || dir == 0 {
            return i.min(len.saturating_sub(1));
        }
        let i = i.min(len - 1);
        let f = self.ids[i];
        let step = dir.signum();
        // Candidate insertion indices in `dir`; skip past any index where the moved field would need
        // its own alignment bump (a wide tile landing mid-row, the panel landing mid-page).
        let mut p = i as i32;
        loop {
            let cand = p + step;
            if cand < 0 || cand as usize >= len {
                return i; // hit an end without a valid landing → no move
            }
            let slot = self.landing_slot(i, cand as usize);
            if placed_slot(slot, f) == slot {
                self.shift(i, cand as usize);
                return cand as usize;
            }
            p = cand;
        }
    }

    /// The slot the field currently at `from` would start at if reordered to insertion index `to`:
    /// walk the *other* fields in order (each bumped to its own alignment by [`placed_slot`]) and
    /// stop once `to` of them are placed — the accumulated slot is where the moved field then lands,
    /// before any bump of its own. The reorder-time mirror of [`walk`], sharing its `placed_slot`
    /// spine so a proposed landing can never disagree with where the grid would actually draw it.
    fn landing_slot(&self, from: usize, to: usize) -> usize {
        let mut slot = 0usize;
        let mut placed = 0usize;
        for (k, &g) in self.as_slice().iter().enumerate() {
            if k == from {
                continue; // the moved field isn't part of what precedes it
            }
            if placed == to {
                break; // `to` other fields now sit before the insertion point
            }
            slot = placed_slot(slot, g) + g.slots();
            placed += 1;
        }
        slot
    }

    /// Move the item from index `from` to index `to` by rotating the span between them — an
    /// order-preserving shift, not a swap, so the passed-over fields keep their relative order.
    fn shift(&mut self, from: usize, to: usize) {
        if from == to {
            return;
        }
        let f = self.ids[from];
        if to > from {
            for k in from..to {
                self.ids[k] = self.ids[k + 1];
            }
        } else {
            for k in (to + 1..=from).rev() {
                self.ids[k] = self.ids[k - 1];
            }
        }
        self.ids[to] = f;
    }
}

/// A field placed in the grid: which field, and its top-left cell (`col` ∈ `0..COLS`,
/// `row` ∈ `0..ROWS_PER_PAGE`) on its page. The Statistics screen turns this into pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub field: StatField,
    pub col: u8,
    pub row: u8,
}

/// The slot `f` actually starts at when a left-to-right walk reaches it at `slot`, bumped forward so
/// it never straddles a row or page: a **single** fills any slot (no bump); a **two-span** tile that
/// would start in the right column is bumped to the next row; the page-sized **panel** is bumped to
/// the next page ([`slot.next_multiple_of(SLOTS_PER_PAGE)`](usize::next_multiple_of)). The single
/// alignment rule shared by the layout [`walk`] and [`StatFieldList::move_item`]'s landing check, so
/// a reorder can never propose a slot the walk would then shift out from under it.
fn placed_slot(slot: usize, f: StatField) -> usize {
    if f.slots() == SLOTS_PER_PAGE {
        slot.next_multiple_of(SLOTS_PER_PAGE) // the panel begins a page
    } else if f.span() == 2 && !slot.is_multiple_of(COLS) {
        slot + 1 // defensive: a malformed list can't mis-render — the wide tile begins a row
    } else {
        slot
    }
}

/// Walk the selection into global slots, calling `visit(field, slot)` for each. Every field is
/// placed at its [`placed_slot`] (bumped so a wide tile begins a row and the panel begins a page,
/// leaving a defensive gap) and then advances the cursor by its [`slots`](StatField::slots)
/// footprint. Because rows align to the [`SLOTS_PER_PAGE`] page, a bumped wide tile never straddles a
/// page either. Returns the total slots consumed (gaps included). Pure spine shared by
/// [`page_count`] / [`page_fields`] / [`slot_of`] / [`next_free_slot`].
fn walk(list: &StatFieldList, mut visit: impl FnMut(StatField, usize)) -> usize {
    let mut slot = 0usize;
    for &f in list.as_slice() {
        slot = placed_slot(slot, f);
        visit(f, slot);
        slot += f.slots();
    }
    slot
}

/// Number of pages the selection fills (at least `1`, even when empty — the grid draws nothing but
/// the page is still "page 0").
pub fn page_count(list: &StatFieldList) -> usize {
    let slots = walk(list, |_, _| {});
    slots.div_ceil(SLOTS_PER_PAGE).max(1)
}

/// The fields placed on `page` (clamped to the last page), with their on-page cells. At most
/// [`SLOTS_PER_PAGE`] entries.
pub fn page_fields(list: &StatFieldList, page: usize) -> heapless::Vec<Placed, SLOTS_PER_PAGE> {
    let page = page.min(page_count(list) - 1);
    let mut out = heapless::Vec::new();
    walk(list, |f, slot| {
        if slot / SLOTS_PER_PAGE == page {
            let s = slot % SLOTS_PER_PAGE;
            let _ = out.push(Placed { field: f, col: (s % COLS) as u8, row: (s / COLS) as u8 });
        }
    });
    out
}

/// The global slot the `index`-th selected field starts at (`None` past the selection) — the same
/// walk [`page_fields`] places with, so a cursor mapped through this always agrees with the drawn
/// grid. `slot / SLOTS_PER_PAGE` is the page, the remainder the on-page cell.
pub fn slot_of(list: &StatFieldList, index: usize) -> Option<usize> {
    let mut found = None;
    let mut i = 0usize;
    walk(list, |_, slot| {
        if i == index {
            found = Some(slot);
        }
        i += 1;
    });
    found
}

/// The first slot past the selection (gaps included) — where the Fields editor's ghost "add"
/// tile lands.
pub fn next_free_slot(list: &StatFieldList) -> usize {
    walk(list, |_, _| {})
}
