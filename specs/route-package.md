# Regional routing package

The routing package of one region, format 10: a binary cell directory, a JSON description, and one
compressed object for each part of each cell. An installer writes the parts of a set of cells as
files, and the route engine reads those files.

The bake also writes format 9 until the old engine is deleted, and the code of format 9 stays until
then. The contract of format 9 is this file on `develop`.

## Version and readers

- Version: `format` 10 in `package.json` and in `directory.bin`; directory magic `OBCROUTE`;
  install marker magic `OBCRINST`. A change of a byte layout needs a new format value.
- Writer: [`planner/router-build`](../planner/router-build/README.md), `cells::write`, in the step
  `planner/routing` of `obc data`. The step `planner/routing/cells/grid` publishes the files.
- Readers: `planner/router/src/install.rs` (the installer of the server and of the phone) and the
  views of `planner/router/src/cells.rs`.
- Code authority: `planner/router/src/cells.rs` (`Directory`, `Topology`, `Group`, `Table`,
  `Geometry`, `encode_road`, `decode_road`, `encode_table`).
- Related: [route API](route-api.md), [planner release](planner-release.md), [obc data](obc-data.md)
  (the option `download_level`).

All integers are little-endian. A **width** is 1, 2 or 4 bytes. A column of a level-1 cell has the
narrowest width whose all-ones value is above each finite value of the column in that cell. A
column with no finite value has width 1. In `costs` and `turns`, the all-ones value of the width
means "cannot use". A varint is unsigned LEB128. A signed value is zigzag-encoded first. Each
offset is from the start of the part. Each field with a name that ends in `_at` is the offset of
its section, also when the section is empty. In a part, each section, each block and each list
starts at a multiple of 4, and the class table at a multiple of 8. A column of a block starts at a
multiple of its width. Only the zero bytes that this needs lie between two items. A reader reads
each integer with no alignment need. A digest is SHA-256 as 64 lower-case hexadecimal characters.

## Files and identity

`routing/package.json` is compact JSON with sorted keys:

| Key | Value |
| --- | --- |
| `format` | 10 |
| `region`, `bounds`, `source_sha256`, `attribution`, `warnings` | As in format 9. Bounds are `[west, south, east, north]` in degrees |
| `profiles` | Sorted by name. Each is `{"group", "profile"}`: the profile group number and the `Profile` of the engine |
| `levels` | The size limit of each level of the package, level 1 first: the first values of `routing.cell_sizes`, one for each level of the partition |
| `download_level` | The download level of the package |
| `counts` | Exactly `cells` (the cell count of each level, level 1 first), `long_roads`, `roads` and `turns` |

`routing/directory.bin` is the cell directory. `routing/objects/SHA256` is an object, named by the
digest of its bytes. A release that has `routing/directory.bin` holds a package of format 10.

The identity of the package is the digest of `directory.bin`. The directory holds the digest of
`package.json`, and the digests of each object and each part. A route answer names the identity
of the directory as its `package`.

## Cells, levels and road IDs

The partition of the roads layer divides the junctions into nested cells of up to 8 levels. A
road is in the cell of its arrival junction. The cells of each level are one range of road IDs,
in ascending cell ID. A cell ID has one field for each level, the top level in the high bits. The
ID of a cell of level ℓ has zero bits in the fields of the levels below ℓ.

The **download level** is the smaller of the setting `download_level` and the number of levels.
A **download cell** is a cell of the download level. An **upper cell** is a cell of a higher level.
These cells have objects. No object is without a cell.

The run-time ID of a road is `first_road` of its download cell plus its index in the cell. It is
the number of the road in the cell order of the bake, so format 9 of the same bake names each road
with the same number. No published byte holds it. The IDs of an install of some cells are sparse.

A **road reference** has 8 bytes: `cell` (u32, a download cell ID) and `index` (u32). Its run-time
ID is `first_road` of that cell plus `index`. Each list of road references is in ascending order
of that ID. A **long road** is a road whose `length_m` is above 10,000.

## Objects

An object is one zstd frame of one part of one cell: level 12, a window of at most 8 MiB, with
the content size, without a checksum or a dictionary, made on one thread. A part is smaller than
4 GiB. The parts of a download cell are part 0 the topology, part 1 the geometry, and part 2 + g
profile group g. An upper cell has part 0, the topology with its two road lists, and part 2 + g
for each group, with its cost tables. Its part 1 is absent.

