// The two satellite documents: a band's cell index (§8) and a region's cell
// list (§6).
//
// DACH is thousands of cells across four bands, so the catalog splits its data and keeps
// the all-or-nothing guarantee **per document**, with the root pinning each
// satellite by `bytes` + `sha256`. That pin is checked by `client.ts` before a
// byte of this parser's input is trusted; what is left here is the internal
// consistency of a document that has already proved it is the one the root meant:
// the revision it was baked at, the band it claims, and the ids inside it.
//
// The revision check is the one worth naming. Assembly copies chunk bytes
// between files, which is only meaningful within one schema revision
// (`OBCA_Spec.md` §6.3) — so a satellite a revision behind the root is not
// "slightly stale", it is a set of cells that must never be assembled together.
// A generator refuses to publish a mixed tree; this refuses to consume one.

import { formatCellId, parseCellId, type CellId } from "./grid";
import type { Catalog, CellIndexRef, RegionEntry } from "./manifest";
import {
    arr,
    bool,
    DATE,
    fail,
    instant,
    int,
    json,
    KEBAB,
    obj,
    PATH_ID,
    pinnedUrlStr,
    realDate,
    SHA256,
    str,
} from "./parse";

/** One source extract behind a cell. */
export interface CellSource {
    extract_id: string;
    snapshot: string;
}

/**
 * One published cell.
 *
 * There is no bbox and that is deliberate: a cell's coverage is exactly its grid
 * square, which the `id` determines to the microdegree, and the bakery verifies
 * the artifact's own OBCM header against it. `cell` below is that square's id,
 * parsed once here so nothing downstream re-parses a string per frame.
 */
export interface CellEntry {
    id: string;
    /** The parsed id. Not on the wire — the wire has the string. */
    cell: CellId;
    bytes: number;
    sha256: string;
    url: string;
    built_at: string;
    sources: CellSource[];
    /** `true` iff the sources do not fully cover the cell's square (§3.7). A
     *  consumer MUST NOT present a partial cell as canonical coverage. */
    partial: boolean;
}

/** An inclusive, same-row run of cells whose canonical content is empty.
 *
 * These cells have no OBCM object: they are explicit coverage, not missing
 * data, and therefore cost zero bytes and need no payload download or graft.
 * Their identities still participate in assembly coverage and output bounds.
 */
export interface KnownEmptyRun {
    start: string;
    end: string;
    /** Parsed bounds. Not on the wire. */
    startCell: CellId;
    endCell: CellId;
    built_at: string;
    sources: CellSource[];
}

export interface CellIndexDocument {
    schema_version: number;
    schema_revision: number;
    band: string;
    cells: CellEntry[];
    known_empty: KnownEmptyRun[];
    /** The cells keyed by canonical id — the lookup every price and every
     *  download plan does, built once. */
    byId: ReadonlyMap<string, CellEntry>;
    /** Known-empty runs keyed by latitude row, for logarithmic membership
     *  checks without expanding a planet-scale range into individual ids. */
    emptyByRow: ReadonlyMap<number, readonly KnownEmptyRun[]>;
}

export interface RegionCellsDocument {
    schema_version: number;
    schema_revision: number;
    region_id: string;
    /** Band id → its cell ids, sorted, exactly as published. */
    cells: Record<string, string[]>;
}

/**
 * `parseCellId`, with the grid's refusal re-thrown as a document error.
 *
 * A `GridError` is a statement about a *string*; from inside a parser the
 * caller's contract is `CatalogFormatError`, a statement about a *document*
 * (§7). The distinction is not pedantry: a consumer catches the document error
 * to say "this catalog is malformed" and let the user retry or report it, and
 * a `GridError` escaping through the same call would sail past that handler and
 * land as a blank screen — for a case that is precisely a bad document.
 */
function parseCellIdIn(id: string, at: string): CellId {
    try {
        return parseCellId(id);
    } catch (e) {
        return fail(`${at}: ${e instanceof Error ? e.message : String(e)}`);
    }
}

