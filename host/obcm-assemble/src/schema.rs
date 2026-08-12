//! The two documents an assembly is driven by: the **schema** (what the cells were baked at) and
//! the **skin** (what the output looks like) — [`OBCC_Spec.md`](../../../specs/OBCC_Spec.md) §4
//! and §11.4, restated as the engine's input types.
//!
//! The split is the whole point of the epic and is byte-level, not editorial
//! ([`OBCA_Spec.md`](../../../specs/OBCA_Spec.md) §6.2): the **schema owns the style ids**, because
//! `obc-pack` numbers feature types 1-based in config order and every feature header in every chunk
//! references those numbers. A skin may change only the other seven bytes of each 8-byte record plus
//! the header's marker colour. That is why a restyle costs ~2 KB of output and no re-bake.

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};

use crate::grid::{MAX_CELL_LOG2, MIN_CELL_LOG2};

/// Which physical file of a volume set a band's content is assembled into (OBCA §5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BandRole {
    /// The one file that cannot be split by bbox: nav graph, POIs, style table.
    Core,
    /// The single whole-assembly shard carrying the coarsest LODs.
    Coarse,
    /// An ordinary splittable geometry shard.
    Geometry,
}

impl BandRole {
    /// The `Role` byte of an OBCS shard record (OBCA §5.2).
    pub fn wire(self) -> u8 {
        match self {
            BandRole::Core => 0,
            BandRole::Geometry => 1,
            BandRole::Coarse => 2,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            BandRole::Core => "core",
            BandRole::Coarse => "coarse",
            BandRole::Geometry => "geometry",
        }
    }
}

/// One band: a named class of cell content with one cell size (OBCA §1.2). The JSON shape is
/// OBCC §4's `bands` entry verbatim, so a catalog's own schema can be handed straight in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Band {
    pub id: String,
    /// Cell size, `log2(µdeg)`.
    pub cell_log2: u32,
    /// Ladder LOD indices this band's cells carry; every other LOD is written empty (§3.1).
    #[serde(default)]
    pub lods: Vec<usize>,
    /// Non-geometry sections this band carries: `"nav"` and/or `"poi"`.
    #[serde(default)]
    pub sections: Vec<String>,
    pub role: BandRole,
}

impl Band {
    pub fn has_nav(&self) -> bool {
        self.sections.iter().any(|s| s == "nav")
    }
    pub fn has_poi(&self) -> bool {
        self.sections.iter().any(|s| s == "poi")
    }
}

/// One ladder level (OBCC §4): its index, its `Max Meters/Pixel` (`null` ⇒ the `+inf` coarsest
/// level), and the band that carries it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LodEntry {
    pub index: usize,
    #[serde(default)]
    pub max_mpp: Option<f64>,
    #[serde(default)]
    pub band: String,
}

/// The canonical style-id assignment: which feature type owns which style id. Part of the schema
/// because the ids are in the cells' chunk bytes (OBCA §6.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleId {
    pub id: u8,
    pub feature_type: String,
}

/// The schema's routing facts. `min_component_edges` is the island-prune threshold the **assembler**
/// applies (OBCA §3.5/§4.6.4) — schema data, never skin data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Routing {
    pub min_component_edges: usize,
    #[serde(default)]
    pub profiles: Vec<String>,
}

impl Default for Routing {
    fn default() -> Self {
        Routing { min_component_edges: 50, profiles: Vec::new() }
    }
}

/// A schema revision: everything a producer must agree on for chunk bytes to mean the same thing in
/// two files (OBCC §4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub revision: u32,
    #[serde(default = "default_obcm_version")]
    pub obcm_version: u8,
    pub lods: Vec<LodEntry>,
    pub bands: Vec<Band>,
    #[serde(default)]
    pub styles: Vec<StyleId>,
    #[serde(default)]
    pub routing: Routing,
    #[serde(default)]
    pub chunk_size: usize,
}

fn default_obcm_version() -> u8 {
    obc_formats::obcm::VERSION
}

impl Schema {
    /// Parse a schema document. Accepts either a bare `SchemaEntry` or an OBCC v2 root
    /// (`{"schema": {...}}`), because both are things a caller has in hand.
    pub fn parse(text: &str) -> Result<Schema, String> {
        #[derive(Deserialize)]
        struct Root {
            schema: Schema,
        }
        if let Ok(root) = serde_json::from_str::<Root>(text) {
            return Ok(root.schema);
        }
        serde_json::from_str(text).map_err(|e| format!("schema: {e}"))
    }

    /// The band with this id.
    pub fn band(&self, id: &str) -> Option<&Band> {
        self.bands.iter().find(|b| b.id == id)
    }