Two profiles are one **profile group** when their turn costs are equal. The groups are numbered by
their first profile in name order. A package has at most 32 groups.

## Directory

The header has 64 bytes, then come `cells` cell records.

| Header field | Type | Meaning |
| --- | --- | --- |
| `magic` | 8 bytes | `OBCROUTE` |
| `format` | u32 | 10 |
| `levels`, `download_level`, `groups`, zero | 4 × u8 | The number of levels (at most 8), the download level, the number of profile groups (at most 32) |
| `level_bits` | 8 × u8 | The bit count of the ID field of each level, level 1 first |
| `cells`, `roads` | 2 × u32 | The number of cell records; the roads of the package |
| `info` | 32 bytes | SHA-256 of `package.json` |

A cell record has 48 + 72 × (2 + `groups`) bytes. The download cells come first, in ascending ID,
then the upper cells in the order of level and ID.

| Field | Type | Meaning |
| --- | --- | --- |
| `level`, zero | u8, 3 bytes | The level of the cell |
| `id` | u32 | The cell ID |
| `first_road`, `roads` | 2 × u32 | The run-time ID of its first road; its road count |
| `box` | 4 × i32 | West, south, east, north in microdegrees: the union of the boxes of its level-1 cells. No long road is in it |
| `long_box` | 4 × i32 | The union of the boxes of the long roads of a download cell. All zero when it has none, and in an upper cell |
| `parts` | 72 bytes each | Part 0 topology, part 1 geometry, part 2 + g profile group g. Part 1 of an upper cell is absent |

A part entry is `object` (32 bytes, SHA-256 of the object), `object_bytes` (u32), `sha256` (32
bytes, SHA-256 of the part) and `bytes` (u32). An absent part is all zero. The first digest finds
and checks the download. The second checks the installed file, also after the object is deleted.

## Topology part

Sections in this order: the header, the level-1 records, the blocks in cell order, the upper
records, and for each upper record its entering list and then its leaving list.

The header has 24 bytes: `cells`, `roads`, `turns`, `upper` and `upper_at` (u32 each), and zero. A
level-1 record has 28 bytes:

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | u32 | The ID of the level-1 cell |
| `first_road`, `roads`, `turns` | 3 × u32 | Its first road as an index in the download cell; its counts |
| `block` | u32 | The offset of its block |
| `entering`, `leaving` | 2 × u16 | Its border roads of level 1. The entering roads are its first roads |
| `first_width`, `head_width`, zero | 2 × u8, 2 bytes | The widths of its two columns |

The block of a level-1 cell, in this order:

| Column | Entries | Meaning |
| --- | --- | --- |
| `first` | `roads` + 1 | The turns of road `i` are `first[i]..first[i+1]`, counted from the first turn of the cell |
| `head` | `turns` | The target of a turn. A value below `roads` is a road of this cell. Another value, less `roads`, is an index in `leaving`. The turns of a road are in ascending order of this value |
| `leaving` | `leaving` road references | The roads that depart in this cell and arrive in another |
| `departure` | `entering` × u32 | The level-1 cell ID of the departure junction of each entering road |

A turn joins two roads that meet at a junction, also where a profile forbids it.

An upper record has 24 bytes: `level` (u8), 3 zero bytes, `id`, `entering_at`, `entering`,
`leaving_at` and `leaving` (u32 each). The records are in the order of level and ID. They give the
two lists of road references of each cell of level 2 to the download level in this download cell.
The topology part of an upper cell has no level-1 cell and one such record. The entering roads of
a cell of level ℓ are, in each of its level-1 cells, the first roads that arrive from another cell
of level ℓ. Its leaving roads depart in it and arrive in another cell of level ℓ. The place of a
road in an entering list is its row in the cost tables of the cell; its place in a leaving list is
its column. For level 1, the row is the index of the road in its cell.

## Profile group part

The profiles of the group are in the order of `package.json`. Sections in this order: the header,
the level-1 records, the blocks, the class table, the slots, the tables.

The header has 24 bytes: `cells`, `upper`, `profiles`, `classes`, `classes_at` and `slots_at` (u32
each). A level-1 record is `block` (u32), `turn_width`, `class_width`, one `cost_width` for each
profile (u8 each), and zero bytes to a multiple of 4. The block:

