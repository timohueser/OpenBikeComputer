//! Allocation-free streamed map-scene contract.
//!
//! This crate owns only the semantic data and operations a base-map renderer needs: bounds,
//! styles, LOD selection, visible candidates, selected-feature decode, and optional streaming
//! diagnostics. It knows nothing about OBCM byte offsets, quadtrees, cache slots, or storage.
//!
//! The visitor methods are deliberately generic rather than object-safe. The production path is a
//! per-feature hot loop, so concrete sources are monomorphized and no dynamic dispatch is paid per
//! candidate. Tests can implement the same small trait over static slices.

#![no_std]
#![forbid(unsafe_code)]

use heapless::Vec;

/// Axis-aligned bounds in integer microdegrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BBox {
    pub min_lon: i32,
    pub min_lat: i32,
    pub max_lon: i32,
    pub max_lat: i32,
}

impl BBox {
    #[inline]
    pub fn intersects(&self, other: &BBox) -> bool {
        !(self.max_lon < other.min_lon
            || self.min_lon > other.max_lon
            || self.max_lat < other.min_lat
            || self.min_lat > other.max_lat)
    }
}

/// Geometry class required by the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Line,
    Polygon,
}

/// A style's boolean draw properties, **packed into one byte**.
///
/// Packed rather than a field each because a source keeps the whole table resident — the OBCM
/// reader holds `[Option<Style>; 256]` — so a `bool` field costs 256 bytes of the device's RAM
/// budget (plus alignment) to carry one bit per style. The values here are the seam's own; a source
/// translates its file's representation into them, exactly as it does for every other field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StyleFlags(u8);

impl StyleFlags {
    const DASHED: u8 = 1 << 0;
    const FIXED_WIDTH: u8 = 1 << 1;
    const TERRAIN_LAYER: u8 = 1 << 2;

    /// No property set: a solid, ramped, non-terrain style.
    pub const NONE: Self = Self(0);

    #[inline]
    pub const fn new(dashed: bool, fixed_width: bool, terrain_layer: bool) -> Self {
        let mut bits = 0;
        if dashed {
            bits |= Self::DASHED;
        }
        if fixed_width {
            bits |= Self::FIXED_WIDTH;
        }
        if terrain_layer {
            bits |= Self::TERRAIN_LAYER;
        }
        Self(bits)
    }

    /// Stroke this line dashed instead of solid. Ignored for polygons.
    #[inline]
    pub const fn dashed(self) -> bool {
        self.0 & Self::DASHED != 0
    }

    /// Use `weight` as the on-screen stroke in **device pixels**, verbatim: the renderer's
    /// zoom→width ramp does not apply. For a *mark on the map* — something with no width on the
    /// ground, like a contour — where the ramp is not merely wrong but backwards.
    #[inline]
    pub const fn fixed_width(self) -> bool {
        self.0 & Self::FIXED_WIDTH != 0
    }

    /// This style belongs to the **terrain layer**, the group a device setting may suppress
    /// wholesale. The renderer's collect pass reads it: with the terrain layer hidden
    /// (`RenderConfig { terrain_layer: false }`) a style carrying this bit is never admitted to the
    /// visible-style mask, so its features are not decoded at all.
    #[inline]
    pub const fn terrain_layer(self) -> bool {
        self.0 & Self::TERRAIN_LAYER != 0
    }
}

/// Complete draw metadata for one style. Sources keep the table; renderers borrow entries by id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub id: u8,
    pub z_index: i8,
    pub color: u16,
    pub weight: u8,
    pub priority: u8,
    pub flags: StyleFlags,
    pub color2: Option<u16>,
}

// A source holds 256 of these resident (`[Option<Style>; 256]` in the OBCM reader), so this struct
// is multiplied by 256 in the board's RAM budget — which is why the boolean properties live packed
// in [`StyleFlags`] and not one `bool` field each. Twelve bytes is what the fields need with no
// padding to spare; growing it is a budget decision, not a detail.
const _: () = assert!(core::mem::size_of::<Style>() <= 12, "Style is resident ×256 — see StyleFlags");

