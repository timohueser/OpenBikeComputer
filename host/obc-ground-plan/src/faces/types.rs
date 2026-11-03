//! The face plan of one cell and level: the bytes that GP4 to GP7 read.

/// A way of the graph (`WayGraph::ways`), a junction (`WayGraph::junctions`), a feature of the cell's
/// input (`CellInput::features`). Cell-local.
pub type WayId = u32;
pub type JunctionId = u32;
pub type FeatureId = u32;

/// What a face belongs to. `Bend(way, dm)` is a tip fill: its way and the stroke decimetres of its
/// tightest sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Owner {
    Terrain,
    Way(WayId),
    Junction(JunctionId),
    Bend(WayId, i64),
    Platform(FeatureId),
}

/// The codes are bytes of the plan: later items append and never reuse one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Surface {
    Terrain = 0,
    Slope = 1,
    Carriageway = 2,
    Shoulder = 3,
    Verge = 4,
    CornerFill = 5,
    Crossing = 6,
    RailBed = 7,
    Platform = 8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum HeightRule {
    Terrain = 0,
    Slope = 1,
    Owner = 2,
    Blend = 3,
    KerbOffset = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum EdgeKind {
    Continuous = 0,
    Kerb = 1,
    Wall = 2,
    Headwall = 3,
    Portal = 4,
    DeckEdge = 5,
    CellBorder = 6,
}

/// Edge flags. `SEAM`: a guest's blend part meets its host's seam face (the give-way line).
/// `BLEND_JOIN`: two blend faces, one blend region. `UNRELATED`: two owners that no way, junction,
/// bend or joint relates.
pub const SEAM: u8 = 1;
pub const BLEND_JOIN: u8 = 2;
pub const UNRELATED: u8 = 4;
/// The face index past the window's edge.
pub const OUTSIDE: u32 = u32::MAX;

#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub id: u32,
    pub owner: Owner,
    pub surface: Surface,
    pub level: i8,
    pub rule: HeightRule,
    /// The way whose class and tags give the look.
    pub material: Option<WayId>,
    /// The rail way under a crossing face.
    pub crossing: Option<WayId>,
    /// The index of the face's rank in `plan.json`'s `ranks`: 0 wins.
    pub rank: u8,
    /// The face lies in the cell square.
    pub inside: bool,
    /// The slope reach of the face's class; 0 for slope and terrain.
    pub reach_m: f32,
    /// Vertex indices; ring 0 has a positive shoelace sum (x east, y south), each hole a negative one.
    pub rings: Vec<Vec<u32>>,
}

/// One edge of the planar map: `left` is the face on the side where the shoelace sum of (a, b, point)
/// is positive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub a: u32,
    pub b: u32,
    pub left: u32,
    pub right: u32,
    pub kind: EdgeKind,
    pub flags: u8,
}

/// The circle of a corner fill: centre and feet in cell units, the arc from `t_a` to `t_b`.
#[derive(Clone, Debug, PartialEq)]
pub struct CornerArc {
    pub owner: Owner,
    pub centre: [i32; 2],
    pub radius_m: f64,
    pub t_a: [i32; 2],
    pub t_b: [i32; 2],
    pub arc: Vec<[i32; 2]>,
}

/// What the stages built, dropped and merged.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FaceStats {
    pub candidates: u32,
    pub pieces: u32,
    pub patches: u32,
    pub slice_segments: u32,
    pub slices: u32,
    pub faces: u32,
    pub vertices: u32,
    pub edges: u32,
    /// Corners by pair rule (`road/track`, `bend`, `kink`): built, built smaller than asked, dropped.
    pub corners: std::collections::BTreeMap<String, [u32; 3]>,
    pub seam_missing: u32,
    pub ramp_capped: u32,
    pub platforms_skipped: u32,
    /// Thin pieces merged, by (from surface, to surface).
    pub thin: std::collections::BTreeMap<(Surface, Surface), u32>,
    pub pinches_left: u32,
    /// The thin pieces of every pass, for the debug draw.
    pub thin_pieces: Vec<Vec<[i32; 2]>>,
    /// Candidate points past their extent or ramp cap: check 3, target 0.
    pub extent_faults: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FacePlan {
    pub cell: crate::frame::CellKey,
    pub level: i8,
    pub vertices: Vec<[i32; 2]>,
    pub faces: Vec<Face>,
    pub edges: Vec<Edge>,
    pub corners: Vec<CornerArc>,
    pub stats: FaceStats,
}
