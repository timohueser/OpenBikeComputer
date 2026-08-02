// The cell catalog root: one schema, the skins, the named regions, and the pins on
// the per-band cell indices. Normative in `OBCC_Spec.md`, produced by
// `obc-pack catalog` / `obc-bake`.
//
// The whole-document contract is why this takes a *string*: a document
// is read whole and parsed as one JSON value, and a failure rejects all of it
// rather than leaving a half-populated catalog on screen. The guarantee spans
// four kinds of document, so the
// root **pins each satellite by `bytes` + `sha256`** and this parser's job
// extends to checking that the root is internally consistent before any of those
// satellites is fetched: a band with no cell index, a skin missing a feature
// type, a region whose `bytes_by_band` does not add up to its `bytes` — each is
// a document that would price a selection wrongly, and pricing is the one thing
// the builder promises to get right before a download starts (§6).
//
// The checks below are the spec's MUSTs, not a taste for strictness. Every
// consumer-side rejection has a `fail()` here with its reason in the message.

import { GRID_ORIGIN, MAX_CELL_LOG2, MIN_CELL_LOG2, WORLD_SIDE } from "./grid";
import {
    arr,
    bool,
    CatalogFormatError,
    fail,
    instant,
    int,
    intMap,
    json,
    KEBAB,
    obj,
    optionalStr,
    PATH_ID,
    pinnedUrlStr,
    SHA256,
    str,
} from "./parse";

export { CatalogFormatError };

/** The envelope version this client implements. Checked before any other field. */
export const CATALOG_SCHEMA_VERSION = 2;

/** Which physical file of a volume set a band's content assembles into
 *  (`OBCA_Spec.md` §5.1). */
export type BandRole = "core" | "coarse" | "geometry";

/** A non-geometry OBCM section a band may carry. */
export type BandSection = "nav" | "poi";

export interface GridConstants {
    origin_udeg: number;
    world_side_udeg: number;
}

export interface LodEntry {
    index: number;
    /** Meters-per-pixel upper bound; `null` for the `+inf` coarsest level. */
    max_mpp: number | null;
    band: string;
}

export interface BandEntry {
    id: string;
    /** `log2(S)` in µdeg. */
    cell_log2: number;
    lods: number[];
    sections: BandSection[];
    role: BandRole;
}

export interface StyleAssignment {
    id: number;
    feature_type: string;
}

export interface RoutingEntry {
    min_component_edges: number;
    profiles: string[];
}

export interface SchemaEntry {
    id: string;
    revision: number;
    name: string;
    description: string;
    obcm_version: number;
    grid: GridConstants;
    lods: LodEntry[];
    bands: BandEntry[];
    styles: StyleAssignment[];
    routing: RoutingEntry;
    chunk_size: number;
}

export interface SkinStyle {
    feature_type: string;
    color: number;
    weight: number;
    z_index: number;
    priority: number;
    dashed: boolean;
    color2: number | null;
}

export interface SkinPreview {
    url: string;
    bytes: number;
    sha256: string;
}

export interface SkinEntry {
    id: string;
    name: string;
    description: string;
    version: number;
    marker_color: number;
    styles: SkinStyle[];
    /** A digest-pinned canonical rendering, or null for older/generic catalogs. */
    preview: SkinPreview | null;
}

/** A region's simplified outline: rings of `[lat, lon]` integer microdegrees
 *  (§7). **Presentation only** — it MUST NOT be used to compute a cell set
 *  (that is stored, §6), to price a selection, or as a packer input bbox. */
export interface Boundary {
    tolerance_udeg: number;
    rings: [number, number][][];
}

export interface RegionEntry {
    id: string;
    /** This region's terrain selection, priced (§13.3); `null` for a catalog
     *  with no terrain block. */
    terrain: RegionTerrain | null;
    name: string;
    parent: string | null;
    boundary: Boundary;
    /** Total bytes of every cell in this region's set, across all bands. */
    bytes: number;
    /** Those bytes per band — the per-file split a volume set needs (§5.7). */
    bytes_by_band: Record<string, number>;
    cell_count: Record<string, number>;
    partial_cell_count_by_band: Record<string, number>;
    cells_url: string;
    cells_bytes: number;
    cells_sha256: string;
}

