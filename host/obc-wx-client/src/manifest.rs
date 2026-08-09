//! The client's view of `wx/v1/manifest.json` (OBCG §10).
//!
//! Deliberately an **independent** model, not a re-export of `obc-wx-bake`'s: the baker's structs
//! are `deny_unknown_fields` writers, and a client that refuses a document because the service
//! grew a field would turn every future baker deploy into an outage. The normative contract is
//! the checked-in JSON Schema; this is the second implementation of it, exactly as the Swift
//! client is the third. A dev-dependency test feeds a baker-written manifest through this parser
//! so the two can never silently diverge.
//!
//! Strictness is split the same way the phone splits it, and for the same reason:
//!
//! - **the document is strict** — unparseable JSON or an unknown `version` is a hard failure;
//! - **an entry is lenient** — a malformed product is *skipped and counted*, never fatal, so one
//!   bad adapter cannot take the whole service down for every rider.

use serde::Deserialize;

pub const MANIFEST_KEY: &str = "wx/v1/manifest.json";
pub const MANIFEST_VERSION: u32 = 1;

/// OBCG §10: the manifest caches for at most 60 s.
pub const FRESHNESS_WINDOW_S: i64 = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    Malformed(String),
    UnsupportedVersion(u32),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Malformed(why) => write!(f, "malformed manifest: {why}"),
            ManifestError::UnsupportedVersion(v) => write!(f, "unsupported manifest version {v}"),
        }
    }
}

// ── validated model ────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bbox {
    pub south_udeg: i64,
    pub west_udeg: i64,
    pub north_udeg: i64,
    pub east_udeg: i64,
}

impl Bbox {
    pub fn is_well_formed(&self) -> bool {
        self.south_udeg < self.north_udeg
            && self.west_udeg < self.east_udeg
            && (-90_000_000..=90_000_000).contains(&self.south_udeg)
            && (-90_000_000..=90_000_000).contains(&self.north_udeg)
            && (-180_000_000..=180_000_000).contains(&self.west_udeg)
            && (-180_000_000..=180_000_000).contains(&self.east_udeg)
    }

