//! Host-supplied route alternatives and illustrative remaining-ride costs.

#[derive(Debug, PartialEq)]
pub struct Route {
    pub route: usize,
    pub path: &'static [(i32, i32)],
    pub distance_m: u32,
    pub climb_m: u32,
    pub rough_m: u32,
}

#[derive(Debug, PartialEq)]
pub struct Routes {
    pub current: Route,
    pub alternatives: [Route; 3],
}

impl Routes {
    pub fn current(&self, accepted: Option<u8>) -> &Route {
        accepted.map_or(&self.current, |i| &self.alternatives[i as usize])
    }
}