function checkEnvelope(o: Record<string, unknown>, catalog: Catalog, where: string): void {
    if (o.schema_version !== catalog.schema_version) {
        fail(`${where}: schema_version ${JSON.stringify(o.schema_version)} is not ${catalog.schema_version}`);
    }
    const revision = int(o, "schema_revision", where, 1);
    if (revision !== catalog.schema.revision) {
        fail(
            `${where}: schema_revision ${revision} is not the catalog's ${catalog.schema.revision} — ` +
                "cells of two revisions must never be assembled together",
        );
    }
}

function parseSources(v: unknown, at: string): CellSource[] {
    const sources = arr(v, `${at}.sources`).map((s, n) => {
        const sat = `${at}.sources[${n}]`;
        const e = obj(s, sat);
        const snapshot = str(e, "snapshot", sat, DATE);
        realDate(snapshot, `${sat}.snapshot`);
        return { extract_id: str(e, "extract_id", sat, PATH_ID), snapshot };
    });
    if (!sources.length) fail(`${at}: coverage came from at least one source extract`);
    for (let n = 1; n < sources.length; n++) {
        if (sources[n].extract_id <= sources[n - 1].extract_id) {
            fail(`${at}.sources: not sorted by extract_id, or an extract appears twice`);
        }
    }
    return sources;
}

function sameProvenance(a: KnownEmptyRun, b: KnownEmptyRun): boolean {
    return a.built_at === b.built_at && JSON.stringify(a.sources) === JSON.stringify(b.sources);
}

/** The known-empty run containing `id`, if any. */
export function knownEmptyAt(index: CellIndexDocument, id: string): KnownEmptyRun | undefined {
    const cell = parseCellIdIn(id, `cell index (${index.band}) lookup`);
    const runs = index.emptyByRow.get(cell.i) ?? [];
    let lo = 0;
    let hi = runs.length;
    while (lo < hi) {
        const mid = (lo + hi) >>> 1;
        const run = runs[mid];
        if (cell.j < run.startCell.j) hi = mid;
        else if (cell.j > run.endCell.j) lo = mid + 1;
        else return cell.log2 === run.startCell.log2 ? run : undefined;
    }
    return undefined;
}

/** Whether a band explicitly covers a cell, either with an artifact or as
 * canonical empty ground. */
export function cellIndexHas(index: CellIndexDocument, id: string): boolean {
    return index.byId.has(id) || knownEmptyAt(index, id) !== undefined;
}

/**
 * Parse one band's cell index against the root's pin on it.
 *
 * `ref` is what the root says this document is: which band, which cell size, how
 * many cells. Every one of those is re-asserted here, because the digest only
 * proves the bytes are the ones the root hashed — not that the root described
 * them correctly.
 */