export interface CellIndexRef {
    band: string;
    cell_log2: number;
    /** Downloadable OBCM artifacts in the satellite. */
    cell_count: number;
    /** Canonical zero-byte cells represented by compact ranges. */
    known_empty_count: number;
    bytes: number;
    sha256: string;
    url: string;
}

/** A region's terrain price (§13.3). Deliberately **not** part of `bytes` /
 *  `bytes_by_band`: a rider may take the map without the raster, so the two
 *  prices are separate numbers and a consumer MUST present them separately. */
export interface RegionTerrain {
    cell_count: number;
    known_empty_count: number;
    bytes: number;
}

/** The root's `terrain` block (§13.1): a **second artifact class with its own
 *  revision track**, not a band and not covered by the OBCM lockstep.
 *
 *  Absent is complete and valid — everything degrades to "no elevation is known
 *  here", which is the behaviour of every map before terrain existed. A consumer
 *  MUST NOT synthesize elevation from any other source. */
export interface TerrainEntry {
    dataset_id: string;
    /** Opaque: compared for equality, never parsed. */
    dataset_version: string;
    /** `log2(P)` of the sample lattice, µdeg (`OBCT_Spec.md` §1.1). */
    posting_log2: number;
    /** `log2(S)` of the terrain cell, µdeg. Independent of any band's. */
    cell_log2: number;
    terrain_revision: number;
    /** The source dataset's required credit, verbatim. §13.5 makes taking it
     *  from here rather than hard-coding it a MUST, so a dataset change carries
     *  its own notice with it. */
    attribution: string;
    cell_index: TerrainIndexRef;
}

/** The root's pin on the single terrain index (§13.1). */
export interface TerrainIndexRef {
    cell_count: number;
    known_empty_count: number;
    bytes: number;
    sha256: string;
    url: string;
}

export interface Catalog {
    schema_version: number;
    generated_at: string;
    schema: SchemaEntry;
    skins: SkinEntry[];
    regions: RegionEntry[];
    cell_index: CellIndexRef[];
    /** `null` for a catalog that publishes no raster (§13). */
    terrain: TerrainEntry | null;
    /** The terrain revision the network band's baked ascents were integrated
     *  from (§13.4), or `null` when the store was baked with no terrain. Root
     *  level and deliberately not in `SchemaEntry`. */
    network_terrain_revision: number | null;
}

/** What a satellite has to match before it is believed (§9). */
export interface DocumentPin {
    bytes: number;
    sha256: string;
}

const ROLES: readonly BandRole[] = ["core", "coarse", "geometry"];
const SECTIONS: readonly BandSection[] = ["nav", "poi"];

const U8 = 255;
const U16 = 65_535;
const U32 = 4_294_967_295;

function parseGrid(v: unknown, where: string): GridConstants {
    const o = obj(v, where);
    const grid = {
        origin_udeg: int(o, "origin_udeg", where, GRID_ORIGIN, GRID_ORIGIN),
        world_side_udeg: int(o, "world_side_udeg", where, WORLD_SIDE, WORLD_SIDE),
    };
    // §4 restates the constants so no consumer hard-codes them — but this one
    // does, in `grid.ts`, because they are OBCA §1.1 format constants and every
    // cell id in the document was minted with them. A catalog stating anything
    // else is not a catalog whose ids this client can turn into squares, so it is
    // refused rather than silently computed with the wrong lattice.
    return grid;
}

