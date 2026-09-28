# Regional routing package

## Identity and closure

A package directory contains `manifest.json` and `objects/<digest>` files.
A digest is lowercase SHA-256 as 64 hexadecimal characters. Each object name
is the digest of its complete stored bytes. The package ID is the digest of the
exact UTF-8 manifest bytes. Reformatting a manifest changes its ID.

The manifest format is `1`. Its JSON fields are defined by `Manifest` and
`Metric` in `host/route-engine/src/package.rs`. Bounds are
`[west, south, east, north]` in degrees. Metric IDs equal their profile names.
Source digests identify the input data. Attribution and warnings travel with
the package.

Every referenced object must be present in a complete download. Missing objects
are errors, not empty graph cells. Objects are immutable. A consumer must not
mix manifests or pages from different packages during a query.

## Objects

Objects are zlib-compressed Postcard 1 values. Serialized field and enum order
is the Rust declaration order in the types below. Format changes require a
new manifest format value. Compression output need not be canonical; the hash
always refers to the actual stored bytes.

| Manifest reference | Decoded type | Ordering |
| --- | --- | --- |
| `geometry` | `Vec<model::Road>` | Directed road ID, 128 roads per page |
| `spatial` | `Vec<u32>` | Sorted road IDs; key is `latitude_cell,longitude_cell` |
| Metric `endpoints` | `Vec<package::Endpoint>` | Directed road ID, 128 roads per page |
| Metric `graph` | `storage::Page` | CH rank, 128 states per page |

Cells span 10,000 microdegrees on each axis. Cell indices use floor division,
including for negative coordinates. A road appears in all cells crossed by
its geometry segment bounding boxes. The last page can contain fewer entries.

Points store integer microdegree coordinates and signed metre heights.
Height `-32768` means unknown. Surface enum order is unknown, paved, compacted,
gravel, dirt, rough. Access bits are bike `1`, foot `2`, and pushing `4`.
Profile costs are positive integer values for eligible roads. Excluded roads
have no cost. Endpoint arrival and departure values use CH ranks.

CH arcs point to a higher rank. A shortcut names two child arc references.
Leaf arcs name the original directed road. A reference contains its node rank,
arc index and forward/backward side. The unpacked witness must connect legal
directed roads and preserve the prepared total cost.

A stored object and its decoded payload must each fit within 8 MiB. The
manifest must fit within 64 MiB. Consumers verify hashes before decoding.
These bounds protect installation and query memory; they do not set region
coverage or a phone performance target.