| Column | Entries | Meaning |
| --- | --- | --- |
| `turns` | `turns` | The cost of each turn, in the order of `head`. All-ones: the profiles of the group cannot use it |
| `classes` | `roads` | The cost class of each road: an index in the class table |
| `costs`, for each profile | `roads` | The total cost of each road. All-ones: the profile excludes it |
| `allowed`, for each profile | `roads` bits | The snap bit: road `i` is bit `i % 8` of byte `i / 8`. The bits of the last byte above the last road are zero |

The class table has, for each class and each profile, 24 bytes: `factor` (f64), `turn` (f64) and
`ferry` (u64, 0 or 1). A **cost class** is the list of the cost bases of a road for the profiles of
the group. Factor 0 means that the profile excludes the road, and its entry is all zero. The
classes are in ascending order of their bytes. Each download cell has its own classes.

The slots are, for each profile, one slot for each level-1 cell and then one for each upper record
of the topology part: `at` and `bytes` (u32 each). A slot names the cost table of that cell and
profile. The tables lie by profile, then level, then cell, each at a multiple of 4. A group part of
an upper cell has only the header, the slots and the table of that cell: `cells` and `classes` are
0, `upper` is 1, and `classes_at` and `slots_at` are 24. Each upper cell has a group part for each
group.

### Cost tables

In these rules, `C` is a cell of level ℓ and `p` a profile. An entering road `e` of `C` departs
outside `C` and arrives in `C`. A leaving road `l` departs in `C` and arrives outside it.

- **T1. Value.** `T[e][l]` is the least sum, over the turns of a path `e = r0, r1, …, rk = l`
  (k ≥ 1) whose roads `r1 … rk−1` arrive in `C`, of the turn cost plus the total cost of the
  target road. It does not include the cost of `e`. So the forward search sets
  `cost(l) = cost(e) + T[e][l]`, and the backward search sets `cost(e) = T[e][l] + cost(l)`.
- **T2. No path.** All-ones of the width of the table means that no such path exists. A turn with
  an all-ones turn cost or target cost is not a turn. No road is an entering and a leaving road of
  the same cell, and no path in a cell reaches an entering road again.
- **T3. Rows and columns.** Level 1: the row of `e` is its index in the level-1 cell, and the
  column of `l` is its place in the leaving list of the cell. Level 2 and up: the row and the
  column are the places in the entering and the leaving list of the upper record. Each list
  ascends by run-time road ID.
- **T4. Presence.** Each cell of each level has a table for each profile of each group part that
  has its slot. A table with no entering or no leaving road is a header of 16 bytes, so no slot
  has 0 bytes.
- **T5. Bytes.** As in the table below. Without the copy by column, a reader reads a column with a
  stride of `columns × width` bytes. The view `Table` gives each value as `u64`, with `u64::MAX`
  for no path.
- **T6. Range.** Each finite value is at most 2^32 − 2. A reader adds values in `u64`.
- **T7. Unpacking.** For each finite `T[e][l]` of level ℓ ≥ 2, a search from `e` over the tables
  of the cells of level ℓ − 1 in `C` reaches `l` with exactly that cost. For level 1, a search from
  `e` on the roads that arrive in the cell reaches `l` with exactly that cost. The path can be
  another path of the same cost.

| Field | Type | Meaning |
| --- | --- | --- |
| `rows`, `columns` | 2 × u32 | The entering and the leaving roads of the cell; the view checks them against the record |
| `width` | u8 | 1, 2 or 4: the narrowest width whose all-ones value is above each finite value. A table with no finite value has width 1 |
| `layout` | u8 | 0: by row; 1: by row, then by column |
| zero | 6 bytes | |
| by row | `rows × columns` values | Value (r, q) at 16 + (r × `columns` + q) × `width` |
| by column | `rows × columns` values, when `layout` is 1 | Value (r, q) at 16 + (`rows` × `columns` + q × `rows` + r) × `width` |

The writer sets `layout` to 1 when the values by row take more than 16,384 bytes and the table has
more than one row and more than one column. Then a column read with a stride touches more than 4
pages of 4 KiB or more than one page of 16 KiB. A reader accepts layout 0 and layout 1 for each
table and does not check this threshold.

