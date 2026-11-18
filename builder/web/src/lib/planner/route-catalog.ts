import { config, loadRouteCell, loadTrailCell, loadTrailRecord } from './map-data';
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

/** A file that loads once; a failed one loads again on the next call. */
function cached<K, V>(files: Map<K, Promise<V>>, key: K, load: (key: K) => Promise<V>): Promise<V> {
    let file = files.get(key);
    if (!file) {
        file = load(key);
        file.catch(() => files.delete(key));
        files.set(key, file);
    }
    return file;
}

/** The cell files, far files and trail record files of the route catalog. The cache has no limit: a session holds some tens of files. */
export class RouteCatalog {
    private readonly cells = new Map<string, Promise<CatalogRecord[] | null>>();
    private readonly farCells = new Map<string, Promise<TrailEntry[] | null>>();
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
     * The trails with a vertex of their far line in `box`, edges included, in ascending `id` order: each one once, with the
     * pieces of all far files of the box. Null, with no load, when the box meets more than `MAX_CELLS` zoom 7 cells or the
     * release has no trail files.
     */
    async trailsInBox(box: Box): Promise<TrailEntry[] | null> {
        if (!config.trails || trailsTooWide(box)) return null;
        const files = await Promise.all(boxCells(box, 7).map(id => cached(this.farCells, id, loadTrailCell)));
        const found = new Map<number, TrailEntry>();
        for (const entry of files.flatMap(file => file ?? [])) {
            const seen = found.get(entry.id);
            found.set(entry.id, seen ? { ...seen, far_udeg: [...seen.far_udeg, ...entry.far_udeg] } : entry);
        }
        return [...found.values()].filter(entry => entry.far_udeg.some(piece => inBox(piece, box))).sort((a, b) => a.id - b.id);
    }

    /** The whole record of the trail `id`, with `parts` and `trail_profile_m`, or null for a release with no trail files. */
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
