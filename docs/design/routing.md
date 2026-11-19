---
system: routing
kind: system
area: Data and services
purpose: "Finds a bike route between points."
owns:
  - planner/router/**
  - planner/router-build/**
  - planner/service/**
  - tools/planner_overlays.py
depends_on:
  - data-engine
  - source-preparation
test_depends_on: []
status: draft
---

# Routing

The body of this document is not written yet. Only the route facts are described.

## The route facts

The Explore page searches the routes of the catalog by facts that the records do not hold: the car
parks and the lift top stations near a route, the places that it passes, its highest point, and
the shares of its length that are paved, near a big road, under trees and along water. The bake
step `planner/route-facts` (`step::facts`, module [`facts`](../../planner/router-build/src/facts/mod.rs))
writes these facts into a file for each grid cell, beside the cell files of the catalog. The
[route catalog](../../specs/route-catalog.md#facts-files) gives the files and every rule.

The step runs after `planner/routing`, in the optional part `route-facts`, which needs `routing`.
It reads the route catalog, the highways of the routing source, the OSM extract of the region in
three passes (the relations, then the tagged nodes and the ways, then the nodes of those ways), the
GLO-30 heights of the lift stations without `ele`, and the canopy archive when the part
`trailview-canopy` is on. It puts the segments of all route lines in one grid of 3 km cells and
finds the routes near each car park, lift and place. It puts the samples of all routes, one for
each 100 m, in a grid of 200 m cells, and passes each highway and each way of water over it once.

| Rule | Check |
| --- | --- |
| The tags of an element decide its kind, and a private car park is no car park | `the_tags_decide_the_kind_of_an_element` in [tests.rs](../../planner/router-build/src/facts/tests.rs) |
| `capacity` decides the size of a car park, else the area | `the_capacity_decides_the_size_class_and_else_the_area` in [tests.rs](../../planner/router-build/src/facts/tests.rs) |
| A café in built-up land is no hut, and a lake without a name needs 1 ha | `areas_give_the_size_the_position_and_the_food_rule` in [tests.rs](../../planner/router-build/src/facts/tests.rs) |
| The samples are where `profile_m` has its entries | `the_samples_are_where_the_profile_has_its_entries` in [tests.rs](../../planner/router-build/src/facts/tests.rs) |
| The nearest highway decides the paved share | `the_nearest_highway_decides_the_paved_share_and_any_big_road_within_200_m_counts` in [tests.rs](../../planner/router-build/src/facts/tests.rs) |
| Each file holds the places that its entries name | `each_file_holds_the_routes_of_its_cell_and_the_places_that_they_name` in [tests.rs](../../planner/router-build/src/facts/tests.rs) |

Decisions:

- The facts are in files of their own, not in the records. A change of a fact then runs only
  `planner/route-facts` and its grid, not `planner/routing`, and the routing package does not
  change.
- The highways come from the routing source, which already joins their nodes. A second join of
  all highway nodes from the extract costs more time and memory.
- A share counts samples, not metres: the search needs a whole percent, and the samples are the
  points that the profile already has.