    /// The band carrying ladder LOD `lod`.
    pub fn band_of_lod(&self, lod: usize) -> Option<&Band> {
        self.bands.iter().find(|b| b.lods.contains(&lod))
    }

    /// The one band with role `core` (validated to exist).
    pub fn core_band(&self) -> Option<&Band> {
        self.bands.iter().find(|b| b.role == BandRole::Core)
    }

    /// `S_MAX` as `log2` — the largest cell size in the table, which is the assembly bbox's
    /// alignment modulus (OBCA §2.1).
    pub fn s_max_log2(&self) -> u32 {
        self.bands.iter().map(|b| b.cell_log2).max().unwrap_or(MIN_CELL_LOG2)
    }

    /// OBCA §1.2's partition rule and §5.1's role rules. A consumer MUST reject a violation: a LOD
    /// in no band is a map blank at that zoom, a LOD in two bands is a map carrying it twice, and a
    /// `core` band with geometry spends the one file a set cannot split.
    pub fn validate(&self) -> Result<(), String> {
        if self.bands.is_empty() {
            return Err("band table is empty".into());
        }
        if self.lods.is_empty() {
            return Err("the ladder is empty".into());
        }
        for (i, l) in self.lods.iter().enumerate() {
            if l.index != i {
                return Err(format!("ladder entry {i} declares index {}: levels must be listed in order", l.index));
            }
        }
        // Strictly decreasing `Max Meters/Pixel` with `+inf` at the top (`OBCM_Spec.md` §3).
        if self.lods[0].max_mpp.is_some() {
            return Err("ladder level 0 must be the +inf level (max_mpp null)".into());
        }
        for w in self.lods.windows(2) {
            if let (Some(a), Some(b)) = (w[0].max_mpp, w[1].max_mpp) {
                if b >= a {
                    return Err(format!("max_mpp must strictly decrease down the ladder ({a} then {b})"));
                }
            }
        }
        let mut owner: Vec<Option<&str>> = vec![None; self.lods.len()];
        let mut ids = std::collections::HashSet::new();
        for b in &self.bands {
            if b.id.is_empty() {
                return Err("a band has an empty id".into());
            }
            if !ids.insert(b.id.as_str()) {
                return Err(format!("band id {:?} appears twice", b.id));
            }
            if !(MIN_CELL_LOG2..=MAX_CELL_LOG2).contains(&b.cell_log2) {
                return Err(format!("band {:?}: cell size 2^{} is outside the grid", b.id, b.cell_log2));
            }
            for s in &b.sections {
                if s != "nav" && s != "poi" {
                    return Err(format!("band {:?}: unknown section {s:?}", b.id));
                }
            }
            for &l in &b.lods {
                let slot =
                    owner.get_mut(l).ok_or_else(|| format!("band {:?} claims LOD {l}, past the ladder", b.id))?;
                if let Some(other) = slot {
                    return Err(format!("LOD {l} is in two bands ({other} and {})", b.id));
                }
                *slot = Some(&b.id);
            }
        }
        if let Some(l) = owner.iter().position(Option::is_none) {
            return Err(format!("LOD {l} is in no band — its cells would be blank at that zoom"));
        }
        for (name, count) in [
            ("nav", self.bands.iter().filter(|b| b.has_nav()).count()),
            ("poi", self.bands.iter().filter(|b| b.has_poi()).count()),
        ] {
            if count != 1 {
                return Err(format!("the {name} section must be in exactly one band, found {count}"));
            }
        }
        let cores: Vec<&Band> = self.bands.iter().filter(|b| b.role == BandRole::Core).collect();
        if cores.len() != 1 {
            return Err(format!("exactly one band must have role \"core\", found {}", cores.len()));
        }
        let core = cores[0];
        if !core.lods.is_empty() {
            return Err(format!(
                "the core band {:?} carries LOD(s) {:?}: geometry belongs in a splittable shard (OBCA §5.1)",
                core.id, core.lods
            ));
        }
        if !(core.has_nav() && core.has_poi()) {
            return Err(format!("the core band {:?} must carry both the nav and POI sections", core.id));
        }
        if self.bands.iter().filter(|b| b.role == BandRole::Coarse).count() > 1 {
            return Err("at most one band may have role \"coarse\"".into());
        }
        for b in &self.bands {
            if b.role != BandRole::Core && !b.sections.is_empty() {
                return Err(format!(
                    "band {:?} (role {}) carries sections: only the core band may",
                    b.id,
                    b.role.as_str()
                ));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for s in &self.styles {
            if !seen.insert(s.id) {
                return Err(format!("style id {} is assigned to two feature types", s.id));
            }
            if s.id == 0 || s.id == 0xFF {
                return Err(format!("style id {} is reserved (0 unused, 0xFF is the chunk sentinel)", s.id));
            }
        }
        Ok(())
    }
}

/// One feature type's presentation, as a skin states it (OBCC §5).
///
/// Two spellings are accepted, because two callers exist. A hosted skin names the **feature type**
/// and the schema's canonical assignment turns it into an id — that is the OBCC shape and the one
/// that keeps the schema/skin split honest. A hand-written local skin may instead name the **id**
/// directly, which is what a caller has when it is restyling a cell tree whose schema document does
/// not travel with it; the engine then cross-checks the resulting id set against the cells' own
/// style table, so a wrong id is a refusal rather than an invisible layer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkinStyle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature_type: Option<String>,
    #[serde(deserialize_with = "de_color")]
    pub color: u16,
    pub weight: u8,
    pub z_index: i8,
    #[serde(default = "default_priority")]
    pub priority: u8,
    #[serde(default)]
    pub dashed: bool,
    /// OBCM §2 flag bit 4 (#1095): `weight` is device pixels, off the renderer's zoom width ramp.
    #[serde(default)]
    pub fixed_width: bool,
    /// OBCM §2 flag bit 5 (#1095): part of the suppressible terrain layer.
    #[serde(default)]
    pub terrain_layer: bool,
    #[serde(default, deserialize_with = "de_color_opt")]
    pub color2: Option<u16>,
}

fn default_priority() -> u8 {
    1
}

/// A skin: the presentation half of a preset, stamped onto ~2 KB of an assembly (OBCA §4.7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skin {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(deserialize_with = "de_color")]
    pub marker_color: u16,
    pub styles: Vec<SkinStyle>,
}

