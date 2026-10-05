use core::cmp::Ordering;

/// One comparator type shares the core sorter across the fill paths.
#[inline]
pub(crate) fn crossings(a: &f32, b: &f32) -> Ordering {
    a.partial_cmp(b).unwrap()
}

#[cfg(test)]
mod tests {
    use super::crossings;

    #[test]
    fn crossing_order_preserves_float_bits() {
        let values = [-0.0f32, 0.0, -8.25, 32.5, f32::NEG_INFINITY, f32::INFINITY];
        for len in 0..=crate::MAX_CROSSINGS {
            let mut original: std::vec::Vec<f32> = (0..len).map(|i| values[(i * 7 + i / 6) % values.len()]).collect();
            let mut shared = original.clone();
            original.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
            shared.sort_unstable_by(crossings);
            assert!(shared.iter().zip(&original).all(|(a, b)| a.to_bits() == b.to_bits()), "length {len}");
        }
    }
}