function parseBands(v: unknown, where: string, lodCount: number): BandEntry[] {
    const raw = arr(v, where);
    if (raw.length === 0) fail(`${where}: a cell store needs at least one band`);
    const ids = new Set<string>();
    const lodOwner = new Map<number, string>();
    const sectionOwner = new Map<BandSection, string>();
    const bands = raw.map((entry, k) => {
        const at = `${where}[${k}]`;
        const o = obj(entry, at);
        const id = str(o, "id", at, KEBAB);
        if (ids.has(id)) fail(`${at}: band ${JSON.stringify(id)} is listed twice`);
        ids.add(id);
        const cellLog2 = int(o, "cell_log2", at, MIN_CELL_LOG2, MAX_CELL_LOG2);

        const lods = arr(o.lods, `${at}.lods`).map((l, n) => {
            if (typeof l !== "number" || !Number.isInteger(l) || l < 0 || l >= lodCount) {
                fail(`${at}.lods[${n}]: not a LOD of a ${lodCount}-level ladder`);
            }
            return l as number;
        });
        for (const lod of lods) {
            // §1.2's partition rule, first half: a LOD in two bands would be
            // written into the assembly twice.
            const other = lodOwner.get(lod);
            if (other) fail(`${at}: LOD ${lod} is in both band "${other}" and band "${id}"`);
            lodOwner.set(lod, id);
        }

        const sections = arr(o.sections, `${at}.sections`).map((s, n) => {
            if (typeof s !== "string" || !SECTIONS.includes(s as BandSection)) {
                fail(`${at}.sections[${n}]: expected "nav" or "poi"`);
            }
            return s as BandSection;
        });
        for (const section of sections) {
            const other = sectionOwner.get(section);
            if (other) fail(`${at}: the ${section} section is in both band "${other}" and band "${id}"`);
            sectionOwner.set(section, id);
        }

        const role = str(o, "role", at);
        if (!ROLES.includes(role as BandRole)) fail(`${at}: unknown role ${JSON.stringify(role)}`);
        return { id, cell_log2: cellLog2, lods, sections, role: role as BandRole };
    });

    for (let lod = 0; lod < lodCount; lod++) {
        // …and the second half: a LOD in no band is a map that is blank at that
        // zoom, which is not a thing to discover on a mountain.
        if (!lodOwner.has(lod)) fail(`${where}: LOD ${lod} is in no band`);
    }
    for (const section of SECTIONS) {
        if (!sectionOwner.has(section)) fail(`${where}: the ${section} section is in no band`);
    }

    // §5.1's roles, which decide which physical file each band's bytes land in.
    const cores = bands.filter((b) => b.role === "core");
    if (cores.length !== 1) fail(`${where}: exactly one band must have role "core", found ${cores.length}`);
    const core = cores[0];
    if (core.lods.length) {
        fail(
            `${where}: the core band "${core.id}" carries LOD(s) ${core.lods.join(", ")} — geometry belongs in a ` +
                "splittable shard, never in the one file a volume set cannot split",
        );
    }
    if (!core.sections.includes("nav") || !core.sections.includes("poi")) {
        fail(`${where}: the core band "${core.id}" must carry both the nav and POI sections`);
    }
    if (bands.filter((b) => b.role === "coarse").length > 1) {
        fail(`${where}: at most one band may have role "coarse"`);
    }
    for (const b of bands) {
        if (b.role === "core") continue;
        if (!b.lods.length) fail(`${where}: band "${b.id}" (role ${b.role}) carries no LOD`);
        if (b.sections.length) fail(`${where}: band "${b.id}" (role ${b.role}) carries a section; only the core may`);
    }
    return bands;
}

