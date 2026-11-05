// The words of the one road model's records for the stamp pass, in place of model-words.wgsl, and the
// look's cache of each mask page that the pass writes (model_cache_texel of model.wgsl).
@group(0) @binding(8) var model_words: texture_2d<u32>;
@group(0) @binding(9) var model_cache_out: texture_storage_2d_array<rgba16uint, write>;
@group(0) @binding(11) var model_paged_out: texture_storage_2d_array<rgba8unorm, write>;