A legal transition costs the turn cost plus the cost of the destination road. The addition and
the path cost use checked `u64` arithmetic. The departures of an end point are the finite turns
into the road.

A `CostBasis` holds an exact `f64` factor, an `f64` bend coefficient and a ferry flag. The distance
cost is the road length in metres as `f64`, times the factor. The shared cost code of the engine
makes the `RoadCost` curve from the basis, the road geometry and the profile. The bake and the
queries use the same order of operations and the same rounding. The fractions of a curve and its
penalties do not decrease. Equal fractions are a step. The last fraction is `1`, unless the curve
has no penalty. The total cost is rounded to a positive integer. The difference of two rounded,
interpolated prefixes gives the cost of a part of a road.

A set snap bit means that the profile has a finite cost for the road, and that the road is in the
largest strongly connected component of road states of the profile, or in one of at least
`connectivity::MINIMUM_COMPONENT` states (`planner/router-build`). The turn rules and the access of
the junctions shape these components. A query snaps only to a road with a set bit.

## Geometry part

Sections in this order: the header, the level-1 records, the blocks, the long list, the set table.

The header has 24 bytes: `cells`, `long`, `long_at`, `sets` and `sets_at` (u32 each), and zero. The
long list has one record of 20 bytes for each long road of the download cell, in ascending order
of the road: `road` (u32, its index in the download cell) and the box of its shape (4 × i32).

A level-1 record has 32 bytes: `box` (4 × i32), `roads`, `block` and `bytes` (u32 each),
`offset_width` (u8) and 3 zero bytes. The box is the box of the shapes of the roads of the cell
that are not long. When each road of the cell is long, it is the last point of the first road; in
a cell with no road, it is the point of the first junction of the cell. `bytes` is the size of the
records of the block. `offset_width` is 2 when `bytes` is below 65,535, else 4. The block is
`roads` + 1 offsets from its first record, then the records. A reader decodes a record with only
the box of its cell:

| Field | Type | Meaning |
| --- | --- | --- |
| `way` | varint | The OSM way ID |
| `use` | u8 | Bits 0–2 the surface (unknown, paved, compacted, gravel, dirt, rough), bits 3–5 the access (bike 1, foot 2, pushing 4) |
| `scales` | u8 | Bits 0–3 the MTB scale (0 to 6; 15 is none), bits 4–7 the SAC scale (0 to 6; 14 is the unknown value 255; 15 is none) |
| `length_m` | varint | The length in metres |
| `closure` | varint | 0, or 1 plus the index of its closure set |
| `points` | varint | The number of points, at least 2 |
| positions | signed varints | Point 0: latitude less `south`, longitude less `west`. Point 1: its step from point 0. Each later point: its step less the step before. Latitude first |
| heights | varints | For each point, the bits of its `f32` height XOR the bits of the point before; zero before point 0. `f32::MIN` is unknown |

Coordinates are integer microdegrees. A record keeps each coordinate and each height bit. The
record has no junction.

