//! Query access is independent of whether data is one region or a union of published blocks.
use crate::{
    base, landmarks,
    model::{Point, Profile, Road},
    package::{Endpoint, Package, Source},
    search::Seed,
    snap::{Candidates, Policy},
    Result,
};
use std::sync::Arc;

pub trait RoutingData {
    fn identity(&self) -> &str;
    fn region(&self) -> &str;
    fn bounds(&self) -> [f64; 4];
    fn attribution(&self) -> &str;
    fn warnings(&self) -> &[String];
    fn profiles(&self) -> Vec<&str>;
    fn profile(&self, name: &str) -> Result<&Profile>;
    fn snap(&mut self, point: Point, metric: &str, policy: Policy) -> Result<Candidates>;
    fn road(&mut self, road: u32) -> Result<Road>;
    fn endpoint(&mut self, metric: &str, road: u32) -> Result<Endpoint>;
    fn base(&mut self, metric: &str) -> Result<(Arc<base::Graph>, Arc<base::Costs>)>;
    fn has_landmarks(&self, metric: &str) -> bool;
    fn landmarks(&self, metric: &str, starts: &[Seed], ends: &[Seed]) -> Result<Option<landmarks::Prepared>>;
    fn routing_bytes(&self, metric: &str) -> Result<usize>;
    fn memory_budget(&self) -> usize;
    fn set_memory_budget(&mut self, bytes: usize);
}

impl<S: Source> RoutingData for Package<S> {
    fn identity(&self) -> &str {
        self.identity()
    }
    fn region(&self) -> &str {
        &self.manifest().region
    }
    fn bounds(&self) -> [f64; 4] {
        self.manifest().bounds
    }
    fn attribution(&self) -> &str {
        &self.manifest().attribution
    }
    fn warnings(&self) -> &[String] {
        &self.manifest().warnings
    }
    fn profiles(&self) -> Vec<&str> {
        self.manifest().metrics.keys().map(String::as_str).collect()
    }
    fn profile(&self, name: &str) -> Result<&Profile> {
        Ok(&self.metric(name)?.profile)
    }
    fn snap(&mut self, point: Point, metric: &str, policy: Policy) -> Result<Candidates> {
        self.snap(point, metric, policy)
    }
    fn road(&mut self, road: u32) -> Result<Road> {
        self.road(road)
    }
    fn endpoint(&mut self, metric: &str, road: u32) -> Result<Endpoint> {
        self.endpoint(metric, road)
    }
    fn base(&mut self, metric: &str) -> Result<(Arc<base::Graph>, Arc<base::Costs>)> {
        self.base(metric)
    }
    fn has_landmarks(&self, metric: &str) -> bool {
        self.manifest().landmarks.as_ref().is_some_and(|i| i.profiles.contains_key(metric))
    }
    fn landmarks(&self, metric: &str, starts: &[Seed], ends: &[Seed]) -> Result<Option<landmarks::Prepared>> {
        self.landmarks(metric, starts, ends)
    }
    fn routing_bytes(&self, metric: &str) -> Result<usize> {
        self.routing_bytes(metric)
    }
    fn memory_budget(&self) -> usize {
        self.memory_budget
    }
    fn set_memory_budget(&mut self, bytes: usize) {
        self.memory_budget = bytes;
    }
}

impl<T: RoutingData + ?Sized> RoutingData for Box<T> {
    fn identity(&self) -> &str {
        (**self).identity()
    }
    fn region(&self) -> &str {
        (**self).region()
    }
    fn bounds(&self) -> [f64; 4] {
        (**self).bounds()
    }
    fn attribution(&self) -> &str {
        (**self).attribution()
    }
    fn warnings(&self) -> &[String] {
        (**self).warnings()
    }
    fn profiles(&self) -> Vec<&str> {
        (**self).profiles()
    }
    fn profile(&self, name: &str) -> Result<&Profile> {
        (**self).profile(name)
    }
    fn snap(&mut self, point: Point, metric: &str, policy: Policy) -> Result<Candidates> {
        (**self).snap(point, metric, policy)
    }
    fn road(&mut self, road: u32) -> Result<Road> {
        (**self).road(road)
    }
    fn endpoint(&mut self, metric: &str, road: u32) -> Result<Endpoint> {
        (**self).endpoint(metric, road)
    }
    fn base(&mut self, metric: &str) -> Result<(Arc<base::Graph>, Arc<base::Costs>)> {
        (**self).base(metric)
    }
    fn has_landmarks(&self, metric: &str) -> bool {
        (**self).has_landmarks(metric)
    }
    fn landmarks(&self, metric: &str, starts: &[Seed], ends: &[Seed]) -> Result<Option<landmarks::Prepared>> {
        (**self).landmarks(metric, starts, ends)
    }
    fn routing_bytes(&self, metric: &str) -> Result<usize> {
        (**self).routing_bytes(metric)
    }
    fn memory_budget(&self) -> usize {
        (**self).memory_budget()
    }
    fn set_memory_budget(&mut self, bytes: usize) {
        (**self).set_memory_budget(bytes);
    }
}
