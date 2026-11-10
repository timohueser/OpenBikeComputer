//! The caps that a build and the CPU height function hit, counted per thread for the native census
//! (the binary `census` of `obc-trailview-features`). The WASM build counts nothing: there `hit` and
//! `field` are empty.

/// The fields that the records pack into 16 bits (8 for a junction's arms), and the leg index that a
/// `u8` holds: `field` keeps the largest value of each since the last `take`. A value past `limit`
/// wrapped or tripped an assert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// A candidate's index in a bin's candidate item.
    Candidate,
    /// The candidate items of a bin.
    BinCandidates,
    /// The segment items of a bin.
    BinSegments,
    /// The height segment items of a bin's window.
    HeightSegments,
    /// The height disc items of a bin's window.
    HeightDiscs,
    /// The item words of a bin (asserted).
    BinItems,
    /// The outline edges of a cell (asserted).
    OutlineEdges,
    /// The cuts before a point of a line: its leg.
    Leg,
    /// A record line's index in a segment item or an outline edge's host (asserted there).
    Line,
    /// A sample's index in a segment item.
    Sample,
    /// A disc's index in a height disc item.
    Disc,
    /// The sample offsets from its leg's start of a junction arm.
    Span,
    /// The arms of a junction record (8 bits, asserted).
    JunctionArms,
    /// A junction record's id (asserted).
    JunctionId,
    /// The blend edges of a bin's window (asserted).
    BlendEdges,
}

/// The fields in code order, with their names.
pub const FIELDS: [(Field, &str); 15] = [
    (Field::Candidate, "candidate"),
    (Field::BinCandidates, "bin_candidates"),
    (Field::BinSegments, "bin_segments"),
    (Field::HeightSegments, "height_segments"),
    (Field::HeightDiscs, "height_discs"),
    (Field::BinItems, "bin_items"),
    (Field::OutlineEdges, "outline_edges"),
    (Field::Leg, "leg"),
    (Field::Line, "line"),
    (Field::Sample, "sample"),
    (Field::Disc, "disc"),
    (Field::Span, "span"),
    (Field::JunctionArms, "junction_arms"),
    (Field::JunctionId, "junction_id"),
    (Field::BlendEdges, "blend_edges"),
];

impl Field {
    /// The largest value that the field holds.
    pub fn limit(self) -> u64 {
        match self {
            Field::Leg | Field::JunctionArms => u64::from(u8::MAX),
            _ => u64::from(u16::MAX),
        }
    }
}

/// A cap that leaves something out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cap {
    /// More than `heights::SLOTS` lines at a point: the last in the painter's order went.
    Slots,
    /// A line with more than `heights::QUADS` quads within its leg reach at a point: one went.
    Quads,
    /// A third region beyond a junction region's edges near a point: the farthest went.
    Regions,
    /// i_overlay gave a slice tile no graph: the whole tile is one patch.
    SliceFallback,
}

/// The counts of a thread since its last `take`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    pub slots: u64,
    pub quads: u64,
    pub regions: u64,
    pub slice_fallback: u64,
    /// The largest value of each field, in the order of `FIELDS`.
    pub fields: [u64; FIELDS.len()],
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static PROBE: std::cell::Cell<Probe> = std::cell::Cell::new(Probe::default());
}

#[inline]
pub fn hit(cap: Cap) {
    #[cfg(not(target_arch = "wasm32"))]
    PROBE.with(|p| {
        let mut v = p.get();
        let n = match cap {
            Cap::Slots => &mut v.slots,
            Cap::Quads => &mut v.quads,
            Cap::Regions => &mut v.regions,
            Cap::SliceFallback => &mut v.slice_fallback,
        };
        *n = n.saturating_add(1);
        p.set(v);
    });
    #[cfg(target_arch = "wasm32")]
    let _ = cap;
}

#[inline]
pub fn field(field: Field, value: usize) {
    #[cfg(not(target_arch = "wasm32"))]
    PROBE.with(|p| {
        let mut v = p.get();
        v.fields[field as usize] = v.fields[field as usize].max(value as u64);
        p.set(v);
    });
    #[cfg(target_arch = "wasm32")]
    let _ = (field, value);
}

/// The counts since the last call, and a fresh start.
pub fn take() -> Probe {
    #[cfg(not(target_arch = "wasm32"))]
    return PROBE.with(|p| p.replace(Probe::default()));
    #[cfg(target_arch = "wasm32")]
    Probe::default()
}