The set table is `sets` + 1 offsets (u32) from its first set, then the closure sets in the order of
their first road. A set is a count (u8) and its entries: `modes` (u8), `kind` (u8, the order of
`closures::Kind`: permit, private, farm, sidepath, discouraged, limited, seasonal, conditional,
unclear), and the condition as a varint length and UTF-8. A set keeps the order of its source:
access values before conditions. [The route API](route-api.md#edges) lists the kinds.

## Installed files and the installer

An install is one folder: `directory.bin`, `package.json`, `cells/LEVEL-ID-PART` (`LEVEL` and
`PART` decimal, `ID` as 8 lower-case hexadecimal digits), `install.lock` and `install.bin`. An
installed file is one part, without compression. `install.bin` is `OBCRINST` (8 bytes), the
SHA-256 of the directory, `groups` (u32, bit g set when group g is installed), `count` (u32) and
`count` record numbers of the directory (u32, ascending). Each installed cell has the parts of
each installed group: a download cell has 2 files and one for each installed group, an upper cell
1 file and one for each installed group. The server has each cell; a phone has fewer cells in the same
layout.

`install::install` installs a set of download cells and profile groups:

1. It takes an exclusive lock of `install.lock` without a wait, and fails when another call holds
   it.
2. It checks `package.json` against the directory. It fails when the folder has a `directory.bin`
   or a `package.json` with other bytes. It writes a missing one through a temporary file, a sync
   and a rename.
3. It deletes each `*.part` in `cells/`. For each missing file, a worker decompresses the object,
   hashes the object and the part, writes `NAME.part`, compares the two hashes and sizes with the
   directory, syncs, drops the two files from the page cache, and renames. A wrong object stops
   the install with the name of the object.
4. It syncs `cells/`, writes `install.bin` through a temporary file, a sync and a rename, and syncs
   the folder.
5. It deletes each file in `cells/` that the install does not name, and nothing outside `cells/`.

The install adds each upper cell whose download cells are all in the install, with its topology part
and the parts of the installed groups. It takes no part of an upper cell that it holds only in
part; the search treats that cell as open (rule 3). `install::check`
checks each object without a file. `install::scrub` reads each installed file again.

The rules of a reader:

1. **Road ID.** Each host uses the run-time ID. A leg position (`road:fraction`) names the same road
   on the server and on a phone that has its cell. No array of a reader has an entry for each road
   ID of the package.
2. **Missing cell.** A turn from or into a download cell that is not installed is not a turn, also
   in the backward search and in the departures of an end point. A request point is
   `MissingRegion` when it is outside the bounds of the package, or when a download cell whose box
   meets the snap window is not installed. The snap window is, on each side of the point, the
   radius divided by 0.110 m for each microdegree (for the longitude also by the cosine of the
   latitude), rounded up, plus 2 microdegrees, for each radius that the snap tries. A long road of
   a download cell that is not installed is skipped and gives no `MissingRegion`. A read of a part
   that the install does not have is `MissingRegion`.
3. **Tables.** The search treats a cell as open when its slot has 0 bytes, or when it is an upper
   cell that is not installed or has no group part. An upper cell is installed when, and only when,
   each download cell below it is installed. A profile of a group that is not installed is not
   installed: a request with it fails as an unknown profile does.
4. **Trust.** A reader opens only a folder with `install.bin` whose digest is that of
   `directory.bin`, and reads both once. It does not hash a part. An installed file never changes
   under its name: the installer only adds and removes files. A reader keeps the bytes of a file
   that it mapped before the installer removed it; a first read of a removed file is
   `MissingRegion`. A reader closes each file after it maps it.
5. **Snapping.** The roads near a point are the roads of each level-1 cell whose box meets the snap
   window, in the installed download cells whose `box` meets it, and each road of the long list
   whose box meets it, in the installed download cells whose `long_box` meets it. The snap bit and
   the distance then decide.
6. **Spur.** The spur check compares the way and the two end points of a road.

## Limits

A part and an object are smaller than 4 GiB. A level-1 cell has fewer than 65,536 entering and
65,536 leaving roads. A package has fewer than 2^32 roads and 2^32 turns, at most 8 levels, cell
IDs of at most 32 bits, and at most 32 profile groups. A closure set has at most 255 entries. An
MTB or SAC scale that the record cannot hold fails the bake. These limits do not set the size of a
region or a phone performance target.

## Grid selections

This section is of format 9; it goes with format 9. `routing/blocks.json` has `format: 2`. `source`
is the SHA-256 of the format 9 manifest. `data` has the structure of that manifest, with the bounds
and the region of the selection. Each sparse table adds `pages`, an ascending list of source page
numbers, parallel to `blocks`. `len` stays the length of the source column. An absent page is
unavailable, not an empty page.

`roads` holds sorted, disjoint half-open ranges of source roads. `arcs` is the number of source
turns of those roads, before the turns to absent roads are removed. `snap` maps source spatial
keys to page hashes. `archives` is the sorted list of pack IDs, each once. Each pack is
`packs/ID/{pages.idx,pages.bin}`, with the objects of format 9.

A selection keeps the source geometry, weights, turn costs, snap bits and landmark values. Its
run-time road IDs are compact indices in source road order. A query uses only the turns whose two
roads are present. The snap bits come from the whole region, so a road near the edge of a
selection can be snappable although the selection has no path to it; such a query returns
`no_path`. The source junction IDs join the kept roads to the landmark columns. The hash of the
selection manifest is its package identity. Selections of different source releases do not mix.
