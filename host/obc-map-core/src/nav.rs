//! Routing profile vocabulary shared by config and producers.

/// Default island-pruning threshold: keep every connected component with at least this many edges,
/// plus the single largest. The packer threads `routing.min_component_edges` through
/// `build_graph_with` instead; this default is for tests.
pub const DEFAULT_MIN_COMPONENT_EDGES: usize = 50;

/// Canonical highway-class names, indexed by the 5-bit class id. Also the profile config's class
/// keys: a `routing.profiles[*].highway` map is keyed by these exact names, resolved via
/// [`highway_class_index`]. One source of truth for the packed byte and the config vocabulary.
pub const HIGHWAY_CLASS_NAMES: [&str; 14] = [
    "cycleway",      // 0
    "path",          // 1
    "track",         // 2
    "footway",       // 3
    "steps",         // 4
    "bridleway",     // 5
    "living_street", // 6
    "residential",   // 7
    "service",       // 8
    "unclassified",  // 9
    "tertiary",      // 10
    "secondary",     // 11
    "primary",       // 12
    "trunk_cycl",    // 13
];

/// Canonical surface-class names, indexed by the 3-bit class id. The other half of the profile
/// config's class vocabulary, resolved via [`surface_class_index`].
pub const SURFACE_CLASS_NAMES: [&str; 8] =
    ["unknown", "paved", "compacted", "gravel", "dirt", "rough", "cobbles", "grass"];

/// Resolve a highway-class name to its 5-bit class id, or `None` for an unknown name. The config's
/// profile parser uses this to key its per-class multipliers.
pub fn highway_class_index(name: &str) -> Option<u8> {
    HIGHWAY_CLASS_NAMES.iter().position(|&n| n == name).map(|i| i as u8)
}

/// Resolve a surface-class name to its 3-bit class id, or `None`.
pub fn surface_class_index(name: &str) -> Option<u8> {
    SURFACE_CLASS_NAMES.iter().position(|&n| n == name).map(|i| i as u8)
}
