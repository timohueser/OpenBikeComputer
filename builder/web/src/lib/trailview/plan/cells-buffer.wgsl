// The cells whose ground-plan meshes draw (cells.wgsl), at the binding of the terrain's and the pieces' groups.
@group(0) @binding(3) var<storage, read> plan_cells: array<u32>;