export function parseCellIndex(body: string, catalog: Catalog, ref: CellIndexRef): CellIndexDocument {
    const where = `cell index (${ref.band})`;
    const doc = obj(json(body, where), where);
    checkEnvelope(doc, catalog, where);

    const bandId = str(doc, "band", where, KEBAB);
    if (bandId !== ref.band) fail(`${where}: document is band ${JSON.stringify(bandId)}, root pinned "${ref.band}"`);

    const raw = arr(doc.cells, `${where}.cells`);
    if (raw.length !== ref.cell_count) {
        fail(`${where}: root says ${ref.cell_count} cells, the document has ${raw.length}`);
    }

    const byId = new Map<string, CellEntry>();
    let previous: CellId | null = null;
    const cells = raw.map((entry, k) => {
        const at = `${where}.cells[${k}]`;
        const o = obj(entry, at);
        const id = str(o, "id", at);
        const cell = parseCellIdIn(id, at);
        if (formatCellId(cell) !== id) fail(`${at}: id ${JSON.stringify(id)} is not canonically padded`);
        if (cell.log2 !== ref.cell_log2) {
            fail(`${at}: cell size 2^${cell.log2} is not band "${ref.band}"'s 2^${ref.cell_log2}`);
        }
        // Sorted by (i, j), so a consumer may binary-search it and a diff of two
        // publishes is a diff of the cells rather than of their order.
        if (previous && (cell.i < previous.i || (cell.i === previous.i && cell.j <= previous.j))) {
            fail(`${at}: cells are not sorted by (i, j)`);
        }
        previous = cell;

        const sha256 = str(o, "sha256", at, SHA256);
        const cellEntry: CellEntry = {
            id,
            cell,
            bytes: int(o, "bytes", at, 0),
            sha256,
            url: pinnedUrlStr(o, "url", sha256, at),
            built_at: instant(o, "built_at", at),
            sources: parseSources(o.sources, at),
            partial: bool(o, "partial", at),
        };
        byId.set(id, cellEntry);
        return cellEntry;
    });

    const rawEmpty = arr(doc.known_empty, `${where}.known_empty`);
    const emptyByRow = new Map<number, KnownEmptyRun[]>();
    let previousEmpty: KnownEmptyRun | null = null;
    let knownEmptyCount = 0;
    const knownEmpty = rawEmpty.map((entry, k) => {
        const at = `${where}.known_empty[${k}]`;
        const o = obj(entry, at);
        const start = str(o, "start", at);
        const end = str(o, "end", at);
        const startCell = parseCellIdIn(start, `${at}.start`);
        const endCell = parseCellIdIn(end, `${at}.end`);
        if (formatCellId(startCell) !== start) fail(`${at}.start: id ${JSON.stringify(start)} is not canonically padded`);
        if (formatCellId(endCell) !== end) fail(`${at}.end: id ${JSON.stringify(end)} is not canonically padded`);
        if (startCell.log2 !== ref.cell_log2 || endCell.log2 !== ref.cell_log2) {
            fail(`${at}: range cell size is not band "${ref.band}"'s 2^${ref.cell_log2}`);
        }
        if (startCell.i !== endCell.i) fail(`${at}: a known-empty range must stay in one latitude row`);
        if (startCell.j > endCell.j) fail(`${at}: range start comes after its end`);

        const run: KnownEmptyRun = {
            start,
            end,
            startCell,
            endCell,
            built_at: instant(o, "built_at", at),
            sources: parseSources(o.sources, at),
        };
        if (
            previousEmpty &&
            (startCell.i < previousEmpty.startCell.i ||
                (startCell.i === previousEmpty.startCell.i && startCell.j <= previousEmpty.endCell.j))
        ) {
            fail(`${at}: known-empty ranges are not sorted or overlap`);
        }
        if (
            previousEmpty &&
            startCell.i === previousEmpty.endCell.i &&
            startCell.j === previousEmpty.endCell.j + 1 &&
            sameProvenance(previousEmpty, run)
        ) {
            fail(`${at}: adjacent ranges with identical provenance must be merged`);
        }
        previousEmpty = run;
        knownEmptyCount += endCell.j - startCell.j + 1;
        const row = emptyByRow.get(startCell.i) ?? [];
        row.push(run);
        emptyByRow.set(startCell.i, row);
        return run;
    });
    if (knownEmptyCount !== ref.known_empty_count) {
        fail(`${where}: root says ${ref.known_empty_count} known-empty cells, the document covers ${knownEmptyCount}`);
    }

    const index: CellIndexDocument = {
        schema_version: catalog.schema_version,
        schema_revision: catalog.schema.revision,
        band: bandId,
        cells,
        known_empty: knownEmpty,
        byId,
        emptyByRow,
    };
    for (const cell of cells) {
        if (knownEmptyAt(index, cell.id)) {
            fail(`${where}: cell ${cell.id} has both an artifact and a known-empty assertion`);
        }
    }
    return index;
}