/// A source-defined identity for a candidate within one render.
///
/// Consumers must treat the three words as opaque: they exist only so an allocation-free source
/// can find a pass-A candidate again in pass B without publishing storage-format details. The
/// renderer copies the six-byte token into its existing span-sized stub and never interprets it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct FeatureToken([u16; 3]);

impl FeatureToken {
    /// Construct a token from source-private words. Only source adapters should call this.
    #[doc(hidden)]
    #[inline]
    pub const fn from_source_words(words: [u16; 3]) -> Self {
        Self(words)
    }

    /// Recover the source-private words. Renderers must not interpret them.
    #[doc(hidden)]
    #[inline]
    pub const fn source_words(self) -> [u16; 3] {
        self.0
    }
}

/// Complete decoded geometry borrowed from caller-owned scratch.
#[derive(Debug, Clone, Copy)]
pub struct Feature<'a> {
    pub style_id: u8,
    pub kind: Kind,
    points: &'a [(i32, i32)],
    ring_lens: &'a [usize],
    bbox: BBox,
}

impl<'a> Feature<'a> {
    #[inline]
    pub const fn new(style_id: u8, kind: Kind, points: &'a [(i32, i32)], ring_lens: &'a [usize], bbox: BBox) -> Self {
        Self { style_id, kind, points, ring_lens, bbox }
    }

    #[inline]
    pub const fn points(&self) -> &'a [(i32, i32)] {
        self.points
    }

    #[inline]
    pub const fn ring_lens(&self) -> &'a [usize] {
        self.ring_lens
    }

    #[inline]
    pub const fn bbox(&self) -> BBox {
        self.bbox
    }

    /// Whether the borrowed geometry is a complete, slice-safe feature.
    ///
    /// Every feature has at least one non-empty ring and the checked sum of ring lengths must
    /// exactly consume `points`. Renderers validate this at both pass-A reservation and pass-B
    /// publication, so a hostile source cannot publish short, trailing, or overflowing slices.
    #[inline]
    pub fn has_valid_rings(&self) -> bool {
        if self.points.is_empty() || self.ring_lens.is_empty() {
            return false;
        }
        if self.ring_lens.len() == 1 {
            return self.ring_lens[0] == self.points.len();
        }
        self.ring_lens.iter().try_fold(0usize, |sum, &len| if len == 0 { None } else { sum.checked_add(len) })
            == Some(self.points.len())
    }
}

/// A pass-A feature plus the opaque identity needed to request it in pass B.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub token: FeatureToken,
    pub feature: Feature<'a>,
}

/// Stream/index failures, kept distinct across the seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadError {
    Source,
    CacheBusy,
    Malformed,
}

/// Which caller-owned decode buffer rejected a complete feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityError {
    Points,
    Rings,
}

/// Failure to re-decode one selected feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureError {
    Capacity(CapacityError),
    Malformed,
    Read(ReadError),
}

/// Counted stream failures from one traversal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadFailures {
    pub source: u32,
    pub cache_busy: u32,
    pub malformed: u32,
}

impl ReadFailures {
    #[inline]
    pub fn record(&mut self, error: ReadError) {
        match error {
            ReadError::Source => self.source = self.source.saturating_add(1),
            ReadError::CacheBusy => self.cache_busy = self.cache_busy.saturating_add(1),
            ReadError::Malformed => self.malformed = self.malformed.saturating_add(1),
        }
    }
}

/// Outcome of the pass-A visible-candidate traversal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CandidateReport {
    pub chunks_visited: usize,
    pub capacity_dropped: u32,
    pub malformed_features: u32,
    pub read_failures: ReadFailures,
}

/// Outcome of the pass-B selected-feature traversal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecodeReport {
    pub chunks_refetched: u32,
    pub read_failures: ReadFailures,
}

/// Optional cumulative source diagnostics. Renderers snapshot this around collection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Diagnostics {
    pub chunk_hits: u32,
    pub chunk_misses: u32,
    pub source_reads: u32,
    pub bytes_read: u32,
}

