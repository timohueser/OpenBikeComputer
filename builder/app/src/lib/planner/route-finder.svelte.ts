import { clientConfig } from './client-config';
import type { Coordinate } from './editor';
import { coversCell, loadRouteCell, MAP_BOUNDS } from './map-data';
import { corridorTiles } from './place-index';
import type { BikeType } from './riding-profiles';
import { decodeCoordinates } from './route-answer';
import type { RoutingLine } from './routing';
import { joinedPlan, recordPlan, type RoutePlan } from './signed-route-plan';
import { nearestVertex, searchRoutes, type Bounds, type CatalogRecord, type RouteMatch, type RouteRecord, type RouteShape, type RouteSort } from './signed-routes';

export const RADII = [5, 10, 25, 50];

/** A start from a place has its name; a map click has the name of the nearest place once it is known. */
export interface RouteStart { coordinate: Coordinate; name?: string; near?: string }
export interface RouteFilters { radiusKm: number; shape: RouteShape; distanceKm: Bounds; climbM: Bounds; hardest: [number, number]; sort: RouteSort }
/** The shown route, its distance from the start when it is a match, and the stages of its long route once they load. */
export interface RouteDetail { route: CatalogRecord; distanceM?: number; family?: CatalogRecord; stages?: CatalogRecord[] }

const cellsAround = (at: Coordinate, km: number) => corridorTiles([at], km, 9).map(key => key.replaceAll('/', '-'));

/** Offline, the bounds are the union of the downloaded cells. */
function downloadedCells(): Set<string> | undefined {
    if (!clientConfig.bounds || !MAP_BOUNDS) return undefined;
    const [west, south, east, north] = MAP_BOUNDS;
    const center: Coordinate = [(west + east) / 2, (south + north) / 2];
    const km = Math.hypot((east - west) * 111.32 * Math.cos(center[1] * Math.PI / 180), (north - south) * 111.32) / 2;
    return new Set(cellsAround(center, km).filter(id => coversCell(MAP_BOUNDS!, id)));
}

/** The line of a record: its own, or the lines of the stages of a long route in order. */
export function recordLine(route: CatalogRecord, stages: CatalogRecord[] = []): Coordinate[] {
    return route.line_udeg ? decodeCoordinates(route.line_udeg) : stages.flatMap(stage => stage.line_udeg ? decodeCoordinates(stage.line_udeg) : []);
}

/** The Routes view state. It outlives the view, so "Back to routes" finds the filters and the list again. */
export class RouteFinder {
    start = $state<RouteStart | null>(null);
    filters = $state<RouteFilters>({ radiusKm: 25, shape: 'loop', distanceKm: {}, climbM: {}, hardest: [0, 2], sort: 'nearest' });
    matches = $state.raw<RouteMatch[]>([]);
    status = $state<'loading' | 'ready' | 'failed'>('ready');
    progress = $state({ loaded: 0, total: 0 });
    /** With no match: the next radius that has matches, and their count. */
    wider = $state.raw<{ radiusKm: number; count: number } | null>(null);
    /** The listed rows; the map draws their lines. */
    shown = $state(20);
    detail = $state.raw<RouteDetail | null>(null);
    hovered = $state<number | null>(null);
    loopStart = $state<'near' | 'data'>('near');
    /** The detail plan, routed once for its profile and time. */
    preview = $state.raw<RoutingLine | null>(null);
    private readonly covered = downloadedCells();
    private readonly cells = new Map<string, Promise<CatalogRecord[] | null>>();
    private readonly records = new Map<number, CatalogRecord>();
    private serial = 0;

    /** The plan of the detail: a loop from the chosen start, or a long route joined from its stages. Null when a long route is too long for one plan; undefined while its stages load. */
    readonly plan = $derived.by((): RoutePlan | null | undefined => {
        const detail = this.detail;
        if (!detail) return undefined;
        const { route, stages } = detail;
        if (route.stages) return stages?.length === route.stages.length ? joinedPlan(stages.map(stage => recordPlan(stage as RouteRecord))) : undefined;
        const start = route.loop && this.loopStart === 'near' && this.start ? nearestVertex(recordLine(route), this.start.coordinate) : 0;
        return recordPlan(route as RouteRecord, start);
    });

    /** A loaded record, such as the long route of a stage. */
    record(id: number): CatalogRecord | undefined {
        return this.records.get(id);
    }

    private cell(id: string) {
        let file = this.cells.get(id);
        if (!file) {
            file = loadRouteCell(id).then(records => {
                records?.forEach(record => this.records.set(record.id, record));
                return records;
            });
            // A failed cell loads again on the next search.
            file.catch(() => this.cells.delete(id));
            this.cells.set(id, file);
        }
        return file;
    }

    async search(activity: BikeType) {
        const start = this.start;
        if (!start) return;
        const id = ++this.serial;
        const query = { ...$state.snapshot(this.filters), start: start.coordinate, activity, covered: this.covered };
        this.status = 'loading';
        this.progress = { loaded: 0, total: 0 };
        const counted = (cell: string) => {
            if (id === this.serial) this.progress.total++;
            return this.cell(cell).finally(() => { if (id === this.serial) this.progress.loaded++; });
        };
        try {
            const matches = await searchRoutes(query, counted);
            let wider: RouteFinder['wider'] = null;
            if (!matches.length && query.radiusKm < RADII.at(-1)!) {
                const wide = await searchRoutes({ ...query, radiusKm: RADII.at(-1)! }, counted);
                const radiusKm = RADII.find(km => km > query.radiusKm && wide.some(match => match.distanceM <= km * 1000));
                if (radiusKm) wider = { radiusKm, count: wide.filter(match => match.distanceM <= radiusKm * 1000).length };
            }
            if (id !== this.serial) return;
            this.matches = matches;
            this.wider = wider;
            this.shown = 20;
            this.status = 'ready';
        } catch {
            if (id === this.serial) this.status = 'failed';
        }
    }

    /** Shows a record, and loads the stages of its long route. */
    async select(id: number | null) {
        this.loopStart = 'near';
        this.preview = null;
        const route = id === null ? undefined : this.records.get(id);
        if (!route) { this.detail = null; return; }
        const detail: RouteDetail = { route, distanceM: this.matches.find(match => match.route.id === id)?.distanceM };
        this.detail = detail;
        const family = route.stages ? route : route.parent ? this.records.get(route.parent) : undefined;
        if (!family?.stages) return;
        const stages = await this.stagesOf(family).catch(() => undefined);
        if (stages && this.detail === detail) this.detail = { ...detail, family, stages };
    }

    private async stagesOf(route: CatalogRecord): Promise<CatalogRecord[]> {
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
        return route.stages ? joinedPlan((await this.stagesOf(route)).map(stage => recordPlan(stage as RouteRecord))) : recordPlan(route as RouteRecord);
    }
}
