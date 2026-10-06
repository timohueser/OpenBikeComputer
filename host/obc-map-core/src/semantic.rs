//! Semantic style classification shared by config and draw producers.

const CLASSES: usize = 7;

/// Land-cover paint order. Water uses a separate mask after the categorical allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SemanticClass {
    Base = 0,
    Farmland = 1,
    Grass = 2,
    Forest = 3,
    Urban = 4,
    Rock = 5,
    Ice = 6,
    Water = 7,
}

/// Style classification and canonical output styles derived from the typed map config.
#[derive(Clone)]
pub struct SemanticScheme {
    by_style: [Option<SemanticClass>; 255],
    output_style: [Option<u8>; CLASSES + 1],
}

impl SemanticScheme {
    pub fn new() -> Self {
        Self { by_style: [None; 255], output_style: [None; CLASSES + 1] }
    }

    pub fn insert(&mut self, style_id: u8, class: SemanticClass) {
        self.by_style[style_id as usize] = Some(class);
        let slot = &mut self.output_style[class as usize];
        *slot = Some(slot.map_or(style_id, |current| current.min(style_id)));
    }

    #[inline]
    pub fn class_of(&self, style_id: u8) -> Option<SemanticClass> {
        self.by_style.get(style_id as usize).copied().flatten()
    }

    #[inline]
    pub fn style_for(&self, class: SemanticClass) -> Option<u8> {
        self.output_style[class as usize]
    }

    pub fn output_styles(&self) -> &[Option<u8>] {
        &self.output_style
    }
}
impl Default for SemanticScheme {
    fn default() -> Self {
        Self::new()
    }
}
