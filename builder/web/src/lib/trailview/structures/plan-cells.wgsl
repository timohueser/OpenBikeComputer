// The cells whose ground-plan meshes draw (plan/cells.wgsl), at the binding of the structures' group.
@group(0) @binding(6) var<storage, read> plan_cells: array<u32>;