impl Skin {
    pub fn parse(text: &str) -> Result<Skin, String> {
        serde_json::from_str(text).map_err(|e| format!("skin: {e}"))
    }

    /// Resolve this skin against the schema's canonical id assignment: one 8-byte style record per
    /// schema style, in schema order.
    ///
    /// An assembler MUST reject a skin that does not cover every id in the schema's table, and MUST
    /// reject one naming a feature type the schema does not have — silently defaulting a missing
    /// style would ship a map with an invisible layer (OBCA §4.7).
    pub fn resolve(&self, schema: &Schema) -> Result<Vec<StyleRecord>, String> {
        let record = |id: u8, v: &SkinStyle| StyleRecord {
            id,
            z_index: v.z_index,
            color: v.color,
            weight: v.weight,
            priority: v.priority.clamp(1, 4),
            dashed: v.dashed,
            color2: v.color2,
            fixed_width: v.fixed_width,
            terrain_layer: v.terrain_layer,
        };
        let mut out = Vec::with_capacity(self.styles.len());
        if schema.styles.is_empty() {
            // No canonical assignment travelled with the schema: the skin must state the ids, and
            // the engine checks them against the cells' own style table.
            for v in &self.styles {
                let id = v.id.ok_or_else(|| {
                    format!(
                        "the schema declares no style-id assignment, so every skin style must name its `id` \
                         (offending entry: {:?})",
                        v.feature_type.as_deref().unwrap_or("<unnamed>")
                    )
                })?;
                out.push(record(id, v));
            }
        } else {
            for s in &schema.styles {
                let v = self
                    .styles
                    .iter()
                    .find(|k| k.feature_type.as_deref() == Some(s.feature_type.as_str()) || k.id == Some(s.id))
                    .ok_or_else(|| format!("skin covers no style for feature type {:?} (OBCA §4.7)", s.feature_type))?;
                out.push(record(s.id, v));
            }
            for v in &self.styles {
                let named = v
                    .feature_type
                    .as_deref()
                    .map(|ft| schema.styles.iter().any(|s| s.feature_type == ft))
                    .unwrap_or(false);
                let by_id = v.id.map(|id| schema.styles.iter().any(|s| s.id == id)).unwrap_or(false);
                if !named && !by_id {
                    return Err(format!(
                        "skin names feature type {:?}, which the schema does not have",
                        v.feature_type.as_deref().unwrap_or("<unnamed>")
                    ));
                }
            }
        }
        // §4.7: "write the style table with the schema's ids **in the schema's order**", and "the
        // skin MUST NOT introduce, remove, reorder, or renumber ids". Silently sorting would make
        // this crate the thing that decides the order — and the id set is compared against the
        // cells' own style table right after, so a skin listed out of order would resolve to a table
        // that matches by luck. The order is the caller's to get right, and a violation is a
        // refusal.
        for w in out.windows(2) {
            if w[0].id == w[1].id {
                return Err(format!("the resolved style table assigns id {} twice (OBCA §4.7)", w[0].id));
            }
            if w[0].id > w[1].id {
                return Err(format!(
                    "the resolved style table runs {} then {}: ids must ascend, in the schema's own order — a skin \
                     may not reorder them (OBCA §4.7)",
                    w[0].id, w[1].id
                ));
            }
        }
        // The rain band gap (WX10, `obc_map_scene::RAIN_BAND_GAP_LOW/HIGH`): no style may sit in
        // the open interval the renderer's rain boundary lives in. Ground fills stay at or below
        // the gap, the road band at or above it; a skin that parks a style inside would make
        // "roads above precipitation" ambiguous for every map it is stamped onto.
        for record in &out {
            if record.z_index > obc_map_scene::RAIN_BAND_GAP_LOW && record.z_index < obc_map_scene::RAIN_BAND_GAP_HIGH {
                return Err(format!(
                    "style id {} places z_index {} inside the reserved rain band gap ({}, {}) — ground styles stay \
                     at or below {}, the road band at or above {} (WX10, epic #1185)",
                    record.id,
                    record.z_index,
                    obc_map_scene::RAIN_BAND_GAP_LOW,
                    obc_map_scene::RAIN_BAND_GAP_HIGH,
                    obc_map_scene::RAIN_BAND_GAP_LOW,
                    obc_map_scene::RAIN_BAND_GAP_HIGH,
                ));
            }
        }
        Ok(out)
    }
}

