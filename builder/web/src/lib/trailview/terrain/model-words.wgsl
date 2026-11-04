// The words of the one road model's records for the shaders that draw, and the look's cache of each mask
// page: group 3 of the terrain.
@group(3) @binding(3) var model_words: texture_2d<u32>;
@group(3) @binding(4) var model_cache: texture_2d_array<u32>;
