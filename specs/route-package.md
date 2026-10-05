# Regional routing package

## Identity and closure

A package directory contains `manifest.json`, `pages.idx`, and `pages.bin`.
A digest is lowercase SHA-256 as 64 hexadecimal characters. Each object name
is the digest of its complete stored bytes. The package ID is the digest of the
exact UTF-8 manifest bytes. Reformatting a manifest changes its ID.

The manifest format is `8`. Its JSON fields are defined by `Manifest` and
`Metric` in `host/route-engine/src/package.rs`. Bounds are
`[west, south, east, north]` in degrees. Metric IDs equal their profile names.
Source digests identify the input data. Attribution and warnings travel with
the package.

Bounds define query endpoint coverage.

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
store `Vec<String>` digests. Metric `allowed` stores `Vec<u64>` snap words.
Metric `costs` stores `Vec<u32>`. Manifest `costs` stores
`Vec<cost::CostBasis>`. These value tables refer directly to their blocks.
The last block contains the remaining entries. Consumers load bounded blocks.

Objects are zlib-compressed Postcard 1 values. Serialized field and enum order
is the Rust declaration order in the types below. Format changes require a
new manifest format value. Compression output need not be canonical; the hash
always refers to the actual stored bytes.

| Manifest reference | Decoded type | Ordering |
| --- | --- | --- |
| `geometry` | `geometry::Columns` | Directed road ID, 128 roads per page |
| `closures.sets` | `Vec<closures::Set>` | Distinct closure sets, by set index |
| `closures.roads` | Column | One set index per directed road |
| `spatial` directory | `BTreeMap<String, String>` | Fine cell key to road-list digest |
| Spatial road list | `Vec<u32>` | Sorted road IDs |
| Manifest `graph` | `base::Topology` direct tables | Directed road ID and ordered turns |
| Metric `weights` | `base::Weights` direct columns | Directed road ID and ordered turns |

The optional manifest `closures` is absent when no road has a possible closure.
`sets` is the digest of one object with the distinct closure sets. `roads` is a
column with one set index per directed road; the all-ones value means no
possible closure, and a finite value is below the number of sets. A set entry is
`(modes, {kind, condition})`, as [the route API](route-api.md#edges) lists the
kinds. A set keeps the order of its source: access values before conditions.

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

The package holds no source OSM objects. Source hashes and attribution identify
the input. Terrain samples belong to the directed road geometry.

Metric `allowed` holds one snap bit per directed road: road `i` uses bit `i % 64`
of word `i / 64`. Its table length is `ceil(roads / 64)`. A set bit means that
the metric has a finite cost for the road, and that the road lies in the
metric's largest strongly connected component of road states or in one of at
least `connectivity::MINIMUM_COMPONENT` states (`host/route-build`). Turn rules
and node access shape these components. Queries snap only to roads with a set
bit, so a leg without a path joins points that no legal route connects.

The manifest contains one directed topology shared by all metrics. A state is the
arrival at one directed road. Its state ID equals its road ID. An edge joins each
pair of roads whose physical endpoints meet, including pairs forbidden by some
or all metrics. Edges are unique and sorted by source road, then destination road.

`first` is a `Vec<u32>` table of length `roads + 1`. It starts at zero, ends at
the edge count, and never decreases. The outgoing edge range of road `i` is
`first[i]..first[i+1]`. `head` stores each destination road. `offsets` is the
narrowest width that holds every position within an outgoing range. A consumer
builds the incoming edges from `first` and `head`, and stores the position of
each incoming edge within its source's outgoing range at this width.

A column has a `width` and a direct value table. Width is `U8`, `U16`, `U32`, or
`U64`; its blocks store vectors of that exact unsigned type. The all-ones value
means unavailable. Finite values cannot use this sentinel. The writer chooses
the smallest width that holds every finite value without collision.

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

## Optional search bounds

`landmarks` can be absent. When present, it contains `junctions`, `mapping`,
and `profiles`. `junctions` is a positive integer. `mapping` has one arrival
junction ID per directed road. Each ID is less than `junctions`. `profiles` maps
prepared metric IDs to a positive integer `scale` and `tables`, one through 32
distance columns. Each column has `junctions` entries.

These tables store `Vec<i64>` deltas. Values accumulate from zero within each
4096-entry block. Mapping values fit `u32`. Distance values fit `u16`; 65535 is
a capped distance, not an unavailable-cost sentinel.

A distance column `d` of a profile must satisfy `scale * (d(u) - d(v)) <= w(u, v)`
for every legal road-state transition, where `u` and `v` use their mapped
arrival junctions and `w` is the exact transition cost. The builder computes
reverse junction distances with each legal road cost divided by the profile's
`scale` and rounded down. It omits turn penalties from these lower bounds. It
chooses the scale from the farthest junction to the first landmark plus the
farthest junction from it, so that no column of the connected component reaches
the cap; a distance outside the component saturates at 65535. The exact search
still uses all prepared road and turn costs.

## Grid selections

`routing/blocks.json` has `format: 2`. `source` is the source manifest SHA-256.
`data` uses the format 8 manifest structure with selected bounds and region.
Each sparse table adds `pages`, an ascending list of source page numbers,
parallel to `blocks`. `len` remains the source column length. An absent page
is unavailable, not an empty page.

`roads` contains sorted, disjoint half-open source road ranges. `arcs` is the
number of source outgoing transitions for those roads, before transitions to
absent roads are removed. `snap` maps source spatial keys to page hashes.
`archives` is the sorted unique list of pack IDs. Each pack is
`packs/ID/{pages.idx,pages.bin}` and uses the object encoding above.

A selection retains source geometry, weights, turn penalties, snap bits and
landmark values. Runtime road IDs are compact indices in source road order.
Queries use only transitions whose two roads are present. The snap bits come
from the whole region, so a road near the edge of a selection can be snappable
although the selection has no path to it; such a query returns `no_path`. The
source junction IDs
connect retained roads to the original landmark columns. The selection
manifest hash is its package identity. Different source releases cannot mix.
