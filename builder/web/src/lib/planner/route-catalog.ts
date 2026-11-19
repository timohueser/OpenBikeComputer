import { config, coversCell, loadRouteCell, loadTrailCell, loadTrailRecord } from './map-data';
import { tileColumn, tileRow } from './place-index';
import type { BikeType } from './riding-profiles';
import { kinds, type CatalogRecord, type TrailEntry } from './signed-routes';

/**
 * West, south, east and north, in degrees. The caller wraps longitudes to -180..180. A box with west above east,
 * south above north or a longitude outside -180..180 gives no cell.
 */
export type Box = [west: number, south: number, east: number, north: number];

/** The most cell files, and the most far files, that one map view loads (`specs/route-catalog.md`, "Map view"). */
export const MAX_CELLS = 12;

/** The columns and rows of the cells of `zoom` whose box meets `box`, edges included, or null for a box with no cell. */
function boxRange([west, south, east, north]: Box, zoom: number) {
    if (!(-180 <= west && west <= east && east <= 180 && south <= north)) return null;
    // A cell [i, i + 1] meets the range [a, b] when i is from ceil(a) - 1 to floor(b). A latitude on a row edge
    // is not exact in floating point; no vertex in microdegrees lies on such an edge, so this has no effect.
    const range = (a: number, b: number) => ({ from: Math.max(0, Math.ceil(a) - 1), to: Math.min(2 ** zoom - 1, Math.floor(b)) });
    const columns = range(tileColumn(west, zoom), tileColumn(east, zoom)), rows = range(tileRow(north, zoom), tileRow(south, zoom));
    return { columns, rows, count: Math.max(0, columns.to - columns.from + 1) * Math.max(0, rows.to - rows.from + 1) };
}

/** True when `box` meets more than `MAX_CELLS` cells, so that `inBox` loads nothing for it. */
export const tooWide = (box: Box) => (boxRange(box, 9)?.count ?? 0) > MAX_CELLS;

/** True when `box` meets more than `MAX_CELLS` zoom 7 cells, so that `trailsInBox` loads nothing for it. */
export const trailsTooWide = (box: Box) => (boxRange(box, 7)?.count ?? 0) > MAX_CELLS;

/** The ids `Z-X-Y` of the cells of `zoom`, 9 by default, whose box meets `box`, edges included. */
export function boxCells(box: Box, zoom = 9): string[] {
    const cells: string[] = [];
    const range = boxRange(box, zoom);
    if (!range) return cells;
    const { columns, rows } = range;
    for (let x = columns.from; x <= columns.to; x++) for (let y = rows.from; y <= rows.to; y++) cells.push(`${zoom}-${x}-${y}`);
    return cells;
}

/** True when a vertex of `udeg`, the deltas of a line in microdegrees, is in the box, edges included. */
function inBox(udeg: number[], [west, south, east, north]: Box): boolean {
    for (let i = 0, lon = 0, lat = 0; i < udeg.length; i += 2) {
        lon += udeg[i];
        lat += udeg[i + 1];
        const x = lon / 1e6, y = lat / 1e6;
        if (west <= x && x <= east && south <= y && y <= north) return true;
    }
    return false;
}

/** True when a segment of `udeg`, the deltas of a line in microdegrees, meets the box, edges included. */
function segmentInBox(udeg: number[], [west, south, east, north]: Box): boolean {
    // A far view tests some 100,000s of segments: a piece whose bounds miss the box needs no clip test.
    let lon = 0, lat = 0, minLon = Infinity, minLat = Infinity, maxLon = -Infinity, maxLat = -Infinity;
    for (let i = 0; i < udeg.length; i += 2) {
        lon += udeg[i];
        lat += udeg[i + 1];
        minLon = Math.min(minLon, lon);
        maxLon = Math.max(maxLon, lon);
        minLat = Math.min(minLat, lat);
        maxLat = Math.max(maxLat, lat);
    }
    if (minLon / 1e6 > east || maxLon / 1e6 < west || minLat / 1e6 > north || maxLat / 1e6 < south) return false;
    for (let i = 2, lon = udeg[0], lat = udeg[1]; i < udeg.length; i += 2) {
        const x = lon / 1e6, y = lat / 1e6, dx = udeg[i] / 1e6, dy = udeg[i + 1] / 1e6;
        lon += udeg[i];
        lat += udeg[i + 1];
        // Liang-Barsky: the share of the segment from t0 to t1 is inside each edge.
        let t0 = 0, t1 = 1;
        for (const [p, q] of [[-dx, x - west], [dx, east - x], [-dy, y - south], [dy, north - y]]) {
            if (p === 0) t0 = q < 0 ? 2 : t0;
            else if (p < 0) t0 = Math.max(t0, q / p);
            else t1 = Math.min(t1, q / p);
        }
        if (t0 <= t1) return true;
    }
    return false;
}

