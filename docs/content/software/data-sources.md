---
title: Data sources
description: The external data that each product reads, where, for what, and under which licence.
copy: ai
layout: wide
---

# Data sources

OpenBikeComputer makes its maps, routes, search and TrailView from external open data. Each
dataset, asset and tool that a bake reads is a row of the
[source registry](src:data/sources.toml). The guide build makes the tables of this page from that
file, so the page always shows the current registry.

## Why these sources

The licence comes first: it must let us publish what we bake from the data. The registry records
the licence and the credit of each source, and a data source or an asset without a licence is
blocked. Each product shows the credit exactly as the registry states it.

OpenStreetMap is the base of the maps, the routes and the search. Its contributors map trails and
tracks, and any rider can correct an error.

For the maps, the routes and the terrain, a global source covers every region, so no region has
a gap. Where a country or a continent publishes finer open data, the products also read it there:
the national terrain models, the HR-WSI snow layer and the CORINE land cover of Europe. Two
TrailView layers have no global source: the lithology reads maps of Europe, and the canopy reads
the forest reference of Switzerland.

The [data contract](src:specs/obc-data.md) defines the registry, how each layer reads its sources,
and where each credit goes.

## By product and layer

**Used by** names what reads the source: `device` (the device maps), `planner` (the route
planner: basemap, terrain, routes, climate and snow), `search` (the planner search) and
`trailview` (the TrailView layers of the planner). The use of a source says which layer reads it.

```sources
table: used_by
```

## By country or area

A national source names the Geofabrik areas that it covers. A source without areas covers the box
of its extent, in degrees west, south, east and north, or the world.

```sources
table: areas
```

## Licences and credits

The credit is the text that a product shows. **Redistribute** is the `redistribute` key that the
[data contract](src:specs/obc-data.md) defines: whether the licence lets us give the upstream
files to others.

```sources
table: licences
```