/**
 * Parse a region's cell list against the root's entry for that region.
 *
 * The list is **stored, not derived from the boundary** (§6): deriving one
 * would let a simplification error drop an edge cell, and a dropped fine cell is
 * a silent hole in street detail. So this parser's job is to check the stored
 * answer is the one the root priced — same bands, same counts — and never to
 * recompute it.
 */
export function parseRegionCells(body: string, catalog: Catalog, entry: RegionEntry): RegionCellsDocument {
    const where = `region cells (${entry.id})`;
    const doc = obj(json(body, where), where);
    checkEnvelope(doc, catalog, where);

    const regionId = str(doc, "region_id", where, PATH_ID);
    if (regionId !== entry.id) {
        fail(`${where}: document is region ${JSON.stringify(regionId)}, root pinned "${entry.id}"`);
    }

    const bandsById = new Map(catalog.schema.bands.map((b) => [b.id, b]));
    const raw = obj(doc.cells, `${where}.cells`);
    // Null-prototype: band ids come from a document and `"constructor"` is a
    // legal kebab id, so a plain literal would answer `cells["constructor"]`
    // with a function for a band this list never named.
    const cells: Record<string, string[]> = Object.create(null);
    for (const bandId of Object.keys(raw)) {
        const at = `${where}.cells.${bandId}`;
        const band = bandsById.get(bandId);
        // §6: a list naming a band the schema lacks is a list this client
        // cannot place in any file of the assembly.
        if (!band) fail(`${at}: band ${JSON.stringify(bandId)} is not in schema.bands`);
        const ids = arr(raw[bandId], at).map((v, k) => {
            if (typeof v !== "string") fail(`${at}[${k}]: expected a cell id`);
            const cell = parseCellIdIn(v as string, `${at}[${k}]`);
            if (formatCellId(cell) !== v) fail(`${at}[${k}]: id ${JSON.stringify(v)} is not canonically padded`);
            if (cell.log2 !== band.cell_log2) {
                fail(`${at}[${k}]: cell size 2^${cell.log2} is not band "${bandId}"'s 2^${band.cell_log2}`);
            }
            return v as string;
        });
        for (let k = 1; k < ids.length; k++) {
            if (ids[k] <= ids[k - 1]) fail(`${at}: ids are not sorted, or one appears twice`);
        }
        // The root priced this region from these counts; if they disagree the
        // price shown before the download is not the price of the download.
        const priced = entry.cell_count[bandId] ?? 0;
        if (ids.length !== priced) {
            fail(`${at}: the root prices ${priced} cell(s) in this band, the list has ${ids.length}`);
        }
        cells[bandId] = ids;
    }
    for (const [bandId, count] of Object.entries(entry.cell_count)) {
        if (count > 0 && !(bandId in cells)) fail(`${where}: band "${bandId}" is priced but absent from the list`);
    }
    return { schema_version: catalog.schema_version, schema_revision: catalog.schema.revision, region_id: regionId, cells };
}

/**
 * §6's cross-document MUST: a region cell list may not name a cell that is
 * absent from its band's index.
 *
 * It lives apart from `parseRegionCells` because it is the only check that needs
 * two satellites at once, and the client applies it exactly when both are in
 * hand. A violation is not a hole to draw — a hole is ground with no cell, while
 * this is a *named* cell with no bytes, size or digest, which is a broken
 * publish.
 */
export function assertRegionCellsIndexed(
    doc: RegionCellsDocument,
    indexByBand: ReadonlyMap<string, CellIndexDocument>,
): void {
    for (const [bandId, ids] of Object.entries(doc.cells)) {
        const index = indexByBand.get(bandId);
        if (!index) continue; // not loaded yet; the client checks what it has
        for (const id of ids) {
            if (!cellIndexHas(index, id)) {
                fail(`region cells (${doc.region_id}): cell ${id} is not in band "${bandId}"'s index`);
            }
        }
    }
}
