//! The face plan of one cell and level: the bytes that GP4 to GP7 read.

/// The index of a way of the cell's graph (`WayGraph::ways`) and of a junction
/// (`WayGraph::junctions`): local to the cell.
pub type WayId = u32;
pub type JunctionId = u32;

/// What a face belongs to. Band layers (shoulder, verge, slope) and terrain belong to `Terrain`: one
/// face per layer, whatever ways made it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Owner {
    Terrain,
    Way(WayId),
    Junction(JunctionId),
    /// A platform: the index of its feature in the cell's input.
    Platform(u32),
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
    /// Line water: its face lies at the water level.
    Water = 9,
    /// The band from a waterline to the bank top.
    Bank = 10,
    /// Ground over line water at a crossing (`water_crossings`): the water does not show there, and the slopes
    /// shape it.
    Zone = 11,
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
    /// The legs of `material` and `crossing`: a way that passes beside itself splits into legs
    /// (`faces::legs`); 0 for a way with one.
    pub legs: [u8; 2],
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

/// What the stages built. Counts over the cell's inputs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FaceStats {
    pub ways: u32,
    pub junctions: u32,
    pub joints: u32,
    pub turns: u32,
    pub candidates: u32,
    pub patches: u32,
    pub slice_segments: u32,
    pub faces: u32,
    pub vertices: u32,
    pub edges: u32,
    /// Platforms near the window without vertex heights: they get no face.
    pub platforms_skipped: u32,
}

impl FaceStats {
    /// Every counter, by name.
    pub fn counters(&self) -> [(&'static str, u32); 11] {
        [
            ("ways", self.ways),
            ("junctions", self.junctions),
            ("joints", self.joints),
            ("turns", self.turns),
            ("candidates", self.candidates),
            ("patches", self.patches),
            ("slice_segments", self.slice_segments),
            ("faces", self.faces),
            ("vertices", self.vertices),
            ("edges", self.edges),
            ("platforms_skipped", self.platforms_skipped),
        ]
    }
}

/// One round, in cell units: its centre, its radius, its feet `t_a` and `t_b` (the tangent points on
/// the two edges it joins: the ends of its arc), and its arc's points at the global directions
/// strictly between the feet, from `t_a` to `t_b`. A sharp corner (radius 0) has the corner as its
/// centre, its feet and its arc.
#[derive(Clone, Debug, PartialEq)]
pub struct CornerArc {
    pub owner: Owner,
    pub centre: [i32; 2],
    pub radius_m: f64,
    pub t_a: [i32; 2],
    pub t_b: [i32; 2],
    pub arc: Vec<[i32; 2]>,
}

impl CornerArc {
    /// `mpu`: metres per unit at the centre.
    pub(crate) fn new(owner: Owner, c: &super::points::Circle, mpu: f64) -> Self {
        let [t_a, t_b] = c.feet;
        CornerArc { owner, centre: c.centre, radius_m: c.radius * mpu, t_a, t_b, arc: c.arc.clone() }
    }
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