/// Caller-owned selected-candidate state used by [`MapScene::decode_selected`].
///
/// The source asks for opaque tokens and publishes complete geometry back into the same state.
/// This lets it preserve its natural chunk-major streaming order without exposing chunks or
/// offsets and without allocating a second candidate list.
pub trait SelectedFeatures {
    fn len(&self) -> usize;
    #[inline]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Whether `index` is a live, unpublished selection. Returns `false` when out of range.
    fn is_pending(&self, index: usize) -> bool;
    /// The opaque token for a live selection, or `None` when out of range or already completed.
    fn token(&self, index: usize) -> Option<FeatureToken>;
    /// Publish one complete decode. Returns `true` only when `index` was live and the feature was
    /// validated and admitted. Out-of-range and duplicate publication are safe no-ops.
    fn decoded(&mut self, index: usize, feature: Feature<'_>) -> bool;
    /// Complete one live selection with an error. Returns `false` for out-of-range or duplicate
    /// completion; in those cases no renderer state or diagnostic counter is touched.
    fn failed(&mut self, index: usize, error: FeatureError) -> bool;
}

/// Minimal allocation-free base-map scene consumed by the renderer.
pub trait MapScene {
    fn lod_count(&self) -> usize;
    fn select_lod_for_mpp(&self, mpp: f32) -> usize;
    fn style(&self, id: u8) -> Option<&Style>;

    /// RGB565 marker colour stored with the map presentation metadata.
    fn marker_color(&self) -> u16 {
        0
    }

    /// The style at the bottom of the paint order, used to clear the map plane before geometry.
    /// Sources with a pre-resolved backdrop override this; the allocation-free fallback scans the
    /// bounded 256-entry style id space.
    fn backdrop_style(&self) -> Option<&Style> {
        (0..=u8::MAX).filter_map(|id| self.style(id)).min_by_key(|style| (style.z_index, style.id))
    }

    /// Snapshot optional cumulative source/cache counters. `Ok(None)` means the source has none.
    fn diagnostics(&self) -> Result<Option<Diagnostics>, ReadError> {
        Ok(None)
    }

    /// Visit every decoded, style-filtered candidate overlapping the visible source groups.
    #[allow(clippy::too_many_arguments)]
    fn visit_candidates<const P: usize, const R: usize>(
        &self,
        lod: usize,
        view: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        should_decode: impl Fn(u8) -> bool,
        visit: impl FnMut(Candidate<'_>),
    ) -> CandidateReport;

    /// Re-decode the selected candidates into the same caller-owned scratch and publish complete
    /// features through `selected`. Sources retain their natural streaming/cache order.
    fn decode_selected<const P: usize, const R: usize>(
        &self,
        lod: usize,
        view: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        selected: &mut impl SelectedFeatures,
    ) -> DecodeReport;
}

/// Meters of ground per degree in the shared local-equirectangular Earth model.
pub const M_PER_DEG: f64 = 111_320.0;

#[inline]
pub fn cos_lat(lat_ud: i32) -> f32 {
    libm::cosf((lat_ud as f32 / 1e6).to_radians())
}

#[inline]
pub fn delta_m(from: (i32, i32), to: (i32, i32), cl: f32) -> (f32, f32) {
    let dlon = (to.0 - from.0) as f32 * 1e-6;
    let dlat = (to.1 - from.1) as f32 * 1e-6;
    (dlon * M_PER_DEG as f32 * cl, dlat * M_PER_DEG as f32)
}

#[inline]
pub fn ground_dist_m_cl(a: (i32, i32), b: (i32, i32), cl: f32) -> f32 {
    let (mx, my) = delta_m(a, b, cl);
    libm::sqrtf(mx * mx + my * my)
}

#[inline]
pub fn ground_dist_m(a: (i32, i32), b: (i32, i32)) -> f32 {
    ground_dist_m_cl(a, b, cos_lat(a.1))
}
