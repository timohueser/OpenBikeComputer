use crate::{Error, Result};

#[derive(Clone)]
pub struct Ids {
    ranges: Vec<[u32; 2]>,
    offsets: Vec<u32>,
    pub count: u32,
    source_count: u32,
}
impl Ids {
    pub fn new(ranges: Vec<[u32; 2]>, source_count: u32) -> Result<Self> {
        let mut end = 0;
        let mut count = 0u32;
        let mut offsets = Vec::with_capacity(ranges.len());
        for &[first, last] in &ranges {
            if first < end || first >= last || last > source_count {
                return Err(Error::InvalidData("Overlapping or invalid road ranges".into()));
            }
            offsets.push(count);
            count = count.checked_add(last - first).ok_or(Error::Limit)?;
            end = last;
        }
        if count == 0 || count == u32::MAX {
            return Err(Error::InvalidData("Empty routing selection".into()));
        }
        Ok(Self { ranges, offsets, count, source_count })
    }
    pub fn iter(&self) -> impl Iterator<Item = u32> + Clone + '_ {
        self.ranges.iter().flat_map(|&[a, z]| a..z)
    }
    pub fn local(&self, id: u32) -> Option<u32> {
        let at = self.ranges.partition_point(|r| r[0] <= id).checked_sub(1)?;
        (id < self.ranges[at][1]).then(|| self.offsets[at] + id - self.ranges[at][0])
    }
    pub fn source(&self, local: u32) -> Result<u32> {
        if local >= self.count {
            return Err(Error::InvalidData("Road outside selection".into()));
        }
        let at = self.offsets.partition_point(|&i| i <= local) - 1;
        Ok(self.ranges[at][0] + local - self.offsets[at])
    }
    pub fn lookup_bytes(&self) -> usize {
        let mut previous = usize::MAX;
        let mut count = 0usize;
        for &[a, z] in &self.ranges {
            for page in a as usize / 4096..=(z - 1) as usize / 4096 {
                if page != previous {
                    count += 1;
                    previous = page;
                }
            }
        }
        count.saturating_mul(4096 * 4).saturating_add(
            (self.source_count as usize).div_ceil(4096) * std::mem::size_of::<Option<Box<[u32; 4096]>>>(),
        )
    }
    pub fn fast(&self) -> Result<Lookup> {
        let mut pages = Vec::new();
        pages.try_reserve_exact((self.source_count as usize).div_ceil(4096)).map_err(|_| Error::Limit)?;
        pages.resize_with((self.source_count as usize).div_ceil(4096), || None);
        for (local, source) in self.iter().enumerate() {
            let page = pages[source as usize / 4096].get_or_insert_with(|| Box::new([u32::MAX; 4096]));
            page[source as usize % 4096] = local as u32;
        }
        Ok(Lookup(pages))
    }
}
pub struct Lookup(Vec<Option<Box<[u32; 4096]>>>);
impl Lookup {
    pub fn get(&self, source: u32) -> Option<u32> {
        let &local = self.0.get(source as usize / 4096)?.as_ref()?.get(source as usize % 4096)?;
        (local != u32::MAX).then_some(local)
    }
}