function parseSchema(v: unknown, where: string): SchemaEntry {
    const o = obj(v, where);
    const lodsRaw = arr(o.lods, `${where}.lods`);
    if (lodsRaw.length === 0) fail(`${where}.lods: the ladder is empty`);
    const bands = parseBands(o.bands, `${where}.bands`, lodsRaw.length);
    const bandById = new Map(bands.map((b) => [b.id, b]));

    const lods = lodsRaw.map((entry, k) => {
        const at = `${where}.lods[${k}]`;
        const e = obj(entry, at);
        // Coarsest first, and the ladder is dense: index k at position k. The
        // band table is keyed by these indices, so a gap or a reorder would make
        // "LOD 3" mean two different rungs in one document.
        const index = int(e, "index", at, k, k);
        // Absent and `null` both mean the `+inf` coarsest level: the JSON schema
        // does not require the key, and a parser stricter than the schema
        // refuses documents the generator is entitled to write.
        const maxMpp = e.max_mpp === undefined ? null : e.max_mpp;
        if (maxMpp !== null && (typeof maxMpp !== "number" || !Number.isFinite(maxMpp))) {
            fail(`${at}: max_mpp must be a number or null`);
        }
        const band = str(e, "band", at, KEBAB);
        const owner = bandById.get(band);
        if (!owner) fail(`${at}: band ${JSON.stringify(band)} is not in schema.bands`);
        if (!owner.lods.includes(index)) {
            fail(`${at}: band "${band}" does not list LOD ${index} — the ladder and the band table disagree`);
        }
        return { index, max_mpp: (maxMpp as number | null) ?? null, band };
    });

    const styleIds = new Set<number>();
    const featureTypes = new Set<string>();
    const stylesRaw = arr(o.styles, `${where}.styles`);
    if (stylesRaw.length === 0) fail(`${where}.styles: a schema assigns at least one style id`);
    const styles = stylesRaw.map((entry, k) => {
        const at = `${where}.styles[${k}]`;
        const e = obj(entry, at);
        const id = int(e, "id", at, 1, U8);
        const featureType = str(e, "feature_type", at);
        if (styleIds.has(id)) fail(`${at}: style id ${id} is assigned twice`);
        if (featureTypes.has(featureType)) fail(`${at}: feature type ${JSON.stringify(featureType)} appears twice`);
        styleIds.add(id);
        featureTypes.add(featureType);
        return { id, feature_type: featureType };
    });

    const routingAt = `${where}.routing`;
    const routingRaw = obj(o.routing, routingAt);
    const routing: RoutingEntry = {
        min_component_edges: int(routingRaw, "min_component_edges", routingAt, 0),
        profiles: arr(routingRaw.profiles, `${routingAt}.profiles`).map((p, k) => {
            if (typeof p !== "string" || !p.length) fail(`${routingAt}.profiles[${k}]: must be a non-empty string`);
            return p as string;
        }),
    };

    return {
        id: str(o, "id", where, KEBAB),
        revision: int(o, "revision", where, 1),
        name: str(o, "name", where),
        description: str(o, "description", where),
        obcm_version: int(o, "obcm_version", where, 0, U8),
        grid: parseGrid(o.grid, `${where}.grid`),
        lods,
        bands,
        styles,
        routing,
        chunk_size: int(o, "chunk_size", where, 0),
    };
}

function parseSkins(v: unknown, where: string, schema: SchemaEntry): SkinEntry[] {
    const raw = arr(v, where);
    if (raw.length === 0) fail(`${where}: a catalog offers at least one skin`);
    const seen = new Set<string>();
    const schemaTypes = new Set(schema.styles.map((s) => s.feature_type));
    return raw.map((entry, k) => {
        const at = `${where}[${k}]`;
        const o = obj(entry, at);
        const id = str(o, "id", at, KEBAB);
        if (seen.has(id)) fail(`${at}: two skins share the id ${JSON.stringify(id)}`);
        seen.add(id);

        const covered = new Set<string>();
        const styles = arr(o.styles, `${at}.styles`).map((s, n) => {
            const sat = `${at}.styles[${n}]`;
            const e = obj(s, sat);
            const featureType = str(e, "feature_type", sat);
            // §5: a skin naming a feature type the schema lacks is a stale
            // skin claiming a layer that no longer exists.
            if (!schemaTypes.has(featureType)) {
                fail(`${sat}: feature type ${JSON.stringify(featureType)} is not in schema.styles`);
            }
            if (covered.has(featureType)) fail(`${sat}: feature type ${JSON.stringify(featureType)} is styled twice`);
            covered.add(featureType);
            const color2 = e.color2;
            if (color2 !== null && color2 !== undefined) int(e, "color2", sat, 0, U16);
            return {
                feature_type: featureType,
                color: int(e, "color", sat, 0, U16),
                weight: int(e, "weight", sat, 0, U8),
                z_index: int(e, "z_index", sat, -128, 127),
                priority: int(e, "priority", sat, 1, 4),
                dashed: bool(e, "dashed", sat),
                color2: (color2 as number | null | undefined) ?? null,
            };
        });
        // …and a skin that misses one would ship a map with an invisible layer.
        if (covered.size !== schemaTypes.size) {
            const missing = [...schemaTypes].filter((t) => !covered.has(t));
            fail(`${at}: skin "${id}" styles nothing for ${missing.join(", ")}`);
        }

        // §3: sorted by id. Enforced for the same reason the cell index's
        // ordering is: a document whose order is stated and not kept is a
        // document a consumer cannot binary-search or diff.
        if (k > 0 && id <= str(obj(raw[k - 1], at), "id", at)) {
            fail(`${at}: skins must be sorted by id`);
        }

        const skin: SkinEntry = {
            id,
            name: str(o, "name", at),
            description: str(o, "description", at),
            version: int(o, "version", at, 0),
            marker_color: int(o, "marker_color", at, 0, U16),
            styles,
            preview:
                o.preview === undefined
                    ? null
                    : (() => {
                          const pat = `${at}.preview`;
                          const p = obj(o.preview, pat);
                          const sha256 = str(p, "sha256", pat, SHA256);
                          return { url: pinnedUrlStr(p, "url", sha256, pat), bytes: int(p, "bytes", pat, 0), sha256 };
                      })(),
        };
        return skin;
    });
}

