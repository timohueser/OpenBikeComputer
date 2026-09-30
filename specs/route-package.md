# Regional routing package

## Identity and closure

A package directory contains `manifest.json`, `pages.idx`, and `pages.bin`.
A digest is lowercase SHA-256 as 64 hexadecimal characters. Each object name
is the digest of its complete stored bytes. The package ID is the digest of the
exact UTF-8 manifest bytes. Reformatting a manifest changes its ID.

The manifest format is `3`. Its JSON fields are defined by `Manifest` and
`Metric` in `host/route-engine/src/package.rs`. Bounds are
`[west, south, east, north]` in degrees. Metric IDs equal their profile names.
Source digests identify the input data. Attribution and warnings travel with
the package.

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
| `geometry` | `Vec<model::Road>` | Directed road ID, 128 roads per page |
| `spatial` directory | `BTreeMap<String, String>` | Fine cell key to road-list digest |
| Spatial road list | `Vec<u32>` | Sorted road IDs |
| Metric `endpoints` | `Vec<package::Endpoint>` | Directed road ID, 128 roads per page |
| Metric `graph` | `storage::Page` | CH rank, 128 states per page |

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

Source pages preserve all tags on retained highway and ferry ways, their
referenced nodes within bounds, and relations that contain these elements or
other retained relations. Members retain IDs, types, order and roles. References
outside the clipped source set remain IDs; their objects need not be present.
Source node heights remain unknown; an `ele` tag remains source text. Terrain
samples belong to the directed road geometry.

Metric `allowed` holds one bit per directed road: road `i` uses bit `i % 64`
of word `i / 64`. Its table length is `ceil(roads / 64)`. It agrees with endpoint
cost eligibility. Queries use it to snap without reading cost pages.

An eligible endpoint has a `RoadCost`: distance cost and cumulative penalties
at geometric length fractions. Fractions and penalties are nondecreasing;
equal fractions encode a step. The last fraction is `1`, unless there are no
penalties. Total cost is rounded to a positive integer. Rounded interpolated
prefix differences give partial costs. Excluded endpoints have no cost.
Endpoint arrivals and departure states use CH ranks. Each departure also holds
its turn and entry penalty. An arc adds this penalty to the entered road cost.
Arrivals share a state only when all departure permissions and penalties agree.

CH arcs point to a higher rank. A shortcut names two child arc references.
Leaf arcs name the original directed road. A reference contains its node rank,
arc index and forward/backward side. The unpacked witness must connect legal
directed roads and preserve the prepared total cost.

A stored object and its decoded payload must each fit within 8 MiB. The
manifest must fit within 128 MiB. Consumers verify hashes before decoding.
These bounds protect installation and query memory; they do not set region
coverage or a phone performance target.
