//! Intern exact cost parameters without floating-point text conversion.
use crate::cost::CostBasis;
use std::collections::HashMap;

type Result<T> = std::result::Result<T, String>;

#[derive(Default)]
pub struct Dictionary {
    ids: HashMap<(u64, u64, bool), u32>,
    values: Vec<CostBasis>,
}

impl Dictionary {
    pub fn insert(&mut self, value: CostBasis) -> Result<u32> {
        if !value.valid() {
            return Err("Invalid cost basis".into());
        }
        let key = (value.factor.to_bits(), value.turn.to_bits(), value.ferry);
        if let Some(&id) = self.ids.get(&key) {
            return Ok(id);
        }
        let id = u32::try_from(self.values.len() + 1).map_err(|_| "Too many cost factors")?;
        self.ids.insert(key, id);
        self.values.push(value);
        Ok(id)
    }

    pub fn into_values(self) -> Vec<CostBasis> {
        self.values
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dictionary_preserves_float_bits_and_rejects_invalid_costs() {
        let a = CostBasis { factor: f64::from_bits(0x3ff8a3d70a3d70a5), turn: 0.0, ferry: false };
        let b = CostBasis { turn: -0.0, ..a };
        let mut dictionary = Dictionary::default();
        assert_eq!(dictionary.insert(a).unwrap(), 1);
        assert_eq!(dictionary.insert(a).unwrap(), 1);
        assert_eq!(dictionary.insert(b).unwrap(), 2);
        assert!(dictionary.insert(CostBasis { factor: f64::NAN, ..a }).is_err());
        let values = dictionary.into_values();
        assert_eq!(values[0].factor.to_bits(), a.factor.to_bits());
        assert_eq!(values[1].turn.to_bits(), b.turn.to_bits());
    }
}