function parseBoundary(v: unknown, where: string): Boundary {
    const o = obj(v, where);
    const rings = arr(o.rings, `${where}.rings`);
    if (rings.length === 0) fail(`${where}.rings: an outline has at least one ring`);
    return {
        tolerance_udeg: int(o, "tolerance_udeg", where, 0),
        rings: rings.map((ring, r) => {
            const at = `${where}.rings[${r}]`;
            const points = arr(ring, at);
            if (points.length < 4) fail(`${at}: a closed ring needs at least four points`);
            const out = points.map((p, n) => {
                const pair = arr(p, `${at}[${n}]`);
                if (pair.length !== 2 || !pair.every((c) => typeof c === "number" && Number.isInteger(c))) {
                    fail(`${at}[${n}]: expected [lat, lon] integer microdegrees`);
                }
                return [pair[0], pair[1]] as [number, number];
            });
            const first = out[0];
            const last = out[out.length - 1];
            if (first[0] !== last[0] || first[1] !== last[1]) fail(`${at}: the ring is not closed`);
            return out;
        }),
    };
}

function parseRegions(v: unknown, where: string, bandIds: Set<string>): RegionEntry[] {
    const raw = arr(v, where);
    const seen = new Set<string>();
    const regions = raw.map((entry, k) => {
        const at = `${where}[${k}]`;
        const o = obj(entry, at);
        const id = str(o, "id", at, PATH_ID);
        if (seen.has(id)) fail(`${at}: two regions share the id ${JSON.stringify(id)}`);
        // §3, as for the skins: the order is part of the document.
        if (k > 0 && id <= str(obj(raw[k - 1], at), "id", at)) {
            fail(`${at}: regions must be sorted by id`);
        }
        seen.add(id);

        const bytesByBand = intMap(o.bytes_by_band, `${at}.bytes_by_band`);
        const cellCount = intMap(o.cell_count, `${at}.cell_count`);
        const partialByBand = intMap(o.partial_cell_count_by_band, `${at}.partial_cell_count_by_band`);
        for (const band of [
            ...Object.keys(bytesByBand),
            ...Object.keys(cellCount),
            ...Object.keys(partialByBand),
        ]) {
            if (!bandIds.has(band)) fail(`${at}: band ${JSON.stringify(band)} is not in schema.bands`);
        }
        const bytes = int(o, "bytes", at, 0);
        const summed = Object.values(bytesByBand).reduce((a, b) => a + b, 0);
        // §6: the split is what makes pricing per *file* rather than merely
        // per set, so a split that does not add up is a price that is wrong for
        // at least one file — and the core file's price is a hard ceiling.
        if (summed !== bytes) {
            fail(`${at}: bytes_by_band sums to ${summed}, but bytes is ${bytes}`);
        }
        for (const [band, count] of Object.entries(partialByBand)) {
            const bandCells = Object.hasOwn(cellCount, band) ? cellCount[band] : 0;
            if (count > bandCells) {
                fail(`${at}: partial_cell_count_by_band.${band} ${count} exceeds that band's ${bandCells} cells`);
            }
        }
        const cellsSha256 = str(o, "cells_sha256", at, SHA256);
        // §13.3. Optional, because a terrain-less catalog is complete — but its
        // bytes are separate from `bytes` on purpose, so nothing here folds them
        // into the sum checked above.
        const terrain: RegionTerrain | null =
            o.terrain === undefined || o.terrain === null
                ? null
                : (() => {
                      const tat = `${at}.terrain`;
                      const t = obj(o.terrain, tat);
                      return {
                          cell_count: int(t, "cell_count", tat, 0, U32),
                          known_empty_count: int(t, "known_empty_count", tat, 0, U32),
                          bytes: int(t, "bytes", tat, 0),
                      };
                  })();

        return {
            id,
            terrain,
            name: str(o, "name", at),
            parent: optionalStr(o, "parent", at, PATH_ID),
            boundary: parseBoundary(o.boundary, `${at}.boundary`),
            bytes,
            bytes_by_band: bytesByBand,
            cell_count: cellCount,
            partial_cell_count_by_band: partialByBand,
            cells_url: pinnedUrlStr(o, "cells_url", cellsSha256, at),
            cells_bytes: int(o, "cells_bytes", at, 0),
            cells_sha256: cellsSha256,
        };
    });
    for (const region of regions) {
        // The generator sets `parent` to the nearest *published* enclosing
        // region, so a dangling one is a bakery bug, and a picker that drew it
        // would offer a breadcrumb to nowhere.
        if (region.parent && !seen.has(region.parent)) {
            fail(`${where}: region "${region.id}" names a parent "${region.parent}" that is not in the catalog`);
        }
    }
    return regions;
}