    /// Closed containment on all four edges — an exact fit counts as covered. Partial overlap
    /// does **not**: a corridor half outside a product is not answerable by it.
    pub fn contains(&self, other: &Bbox) -> bool {
        other.south_udeg >= self.south_udeg
            && other.north_udeg <= self.north_udeg
            && other.west_udeg >= self.west_udeg
            && other.east_udeg <= self.east_udeg
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceClass {
    Observation,
    Forecast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub south_udeg: i32,
    pub west_udeg: i32,
    pub cell_lat_udeg: u32,
    pub cell_lon_udeg: u32,
    pub width: u32,
    pub height: u32,
    pub cell_size_m: u16,
    pub tile_edge: u16,
    pub entries_per_page: u16,
}

impl Geometry {
    pub fn bounds(&self) -> Bbox {
        Bbox {
            south_udeg: i64::from(self.south_udeg),
            west_udeg: i64::from(self.west_udeg),
            north_udeg: i64::from(self.south_udeg) + i64::from(self.height) * i64::from(self.cell_lat_udeg),
            east_udeg: i64::from(self.west_udeg) + i64::from(self.width) * i64::from(self.cell_lon_udeg),
        }
    }

    /// The OBCG §1/§3 limits, checked from the manifest **before** a single byte is fetched — a
    /// frame the header could only reject is not worth a Range read.
    fn is_valid(&self) -> bool {
        self.cell_lat_udeg > 0
            && self.cell_lon_udeg > 0
            && self.width > 0
            && self.height > 0
            && self.width <= obc_formats::obcg::MAX_GRID_DIM
            && self.height <= obc_formats::obcg::MAX_GRID_DIM
            && u64::from(self.width) * u64::from(self.height) <= obc_formats::obcg::MAX_GRID_CELLS
            && self.cell_size_m > 0
            && self.tile_edge >= obc_formats::obcg::MIN_TILE_EDGE
            && self.tile_edge <= obc_formats::obcg::MAX_TILE_EDGE
            && self.tile_edge.is_power_of_two()
            && self.entries_per_page > 0
            && self.entries_per_page <= obc_formats::obcg::MAX_ENTRIES_PER_PAGE
            && self.bounds().is_well_formed()
    }

    /// Does the fetched OBCG header say what the manifest promised? A manifest that re-stamped a
    /// frame to look current is caught here, before any cell is trusted.
    pub fn agrees_with(&self, header: &obc_formats::obcg::Header) -> bool {
        self.south_udeg == header.south_lat_udeg
            && self.west_udeg == header.west_lon_udeg
            && self.cell_lat_udeg == header.cell_lat_udeg
            && self.cell_lon_udeg == header.cell_lon_udeg
            && self.width == header.width
            && self.height == header.height
            && self.cell_size_m == header.cell_size_m
            && self.tile_edge == header.tile_edge
            && self.entries_per_page == header.entries_per_page
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub offset_min: u32,
    pub valid_at: i64,
    pub source_class: SourceClass,
    pub key: String,
    pub bytes: u64,
    pub object_crc32: u32,
    pub geometry: Geometry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribution {
    pub text: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Product {
    pub id: String,
    pub tier: u8,
    pub bounds: Bbox,
    pub cell_lat_udeg: u32,
    pub cell_lon_udeg: u32,
    pub nominal_m: u16,
    pub reference_time: i64,
    pub generated_at: i64,
    pub staleness_deadline: i64,
    pub attribution: Attribution,
    pub frames: Vec<Frame>,
}

impl Product {
    /// Inclusive: a product is usable up to and including its deadline second.
    pub fn is_fresh(&self, now: i64) -> bool {
        now <= self.staleness_deadline
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub generated_at: i64,
    pub products: Vec<Product>,
    /// Entries the parser refused. Evidence for the diagnostics panel, never control flow.
    pub skipped_products: usize,
}

// ── wire model ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct WireManifest {
    version: u32,
    generated_at: String,
    products: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct WireProduct {
    id: String,
    tier: u8,
    bbox_udeg: WireBbox,
    cell: WireCell,
    reference_time: String,
    generated_at: String,
    staleness_deadline: String,
    attribution: Attribution2,
    frames: Vec<WireFrame>,
}

#[derive(Deserialize)]
struct WireBbox {
    south_udeg: i64,
    west_udeg: i64,
    north_udeg: i64,
    east_udeg: i64,
}

#[derive(Deserialize)]
struct WireCell {
    lat_udeg: u32,
    lon_udeg: u32,
    nominal_m: u16,
}

#[derive(Deserialize)]
struct Attribution2 {
    text: String,
    url: String,
}

#[derive(Deserialize)]
struct WireFrame {
    offset_min: u32,
    valid_at: String,
    source_class: String,
    key: String,
    bytes: u64,
    object_crc32: String,
    geometry: WireGeometry,
}

#[derive(Deserialize)]
struct WireGeometry {
    south_udeg: i32,
    west_udeg: i32,
    cell_lat_udeg: u32,
    cell_lon_udeg: u32,
    width: u32,
    height: u32,
    cell_size_m: u16,
    tile_edge: u16,
    entries_per_page: u16,
}

/// RFC 3339 seconds, the one timestamp shape the manifest uses.
pub fn parse_rfc3339(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text).ok().map(|time| time.timestamp())
}

pub fn parse(bytes: &[u8]) -> Result<Manifest, ManifestError> {
    let wire: WireManifest =
        serde_json::from_slice(bytes).map_err(|error| ManifestError::Malformed(error.to_string()))?;
    if wire.version != MANIFEST_VERSION {
        return Err(ManifestError::UnsupportedVersion(wire.version));
    }
    let generated_at =
        parse_rfc3339(&wire.generated_at).ok_or_else(|| ManifestError::Malformed("generated_at".into()))?;
    let mut products = Vec::new();
    let mut skipped_products = 0usize;
    for value in wire.products {
        match serde_json::from_value::<WireProduct>(value).ok().and_then(validate_product) {
            Some(product) => products.push(product),
            None => skipped_products += 1,
        }
    }
    Ok(Manifest { generated_at, products, skipped_products })
}

fn validate_product(wire: WireProduct) -> Option<Product> {
    // Tier 0 is the OBCG registry's "invalid"; an unknown *nonzero* tier is fine and simply
    // orders after the known ones — adding a tier must never need a client release.
    if wire.tier == 0 || wire.frames.is_empty() {
        return None;
    }
    let bounds = Bbox {
        south_udeg: wire.bbox_udeg.south_udeg,
        west_udeg: wire.bbox_udeg.west_udeg,
        north_udeg: wire.bbox_udeg.north_udeg,
        east_udeg: wire.bbox_udeg.east_udeg,
    };
    if !bounds.is_well_formed() || wire.cell.lat_udeg == 0 || wire.cell.lon_udeg == 0 {
        return None;
    }
    let reference_time = parse_rfc3339(&wire.reference_time)?;
    let generated_at = parse_rfc3339(&wire.generated_at)?;
    let staleness_deadline = parse_rfc3339(&wire.staleness_deadline)?;
    let mut frames = Vec::with_capacity(wire.frames.len());
    for frame in wire.frames {
        frames.push(validate_frame(frame)?);
    }
    // §10: frames are a timeline. Out-of-order or duplicated timestamps would make the OBCW
    // re-encode (which requires strictly increasing `valid_at`) unbuildable later, so refuse now.
    if frames.windows(2).any(|pair| pair[1].valid_at <= pair[0].valid_at) {
        return None;
    }
    // §10: the product bbox is the intersection of its frames' windows.
    if frames.iter().any(|frame| !frame.geometry.bounds().contains(&bounds)) {
        return None;
    }
    Some(Product {
        id: wire.id,
        tier: wire.tier,
        bounds,
        cell_lat_udeg: wire.cell.lat_udeg,
        cell_lon_udeg: wire.cell.lon_udeg,
        nominal_m: wire.cell.nominal_m,
        reference_time,
        generated_at,
        staleness_deadline,
        attribution: Attribution { text: wire.attribution.text, url: wire.attribution.url },
        frames,
    })
}

fn validate_frame(wire: WireFrame) -> Option<Frame> {
    let valid_at = parse_rfc3339(&wire.valid_at)?;
    let source_class = match wire.source_class.as_str() {
        "observation" => SourceClass::Observation,
        "forecast" => SourceClass::Forecast,
        _ => return None,
    };
    let hex = wire.object_crc32.strip_prefix("0x")?;
    let object_crc32 = u32::from_str_radix(hex, 16).ok()?;
    if wire.bytes == 0 || wire.bytes > i32::MAX as u64 {
        return None;
    }
    // Keys are joined onto the service origin; a leading slash or a `..` segment would let a
    // manifest steer the client off its own prefix.
    if wire.key.is_empty() || wire.key.starts_with('/') || wire.key.contains("..") {
        return None;
    }
    let geometry = Geometry {
        south_udeg: wire.geometry.south_udeg,
        west_udeg: wire.geometry.west_udeg,
        cell_lat_udeg: wire.geometry.cell_lat_udeg,
        cell_lon_udeg: wire.geometry.cell_lon_udeg,
        width: wire.geometry.width,
        height: wire.geometry.height,
        cell_size_m: wire.geometry.cell_size_m,
        tile_edge: wire.geometry.tile_edge,
        entries_per_page: wire.geometry.entries_per_page,
    };
    if !geometry.is_valid() {
        return None;
    }
    Some(Frame {
        offset_min: wire.offset_min,
        valid_at,
        source_class,
        key: wire.key,
        bytes: wire.bytes,
        object_crc32,
        geometry,
    })
}
