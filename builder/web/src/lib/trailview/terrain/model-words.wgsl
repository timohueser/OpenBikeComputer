// The words of the records of the cells (the crate's `faces::records` blocks), MODEL_ROW words per row.
// A pass that reads them binds this texture, or swaps this file for its own binding.
@group(0) @binding(0) var model_words: texture_2d<u32>;