function parseCellIndexRefs(v: unknown, where: string, bands: BandEntry[]): CellIndexRef[] {
    const raw = arr(v, where);
    const byId = new Map(bands.map((b) => [b.id, b]));
    const seen = new Set<string>();
    const refs = raw.map((entry, k) => {
        const at = `${where}[${k}]`;
        const o = obj(entry, at);
        const band = str(o, "band", at, KEBAB);
        const owner = byId.get(band);
        if (!owner) fail(`${at}: band ${JSON.stringify(band)} is not in schema.bands`);
        if (seen.has(band)) fail(`${at}: band ${JSON.stringify(band)} has two cell indices`);
        seen.add(band);
        const cellLog2 = int(o, "cell_log2", at, MIN_CELL_LOG2, MAX_CELL_LOG2);
        if (cellLog2 !== owner.cell_log2) {
            fail(`${at}: cell_log2 ${cellLog2} disagrees with band "${band}"'s ${owner.cell_log2}`);
        }
        const sha256 = str(o, "sha256", at, SHA256);
        return {
            band,
            cell_log2: cellLog2,
            cell_count: int(o, "cell_count", at, 0, U32),
            known_empty_count: int(o, "known_empty_count", at, 0, U32),
            bytes: int(o, "bytes", at, 0),
            sha256,
            url: pinnedUrlStr(o, "url", sha256, at),
        };
    });
    for (const band of bands) {
        // A band with no index is a band whose cells cannot be priced or
        // fetched — the assembly would be missing a whole LOD range or the nav
        // graph, so there is nothing partial to salvage here.
        if (!seen.has(band.id)) fail(`${where}: band "${band.id}" has no cell index`);
    }
    for (let k = 1; k < refs.length; k++) {
        if (refs[k].cell_log2 > refs[k - 1].cell_log2) {
            fail(`${where}: entries must be sorted by cell_log2 descending`);
        }
    }
    return refs;
}

/**
 * §13.1's terrain block, or `null`.
 *
 * The whole block is optional; every field inside it is required when it is
 * present. That asymmetry is the spec's, and it is the right one: a catalog
 * without terrain says "no elevation is known here", while a catalog with half a
 * terrain block says nothing a consumer can act on — it could not verify a cell's
 * lattice, price the download, or credit the source.
 */
