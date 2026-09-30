# Regional routing package

## Identity and closure

A package directory contains `manifest.json`, `pages.idx`, and `pages.bin`.
A digest is lowercase SHA-256 as 64 hexadecimal characters. Each object name
is the digest of its complete stored bytes. The package ID is the digest of the
exact UTF-8 manifest bytes. Reformatting a manifest changes its ID.

The manifest format is `6`. Its JSON fields are defined by `Manifest` and
`Metric` in `host/route-engine/src/package.rs`. Bounds are
`[west, south, east, north]` in degrees. Metric IDs equal their profile names.
Source digests identify the input data. Attribution and warnings travel with
the package.

Bounds define query endpoint coverage. A bounding box extraction retains each
intersecting directed road in full. Its route geometry can extend outside these
bounds. The extraction command reports the complete geometry envelope as
`geometry_bounds`; this report field is not part of the package manifest.
Route optimality applies to the retained graph. Connections through omitted
roads are absent, even when a complete parent region contains such a route.

Every referenced object must be present in a complete download. Missing objects
are errors, not empty graph cells. Objects are immutable. A consumer must not
mix manifests or pages from different packages during a query.

## Objects

`pages.bin` contains the stored objects without separators. `pages.idx` starts
with the eight ASCII bytes `OBCRIDX3` and a little-endian `u64` record count.
Each 48-byte record contains a 32-byte SHA-256 digest, a little-endian `u64`
offset in `pages.bin`, and a little-endian `u64` stored length. Records are
sorted by digest bytes. Each digest occurs once. The index has no trailing
bytes. Offsets and lengths must lie within `pages.bin`.

A `Table` contains `len`, the number of entries, and `blocks`, the ordered
digests of its index blocks. Each block stores up to 4096 entries. Page tables
store `Vec<String>` digests. Metric `allowed` stores `Vec<u64>` access words.
Metric `costs` stores `Vec<u32>`. Manifest `costs` stores
`Vec<cost::CostBasis>`. These value tables refer directly to their blocks.
The last block contains the remaining entries. Consumers load bounded blocks.

Objects are zlib-compressed Postcard 1 values. Serialized field and enum order
is the Rust declaration order in the types below. Format changes require a
new manifest format value. Compression output need not be canonical; the hash
always refers to the actual stored bytes.

| Manifest reference | Decoded type | Ordering |
| --- | --- | --- |
| `osm.nodes` | `Vec<osm::Node>` | OSM ID, 128 nodes per page |
| `osm.ways` | `Vec<osm::Way>` | OSM ID, 128 ways per page |
| `osm.relations` | `Vec<osm::Relation>` | OSM ID, 128 relations per page |
| `geometry` | `geometry::Columns` | Directed road ID, 128 roads per page |
| `spatial` directory | `BTreeMap<String, String>` | Fine cell key to road-list digest |
| Spatial road list | `Vec<u32>` | Sorted road IDs |
| Manifest `graph` | `base::Topology` direct tables | Directed road ID and ordered turns |
| Metric `weights` | `base::Weights` direct columns | Directed road ID and ordered turns |

Cells span 10,000 microdegrees on each axis. Cell indices use floor division,
including for negative coordinates. A road appears in all cells crossed by
its geometry segment bounding boxes. The last page can contain fewer entries.
The manifest's spatial keys are one-degree cells. A fine cell's directory key
is `floor(latitude_cell / 100),floor(longitude_cell / 100)`. Fine cell keys are
`latitude_cell,longitude_cell`. Each directory has at most 10,000 cells.

Points store integer microdegree coordinates and `f32` metre heights.
Height `f32::MIN` means unknown. Surface enum order is unknown, paved, compacted,
gravel, dirt, rough. Access bits are bike `1`, foot `2`, and pushing `4`.
A road stores its source way ID and direction relative to that way.

Geometry columns store roads with empty shapes, one shape length per road,
latitude deltas, longitude deltas, and elevation bit deltas. Point columns
follow road and shape order. Coordinates accumulate from zero across the page.
Elevation stores each `f32` bit pattern XOR the previous bit pattern, with zero
as the first predecessor. Column lengths must equal the sum of shape lengths.
Reconstruction preserves every coordinate and elevation bit.

Source pages preserve all tags on retained highway and ferry ways, their
referenced nodes within bounds, and relations that contain these elements or
other retained relations. Members retain IDs, types, order and roles. References
outside the clipped source set remain IDs; their objects need not be present.
Source node heights remain unknown; an `ele` tag remains source text. Terrain
samples belong to the directed road geometry. An extraction from a prepared
package can retain its complete source OSM object closure, including objects
outside the requested bounds.

A runtime-only package can have empty OSM tables after all routing attributes
and overlay features are compiled. Source hashes and attribution stay present.
Build inputs stay on the preparation host. Removing source tables does not
remove road geometry, access rules, turn rules, costs, or overlay information.

Metric `allowed` holds one bit per directed road: road `i` uses bit `i % 64`
of word `i / 64`. Its table length is `ceil(roads / 64)`. It agrees with endpoint
cost eligibility and finite road costs. Queries use it to snap without reading
geometry cost curves.

The manifest contains one directed topology shared by all metrics. A state is the
arrival at one directed road. Its state ID equals its road ID. An edge joins each
pair of roads whose physical endpoints meet, including pairs forbidden by some
or all metrics. Edges are unique and sorted by source road, then destination road.

`first` and `reverse_first` are `Vec<u32>` tables of length `roads + 1`.
They start at zero, end at the edge count, and never decrease. The outgoing edge
range of road `i` is `first[i]..first[i+1]`. `head` stores each destination road.
The incoming range uses `reverse_first`; `reverse_tail` stores each source road.
`reverse_offsets` stores the local index within that source's outgoing range.
Every reverse entry must identify the same forward edge.

A column has a `width` and a direct value table. Width is `U8`, `U16`, `U32`, or
`U64`; its blocks store vectors of that exact unsigned type. The all-ones value
means unavailable. Finite values cannot use this sentinel. The writer chooses
the smallest width that holds every finite value without collision. Reverse
edge offsets are always finite.

Each metric stores two columns. `road_costs` has one exact total cost per road.
`turns` has one exact entry penalty per edge in the shared topology order.
An unavailable road or turn cannot be traversed. A legal transition costs the
turn penalty plus the destination road cost. Both addition and accumulated path
cost use checked `u64` arithmetic. Endpoint departures are the finite incoming
turns of the selected road. No separate endpoint topology is stored.

Metric `costs` has one entry per road. Zero means excluded. Other values index
manifest `costs`, starting at one. A `CostBasis` contains an exact `f64` factor,
`f64` bend coefficient, and ferry flag. The distance cost is the road length in
metres converted to `f64`, multiplied by the factor. Binary Postcard storage
preserves the factors' floating point bits. The shared cost implementation
derives the `RoadCost` curve from these values, the directed road geometry, and
the metric profile. Preparation and queries use the same operation order and
rounding. Curve fractions and cumulative penalties are nondecreasing. Equal
fractions encode a step. The last fraction is `1`, unless there are no penalties.
Total cost is rounded to a positive integer. Rounded interpolated prefix
differences give partial costs.

A stored object and its decoded payload must each fit within 8 MiB. The
manifest must fit within 128 MiB. Consumers verify hashes before decoding.
These bounds protect installation and query memory; they do not set region
coverage or a phone performance target.
