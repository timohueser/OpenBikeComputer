//! The four fixed bike types (`OBCR_Spec.md` §1.2, `OBCM_Spec.md` §8.6).
//!
//! The wire value is also the map's routing-profile index, so `BikeType as u8` is the profile
//! the router uses.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[repr(u8)]
pub enum BikeType {
    #[default]
    Road = 0,
    Gravel = 1,
    Mtb = 2,
    Touring = 3,
}

impl BikeType {
    pub const ALL: [BikeType; 4] = [BikeType::Road, BikeType::Gravel, BikeType::Mtb, BikeType::Touring];

    /// The wire value `0..=3`; anything else is not a bike type.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(BikeType::Road),
            1 => Some(BikeType::Gravel),
            2 => Some(BikeType::Mtb),
            3 => Some(BikeType::Touring),
            _ => None,
        }
    }

    /// The name a map writes into its profile record for this type.
    pub const fn profile_name(self) -> &'static str {
        match self {
            BikeType::Road => "Road",
            BikeType::Gravel => "Gravel",
            BikeType::Mtb => "MTB",
            BikeType::Touring => "Touring",
        }
    }

    /// Flat-ground speed (km/h) and climb cost (tenths of a second per metre climbed): the
    /// estimate table of `OBCR_Spec.md` §1.2.
    const fn eta_row(self) -> (u64, u64) {
        match self {
            BikeType::Road => (22, 16),
            BikeType::Gravel => (19, 19),
            BikeType::Mtb => (16, 23),
            BikeType::Touring => (17, 22),
        }
    }

    /// Whole seconds to ride `distance_m` metres while climbing `ascent_m` metres:
    /// `floor((36·d + a·k·v) / (10·v))`. Integer arithmetic, so every implementation of the
    /// contract gives the same second.
    pub const fn ride_time_s(self, distance_m: u32, ascent_m: u32) -> u32 {
        let (v, k) = self.eta_row();
        let t = (36 * distance_m as u64 + ascent_m as u64 * k * v) / (10 * v);
        if t > u32::MAX as u64 {
            u32::MAX
        } else {
            t as u32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_values_round_trip_and_reject_the_rest() {
        for bike in BikeType::ALL {
            assert_eq!(BikeType::from_u8(bike as u8), Some(bike));
        }
        assert_eq!(BikeType::from_u8(4), None);
    }

    #[test]
    fn estimate_matches_the_shared_vector_file() {
        let mut cases = 0;
        for line in
            include_str!("../../../specs/vectors/eta.csv").lines().skip_while(|l| !l.starts_with("bike,")).skip(1)
        {
            let v: std::vec::Vec<u64> = line.split(',').map(|f| f.parse().unwrap()).collect();
            let bike = BikeType::from_u8(v[0] as u8).unwrap();
            assert_eq!(u64::from(bike.ride_time_s(v[1] as u32, v[2] as u32)), v[3], "{line}");
            cases += 1;
        }
        assert!(cases >= 4 * 10, "the vector file covers every type");
    }
}