/// One resolved 8-byte style-table record (`OBCM_Spec.md` §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleRecord {
    pub id: u8,
    pub z_index: i8,
    pub color: u16,
    pub weight: u8,
    pub priority: u8,
    pub dashed: bool,
    pub color2: Option<u16>,
    /// Flag bit 4 (#1095): the weight is used verbatim on screen, off the zoom width ramp.
    pub fixed_width: bool,
    /// Flag bit 5 (#1095): part of the suppressible terrain layer.
    pub terrain_layer: bool,
}

/// RGB565 as either a JSON number or a `"0x…"` / decimal string — the two spellings that exist in
/// the wild (OBCC writes numbers; `obc-pack` config files write `"0xF800"`).
fn de_color<'de, D: Deserializer<'de>>(d: D) -> Result<u16, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Num(u32),
        Text(String),
    }
    match Raw::deserialize(d)? {
        Raw::Num(v) => u16::try_from(v).map_err(|_| de::Error::custom(format!("color {v} is not RGB565"))),
        Raw::Text(s) => {
            let t = s.trim();
            let parsed = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                Some(hex) => u16::from_str_radix(hex, 16),
                None => t.parse::<u16>(),
            };
            parsed.map_err(|_| de::Error::custom(format!("color {s:?} is not an RGB565 value")))
        }
    }
}