/** The most far files that the catalog keeps: those of the last two views. A far file holds up to 400 kB. */
const FAR_FILES = 2 * MAX_CELLS;

/** A file that loads once; a failed one loads again on the next call. Above `limit` files, the least recently asked goes. */
function cached<K, V>(files: Map<K, Promise<V>>, key: K, load: (key: K) => Promise<V>, limit = Infinity): Promise<V> {
    let file = files.get(key);
    if (file) files.delete(key);
    else {
        const loading = file = load(key);
        loading.catch(() => files.get(key) === loading && files.delete(key));
    }
    files.set(key, file);
    while (files.size > limit) files.delete(files.keys().next().value!);
    return file;
}

/**
 * The cell files, far files and trail record files of the route catalog. Cell files and trail record files have no limit:
 * a session holds some tens of them; far files are larger and keep `FAR_FILES`.
 */
export class RouteCatalog {
    private readonly cells = new Map<string, Promise<CatalogRecord[] | null>>();
    private readonly farCells = new Map<string, Promise<TrailEntry[] | null>>();
    /** The far files of `farCells` that have loaded. */
    private readonly farLoaded = new Map<string, TrailEntry[] | null>();
    private readonly joined = new WeakMap<TrailEntry, { from: TrailEntry[]; entry: TrailEntry }>();
    private readonly trails = new Map<number, Promise<CatalogRecord | null>>();
    private readonly loaded = new Map<string, CatalogRecord[] | null>();
    private readonly records = new Map<number, CatalogRecord>();

    /** A loaded record, such as the long route of a stage. */
    record(id: number): CatalogRecord | undefined {
        return this.records.get(id);
    }

    /** The records of a cell, or null for a cell outside the release. A failed cell loads again on the next call. */
    cell(id: string): Promise<CatalogRecord[] | null> {
        return cached(this.cells, id, id => loadRouteCell(id).then(records => {
            records?.forEach(record => this.records.set(record.id, record));
            this.loaded.set(id, records);
            return records;
        }));
    }

    /**
     * The route records for `bike` with a vertex of their line in `box`, edges included, in ascending `id` order.
     * A long route is not one, but its stages are. Null, with no load, when the box meets more than `MAX_CELLS` cells.
     */
    async inBox(box: Box, bike: BikeType): Promise<CatalogRecord[] | null> {
        if (tooWide(box)) return null;
        await Promise.all(boxCells(box).map(id => this.cell(id)));
        return this.inBoxNow(box, bike)!;
    }

    /** The answer of `inBox` with no wait, or undefined while a cell of the box is not loaded. */
    inBoxNow(box: Box, bike: BikeType): CatalogRecord[] | null | undefined {
        if (tooWide(box)) return null;
        const cells = boxCells(box);
        if (!cells.every(id => this.loaded.has(id))) return undefined;
        const found = new Map<number, CatalogRecord>();
        for (const id of cells) {
            for (const route of this.loaded.get(id) ?? []) {
                if (route.line_udeg && kinds[bike].includes(route.kind) && inBox(route.line_udeg, box)) found.set(route.id, route);
            }
        }
        return [...found.values()].sort((a, b) => a.id - b.id);
    }

