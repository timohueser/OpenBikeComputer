// The words of the one road model's records for the shaders that draw, and the look's cache of each mask
// page: group 3 of the terrain.
@group(3) @binding(3) var model_words: texture_2d<u32>;
@group(3) @binding(4) var model_cache: texture_2d_array<u32>;
// The paint of the look once per page (probe 2048): per mask texel and per texel of a drawn page past the masks, as
// the feature mask holds a road (features.wgsl) but of the paved label: its distance, its host's metres across and
// its finish code. The stamp pass writes it (paint_texel of stamp.wgsl).
@group(3) @binding(6) var model_paged: texture_2d_array<f32>;
