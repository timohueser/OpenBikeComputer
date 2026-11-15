import type { Coordinate } from './geo';
import { loadRouteCell } from './map-data';
import { corridorTiles, tileColumn, tileRow } from './place-index';
import type { BikeType } from './riding-profiles';
import { decodeCoordinates } from './route-answer';
import { joinedPlan, type RoutePlan } from './signed-route-plan';
import { kinds, routePlan, type CatalogRecord, type RouteRecord } from './signed-routes';

/**
 * West, south, east and north, in degrees. The caller wraps longitudes to -180..180. A box with west above east,
 * south above north or a longitude outside -180..180 gives no cell.
 */
export type Box = [west: number, south: number, east: number, north: number];

/** The most cell files that one map view loads (`specs/route-catalog.md`, "Map view"). */
export const MAX_CELLS = 12;

const cellsAround = (at: Coordinate, km: number) => corridorTiles([at], km, 9).map(key => key.replaceAll('/', '-'));

/** The line of a record: its own, or the lines of the stages of a long route in order. */
export function recordLine(route: CatalogRecord, stages: CatalogRecord[] = []): Coordinate[] {
    return route.line_udeg ? decodeCoordinates(route.line_udeg) : stages.flatMap(stage => stage.line_udeg ? decodeCoordinates(stage.line_udeg) : []);
}

/** The columns and rows of the zoom 9 cells whose box meets `box`, edges included, or null for a box with no cell. */
function boxRange([west, south, east, north]: Box) {
    if (!(-180 <= west && west <= east && east <= 180 && south <= north)) return null;
    // A cell [i, i + 1] meets the range [a, b] when i is from ceil(a) - 1 to floor(b). A latitude on a row edge
    // is not exact in floating point; no vertex in microdegrees lies on such an edge, so this has no effect.
    const range = (a: number, b: number) => ({ from: Math.max(0, Math.ceil(a) - 1), to: Math.min(511, Math.floor(b)) });
    const columns = range(tileColumn(west, 9), tileColumn(east, 9)), rows = range(tileRow(north, 9), tileRow(south, 9));
    return { columns, rows, count: Math.max(0, columns.to - columns.from + 1) * Math.max(0, rows.to - rows.from + 1) };
}

/** The ids `9-X-Y` of the zoom 9 cells whose box meets `box`, edges included. */
export function boxCells(box: Box): string[] {
    const cells: string[] = [];
    const range = boxRange(box);
    if (!range) return cells;
    const { columns, rows } = range;
    for (let x = columns.from; x <= columns.to; x++) for (let y = rows.from; y <= rows.to; y++) cells.push(`9-${x}-${y}`);
    return cells;
}

/** The cell files of the route catalog and their records. The cache has no limit: a session holds some tens of cells. */
export class RouteCatalog {
    private readonly cells = new Map<string, Promise<CatalogRecord[] | null>>();
    private readonly records = new Map<number, CatalogRecord>();

    /** A loaded record, such as the long route of a stage. */
    record(id: number): CatalogRecord | undefined {
        return this.records.get(id);
    }

    /** The records of a cell, or null for a cell outside the release. A failed cell loads again on the next call. */
    cell(id: string): Promise<CatalogRecord[] | null> {
        let file = this.cells.get(id);
        if (!file) {
            file = loadRouteCell(id).then(records => {
                records?.forEach(record => this.records.set(record.id, record));
                return records;
            });
            file.catch(() => this.cells.delete(id));
            this.cells.set(id, file);
        }
        return file;
    }

    /**
     * The route records for `bike` with a vertex of their line in `box`, edges included, in ascending `id` order.
     * A long route is not one, but its stages are. Null, with no load, when the box meets more than `MAX_CELLS` cells.
     */
    async inBox(box: Box, bike: BikeType): Promise<CatalogRecord[] | null> {
        if ((boxRange(box)?.count ?? 0) > MAX_CELLS) return null;
        const cells = boxCells(box);
        const [west, south, east, north] = box;
        const found = new Map<number, CatalogRecord>();
        for (const file of await Promise.all(cells.map(id => this.cell(id)))) {
            for (const route of file ?? []) {
                if (!route.line_udeg || !kinds[bike].includes(route.kind)) continue;
                if (decodeCoordinates(route.line_udeg).some(([lon, lat]) => west <= lon && lon <= east && south <= lat && lat <= north)) found.set(route.id, route);
            }
        }
        return [...found.values()].sort((a, b) => a.id - b.id);
    }

    /** The stages of a long route, in order. */
    async stagesOf(route: CatalogRecord): Promise<CatalogRecord[]> {
        await Promise.all(route.cells.map(cell => this.cell(cell)));
        const stages = route.stages!.map(stage => this.records.get(stage));
        if (stages.some(stage => !stage)) throw new Error('A stage of this route is missing from the catalog.');
        return stages as CatalogRecord[];
    }

    /** A record of the catalog that a map point lies on. */
    async find(id: number, at: Coordinate): Promise<CatalogRecord | undefined> {
        await Promise.all(cellsAround(at, .1).map(cell => this.cell(cell)));
        return this.records.get(id);
    }

    /** The plan of a record in data order, or null when a long route is too long for one plan. */
    async dataPlan(route: CatalogRecord): Promise<RoutePlan | null> {
        return route.stages ? joinedPlan((await this.stagesOf(route)).map(stage => routePlan(stage as RouteRecord))) : routePlan(route as RouteRecord);
    }
}

/** The catalog that the pages of the app share. */
export const catalog = new RouteCatalog();