    /**
     * The trails with a segment of their far line in `box`, edges included, in ascending `id` order: each one once, with the
     * pieces of all far files of the box. Null, with no load, when the box meets more than `MAX_CELLS` zoom 7 cells or the
     * release has no trail files.
     */
    async trailsInBox(box: Box): Promise<TrailEntry[] | null> {
        if (!config.trails || trailsTooWide(box)) return null;
        const files = boxCells(box, 7).map(id => cached(this.farCells, id, id => loadTrailCell(id).then(entries => {
            if (this.farCells.has(id)) this.farLoaded.set(id, entries);
            return entries;
        }), FAR_FILES));
        // The files that the cache dropped go, also when a load of this box fails.
        for (const id of this.farLoaded.keys()) if (!this.farCells.has(id)) this.farLoaded.delete(id);
        return this.trailsOf(await Promise.all(files), box);
    }

    /** The answer of `trailsInBox` with no wait, or undefined while a far file of the box is not loaded. */
    trailsInBoxNow(box: Box): TrailEntry[] | null | undefined {
        if (!config.trails || trailsTooWide(box)) return null;
        // A cell outside the release has no far file to wait for.
        const files = this.farFilesNow(box);
        return files && this.trailsOf(files, box);
    }

    /**
     * Every trail of the loaded far files of the zoom 7 cells of `box`, by id, with no test against the box: the far files
     * of a near view name each trail that a record of the view is in. Undefined while a far file of the box is not
     * loaded; empty when the release has no trail files.
     */
    trailsOfCellsNow(box: Box): Map<number, TrailEntry> | undefined {
        if (!config.trails) return new Map();
        const files = this.farFilesNow(box);
        return files && new Map(this.joinedOf(files).map(entry => [entry.id, entry]));
    }

    /** The far files of the box, or undefined while one is not loaded. A cell outside the release has no far file to wait for. */
    private farFilesNow(box: Box): (TrailEntry[] | null)[] | undefined {
        const files = boxCells(box, 7).map(id => coversCell(config.bounds, id) ? this.farLoaded.get(id) : null);
        return files.every(file => file !== undefined) ? files as (TrailEntry[] | null)[] : undefined;
    }

    /** The trails of `files` in `box`. */
    private trailsOf(files: (TrailEntry[] | null)[], box: Box): TrailEntry[] {
        return this.joinedOf(files).filter(entry => entry.far_udeg.some(piece => segmentInBox(piece, box))).sort((a, b) => a.id - b.id);
    }

    /** Each trail of `files` once. A trail in several files keeps one joined object while those files stay loaded, so a new
     * answer keeps its row. */
    private joinedOf(files: (TrailEntry[] | null)[]): TrailEntry[] {
        const found = new Map<number, TrailEntry[]>();
        for (const entry of files.flatMap(file => file ?? [])) found.set(entry.id, [...found.get(entry.id) ?? [], entry]);
        return [...found.values()].map(from => {
            if (from.length === 1) return from[0];
            const kept = this.joined.get(from[0]);
            if (kept?.from.length === from.length && kept.from.every((entry, i) => entry === from[i])) return kept.entry;
            const entry = { ...from[0], far_udeg: from.flatMap(entry => entry.far_udeg) };
            this.joined.set(from[0], { from, entry });
            return entry;
        });
    }

    /** The whole record of the trail `id`, with `parts` and `trail_profile_m`, or null for a grid release with no trail files
     * and for an id that the release has no trail record file of. */
    trail(id: number): Promise<CatalogRecord | null> {
        return cached(this.trails, id, loadTrailRecord);
    }

    /** The stages of a long route, in order. */
    async stagesOf(route: CatalogRecord): Promise<CatalogRecord[]> {
        await Promise.all(route.cells.map(cell => this.cell(cell)));
        const stages = route.stages!.map(stage => this.records.get(stage));
        if (stages.some(stage => !stage)) throw new Error('A stage of this route is missing from the catalog.');
        return stages as CatalogRecord[];
    }

}

/** The catalog that the pages of the app share. */
export const catalog = new RouteCatalog();
