//! The facts of each catalog route for the search of the Explore page: the car parks, the lifts
//! and the places near it, its shares of paved length, big road, canopy and water, and for a
//! one-way route the lift between its ends and the time back. The section "Facts files" of
//! `specs/route-catalog.md` defines the files.

pub mod geometry;
pub mod places;

use planner_router::shape::P;

use crate::source::Id;
use geometry::Line;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Parking,
    Lift,
    Peak,
    /// A hut with food: an alpine hut, or a restaurant, a cafe or a beer garden outside a town.
    Hut,
    Lake,
    Waterfall,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Parking => "parking",
            Kind::Lift => "lift",
            Kind::Peak => "peak",
            Kind::Hut => "hut",
            Kind::Lake => "lake",
            Kind::Waterfall => "waterfall",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Large,
    Small,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lift {
    pub valley: P,
    /// The car park of the valley station.
    pub parking: Option<Id>,
}

/// An OSM object that a route can pass or start from.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub id: Id,
    pub kind: Kind,
    pub name: Option<String>,
    /// The `ele` tag.
    pub ele_m: Option<i32>,
    /// The node; the top station of a lift; a point inside an area.
    pub at: P,
    /// Car parks only.
    pub size: Option<Size>,
    /// Lifts only.
    pub lift: Option<Lift>,
    /// The outer and inner rings of an area; empty for a point.
    pub rings: Vec<Vec<P>>,
}

impl Place {
    /// The distance from `line` to the place: to its point, or to the edge of its area and 0 when the
    /// line enters it. Infinite for an area whose edge is farther than `reach_m` from the box of
    /// the line.
    pub fn distance(&self, line: &Line, reach_m: f64) -> f64 {
        match self.rings.is_empty() {
            true => line.to_point(self.at),
            false => line.to_area(&self.rings, reach_m),
        }
    }
}
