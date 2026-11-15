import type { Coordinate } from './geo';
import { loadRouteCell } from './map-data';
import { corridorTiles } from './place-index';
import type { BikeType } from './riding-profiles';
import { decodeCoordinates } from './route-answer';
import { joinedPlan, type RoutePlan } from './signed-route-plan';
import { kinds, routePlan, type CatalogRecord, type RouteRecord } from './signed-routes';

/** West, south, east and north, in degrees. */
export type Box = [west: number, south: number, east: number, north: number];

/** The most cell files that one map view loads (`specs/route-catalog.md`, "Map view"). */
export const MAX_CELLS = 12;

const cellsAround = (at: Coordinate, km: number) => corridorTiles([at], km, 9).map(key => key.replaceAll('/', '-'));

/** The line of a record: its own, or the lines of the stages of a long route in order. */
export function recordLine(route: CatalogRecord, stages: CatalogRecord[] = []): Coordinate[] {
    return route.line_udeg ? decodeCoordinates(route.line_udeg) : stages.flatMap(stage => stage.line_udeg ? decodeCoordinates(stage.line_udeg) : []);
}

/** The ids `9-X-Y` of the zoom 9 cells whose box meets `box`, edges included. */
export function boxCells([west, south, east, north]: Box): string[] {
    if (!(west <= east && south <= north)) return [];
    const n = 512;
    // Multiplication before division keeps a longitude on a cell edge exact.
    const column = (lon: number) => (lon + 180) * n / 360;
    const row = (lat: number) => {
        const rad = lat * Math.PI / 180;
        return (1 - Math.log(Math.tan(rad) + 1 / Math.cos(rad)) / Math.PI) / 2 * n;
    };
    // A cell [i, i + 1] meets the range [a, b] when i is from ceil(a) - 1 to floor(b).
    const range = (a: number, b: number) => {
        const from = Math.max(0, Math.ceil(a) - 1), to = Math.min(n - 1, Math.floor(b));
        return Array.from({ length: Math.max(0, to - from + 1) }, (_, i) => from + i);
    };
    const rows = range(row(north), row(south));
    return range(column(west), column(east)).flatMap(x => rows.map(y => `9-${x}-${y}`));
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
        const cells = boxCells(box);
        if (cells.length > MAX_CELLS) return null;
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