fn de_color_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u16>, D::Error> {
    #[derive(Deserialize)]
    struct Wrap(#[serde(deserialize_with = "de_color")] u16);
    Ok(Option::<Wrap>::deserialize(d)?.map(|w| w.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The v1 band table of OBCA §1.5, over the shipped 9-LOD ladder.
    pub(crate) fn v1_schema() -> Schema {
        let band = |id: &str, cell_log2: u32, lods: &[usize], sections: &[&str], role: BandRole| Band {
            id: id.into(),
            cell_log2,
            lods: lods.to_vec(),
            sections: sections.iter().map(|s| (*s).to_string()).collect(),
            role,
        };
        let mpp =
            [None, Some(900.0), Some(400.0), Some(120.0), Some(90.0), Some(40.0), Some(12.0), Some(4.0), Some(1.0)];
        Schema {
            id: "bikepacking".into(),
            revision: 1,
            obcm_version: obc_formats::obcm::VERSION,
            lods: mpp
                .iter()
                .enumerate()
                .map(|(index, &max_mpp)| LodEntry { index, max_mpp, band: String::new() })
                .collect(),
            bands: vec![
                band("coarse", 20, &[0, 1, 2, 3, 4], &[], BandRole::Coarse),
                band("mid", 19, &[5, 6], &[], BandRole::Geometry),
                band("fine", 18, &[7, 8], &[], BandRole::Geometry),
                band("network", 18, &[], &["nav", "poi"], BandRole::Core),
            ],
            styles: vec![StyleId { id: 1, feature_type: "natural.water".into() }],
            routing: Routing { min_component_edges: 50, profiles: vec!["Road".into()] },
            chunk_size: 4096,
        }
    }

    #[test]
    fn v1_table_validates_and_reports_s_max() {
        let s = v1_schema();
        s.validate().expect("the v1 table partitions the 9-LOD ladder");
        assert_eq!(s.s_max_log2(), 20);
        assert_eq!(s.core_band().unwrap().id, "network");
        assert_eq!(s.band_of_lod(4).unwrap().id, "coarse");
        assert_eq!(s.band_of_lod(6).unwrap().id, "mid");
    }

    #[test]
    fn validation_catches_the_partition_and_role_traps() {
        let err = |s: &Schema| s.validate().expect_err("must be rejected");
        let mut s = v1_schema();
        s.bands[1].lods.push(4);
        assert!(err(&s).contains("in two bands"));

        let mut s = v1_schema();
        s.bands[2].lods = vec![7];
        assert!(err(&s).contains("LOD 8 is in no band"));

        let mut s = v1_schema();
        s.bands[3].lods = vec![8];
        s.bands[2].lods = vec![7];
        assert!(err(&s).contains("core band"));

        let mut s = v1_schema();
        s.bands[2].sections = vec!["nav".into()];
        assert!(err(&s).contains("nav section must be in exactly one band"));

        let mut s = v1_schema();
        s.lods[3].max_mpp = Some(1000.0);
        assert!(err(&s).contains("strictly decrease"));
    }

    #[test]
    fn a_skin_must_cover_the_schema_exactly() {
        let mut schema = v1_schema();
        schema.styles = vec![
            StyleId { id: 1, feature_type: "natural.water".into() },
            StyleId { id: 2, feature_type: "highway.residential".into() },
        ];
        let style = |ft: &str, color: u16| SkinStyle {
            id: None,
            feature_type: Some(ft.into()),
            color,
            weight: 2,
            z_index: 3,
            priority: 1,
            dashed: false,
            fixed_width: false,
            terrain_layer: false,
            color2: None,
        };
        let full = Skin {
            id: "default".into(),
            name: "Default".into(),
            marker_color: 0xF800,
            styles: vec![style("natural.water", 0x001F), style("highway.residential", 0xFFFF)],
        };
        let recs = full.resolve(&schema).expect("a covering skin resolves");
        assert_eq!(recs.iter().map(|r| (r.id, r.color)).collect::<Vec<_>>(), vec![(1, 0x001F), (2, 0xFFFF)]);

        let mut missing = full.clone();
        missing.styles.pop();
        assert!(missing.resolve(&schema).unwrap_err().contains("covers no style"));

        let mut stale = full.clone();
        stale.styles.push(style("landuse.gone", 0));
        assert!(stale.resolve(&schema).unwrap_err().contains("does not have"));

        // The id-keyed spelling: legal when no canonical assignment travelled with the schema.
        let mut bare = schema.clone();
        bare.styles.clear();
        let by_id = Skin {
            styles: vec![
                SkinStyle { id: Some(1), ..style("", 0xFF00) },
                SkinStyle { id: Some(2), ..style("", 0x00FF) },
            ],
            ..full.clone()
        };
        let recs = by_id.resolve(&bare).expect("id-keyed skins resolve");
        assert_eq!(recs.iter().map(|r| (r.id, r.color)).collect::<Vec<_>>(), vec![(1, 0xFF00), (2, 0x00FF)]);
        // …but the order is the caller's to state, not this crate's to invent: §4.7 forbids a skin
        // from reordering ids, so a mis-ordered table is a refusal rather than a silent re-sort.
        let shuffled = Skin { styles: by_id.styles.iter().rev().cloned().collect(), ..by_id.clone() };
        assert!(shuffled.resolve(&bare).unwrap_err().contains("ids must ascend"));
        let idless = Skin { styles: vec![style("natural.water", 1)], ..full };
        assert!(idless.resolve(&bare).unwrap_err().contains("must name its `id`"));
    }

    #[test]
    fn colors_parse_as_numbers_or_hex_strings() {
        let json = r#"{
            "marker_color": "0xF800",
            "styles": [{"feature_type": "a", "color": 31, "weight": 1, "z_index": 0, "color2": "0x07E0"}]
        }"#;
        let skin = Skin::parse(json).expect("parses");
        assert_eq!(skin.marker_color, 0xF800);
        assert_eq!(skin.styles[0].color, 31);
        assert_eq!(skin.styles[0].color2, Some(0x07E0));
        assert_eq!(skin.styles[0].priority, 1, "priority defaults to the highest tier");
    }
}
