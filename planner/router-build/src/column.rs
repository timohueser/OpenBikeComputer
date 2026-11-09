//! Flat containers of fixed-width values, the storage of the large tables of the bake.

#[derive(Clone, Debug)]
pub struct Column<T>(Vec<T>);

impl<T> Default for Column<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T: Copy> Column<T> {
    pub fn filled(len: usize, value: T) -> Self {
        Self(vec![value; len])
    }

    pub fn push(&mut self, value: T) {
        self.0.push(value);
    }

    pub fn extend(&mut self, values: &[T]) {
        self.0.extend_from_slice(values);
    }

    pub fn values(&self) -> &[T] {
        &self.0
    }

    pub fn values_mut(&mut self) -> &mut [T] {
        &mut self.0
    }

    /// Value `i` of the result is value `order[i]` of `self`.
    pub fn permuted(&self, order: &[u32]) -> Self {
        Self(order.iter().map(|&i| self.0[i as usize]).collect())
    }
}

impl<T> FromIterator<T> for Column<T> {
    fn from_iter<I: IntoIterator<Item = T>>(values: I) -> Self {
        Self(values.into_iter().collect())
    }
}

/// Many variable-length lists in one column.
#[derive(Clone, Debug)]
pub struct Lists<T> {
    ends: Column<u64>,
    values: Column<T>,
}

impl<T> Default for Lists<T> {
    fn default() -> Self {
        Self { ends: Column::default(), values: Column::default() }
    }
}

impl<T: Copy> Lists<T> {
    pub fn push(&mut self, list: &[T]) {
        self.values.extend(list);
        self.ends.push(self.values.values().len() as u64);
    }

    /// Lists of the given lengths, each value `value`.
    pub fn filled(lengths: impl IntoIterator<Item = usize>, value: T) -> Self {
        let mut ends = Column::default();
        let mut end = 0;
        for length in lengths {
            end += length as u64;
            ends.push(end);
        }
        Self { ends, values: Column::filled(end as usize, value) }
    }

    pub fn get(&self, index: usize) -> &[T] {
        let range = self.range(index);
        &self.values.values()[range]
    }

    pub fn get_mut(&mut self, index: usize) -> &mut [T] {
        let range = self.range(index);
        &mut self.values.values_mut()[range]
    }

    fn range(&self, index: usize) -> std::ops::Range<usize> {
        let ends = self.ends.values();
        let start = if index == 0 { 0 } else { ends[index - 1] as usize };
        start..ends[index] as usize
    }

    /// List `i` of the result is list `order[i]` of `self`.
    pub fn permuted(&self, order: &[u32]) -> Self {
        let total = order.iter().map(|&i| self.range(i as usize).len()).sum();
        let mut lists =
            Self { ends: Column(Vec::with_capacity(order.len())), values: Column(Vec::with_capacity(total)) };
        for &i in order {
            lists.push(self.get(i as usize));
        }
        lists
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_read_back_what_they_took() {
        let taken: [&[i64]; 4] = [&[3, 1], &[], &[i64::MIN], &[7, 7, 7]];
        let mut lists = Lists::default();
        for list in taken {
            lists.push(list);
        }
        assert_eq!((0..taken.len()).map(|i| lists.get(i)).collect::<Vec<_>>(), taken);
    }
}