function parseTerrain(v: unknown, where: string): TerrainEntry | null {
    if (v === undefined || v === null) return null;
    const o = obj(v, where);
    const refAt = `${where}.cell_index`;
    const ref = obj(o.cell_index, refAt);
    const sha256 = str(ref, "sha256", refAt, SHA256);
    // §13.5: a producer MUST NOT publish an empty attribution, and a consumer
    // that displays terrain MUST take the string from here. An empty one would
    // ship derived Copernicus raster with no credit at all, which is a licence
    // problem rather than a cosmetic one.
    const attribution = str(o, "attribution", where);
    if (!attribution.trim()) fail(`${where}.attribution: a terrain block credits its source (§13.5)`);
    return {
        dataset_id: str(o, "dataset_id", where, KEBAB),
        dataset_version: str(o, "dataset_version", where),
        // The OBCT §4.5 ranges, checked here so a shard can never be planned at
        // a pairing the format does not admit.
        posting_log2: int(o, "posting_log2", where, 4, 16),
        cell_log2: int(o, "cell_log2", where, MIN_CELL_LOG2, MAX_CELL_LOG2),
        terrain_revision: int(o, "terrain_revision", where, 1),
        attribution,
        cell_index: {
            cell_count: int(ref, "cell_count", refAt, 0, U32),
            known_empty_count: int(ref, "known_empty_count", refAt, 0, U32),
            bytes: int(ref, "bytes", refAt, 0),
            sha256,
            url: pinnedUrlStr(ref, "url", sha256, refAt),
        },
    };
}

/** Parse the current catalog root, or throw. */
export function parseRoot(body: string): Catalog {
    const root = obj(json(body, "catalog"), "catalog");

    // Before any other field (§7): a document from another envelope may spell
    // everything below differently, so nothing else is worth reading.
    if (root.schema_version !== CATALOG_SCHEMA_VERSION) {
        fail(
            `schema_version ${JSON.stringify(root.schema_version)} is not supported ` +
                `(this client reads ${CATALOG_SCHEMA_VERSION})`,
        );
    }
    const schema = parseSchema(root.schema, "catalog.schema");
    const bandIds = new Set(schema.bands.map((b) => b.id));
    const terrain = parseTerrain(root.terrain, "catalog.terrain");
    const ntr = root.network_terrain_revision;
    if (ntr !== undefined && ntr !== null && (typeof ntr !== "number" || !Number.isInteger(ntr) || ntr < 1)) {
        fail("catalog.network_terrain_revision: must be a positive integer or null");
    }
    return {
        schema_version: CATALOG_SCHEMA_VERSION,
        generated_at: instant(root, "generated_at", "catalog"),
        schema,
        skins: parseSkins(root.skins, "catalog.skins", schema),
        regions: parseRegions(root.regions, "catalog.regions", bandIds),
        cell_index: parseCellIndexRefs(root.cell_index, "catalog.cell_index", schema.bands),
        terrain,
        network_terrain_revision: (ntr as number | null | undefined) ?? null,
    };
}

/** The band with this id, or `undefined`. */
export function band(catalog: Catalog, id: string): BandEntry | undefined {
    return catalog.schema.bands.find((b) => b.id === id);
}

/** The one band whose bytes become the core file — the file with the ceiling
 *  (`OBCA_Spec.md` §5.7). Guaranteed to exist: the parser rejects a schema
 *  without exactly one. */
export function coreBand(catalog: Catalog): BandEntry {
    return catalog.schema.bands.find((b) => b.role === "core")!;
}

/** The region with this id, or `undefined`. */
export function region(catalog: Catalog, id: string): RegionEntry | undefined {
    return catalog.regions.find((r) => r.id === id);
}

/** The root's pin on a band's cell index. */
export function cellIndexRef(catalog: Catalog, bandId: string): CellIndexRef | undefined {
    return catalog.cell_index.find((r) => r.band === bandId);
}
