import type { Coordinate } from './geo';
import { loadRouteCell } from './map-data';
import { corridorTiles } from './place-index';
import type { BikeType } from './riding-profiles';
import { decodeCoordinates } from './route-answer';
import type { RoutingLine } from './routing';
import { joinedPlan, type RoutePlan } from './signed-route-plan';
import type { SignedRoutesView } from './signed-routes-map';
import { routePlan, searchRoutes, type Bounds, type CatalogRecord, type RouteMatch, type RouteRecord, type RouteShape, type RouteSort } from './signed-routes';

export const RADII = [5, 10, 25, 50];
/** The filters that can remove every match near the start. */
export type RouteFilter = 'distanceKm' | 'climbM' | 'hardest';
const open = { distanceKm: {}, climbM: {}, hardest: [0, 3] } as const;

/** A start from a place has its name; a map click has the name of the nearest place once it is known. */
export interface RouteStart { coordinate: Coordinate; name?: string; near?: string }
export interface RouteFilters { radiusKm: number; shape: RouteShape; distanceKm: Bounds; climbM: Bounds; hardest: [number, number]; sort: RouteSort }
/** The shown route, its distance from the start when it is a match, and the stages of its long route once they load or fail to. */
export interface RouteDetail { route: CatalogRecord; distanceM?: number; family?: CatalogRecord; stages?: CatalogRecord[]; failed?: boolean }

const cellsAround = (at: Coordinate, km: number) => corridorTiles([at], km, 9).map(key => key.replaceAll('/', '-'));

/** The line of a record: its own, or the lines of the stages of a long route in order. */
export function recordLine(route: CatalogRecord, stages: CatalogRecord[] = []): Coordinate[] {
    return route.line_udeg ? decodeCoordinates(route.line_udeg) : stages.flatMap(stage => stage.line_udeg ? decodeCoordinates(stage.line_udeg) : []);
}

/** The Routes view state. It outlives the view, so "Back to routes" finds the filters and the list again. */
export class RouteFinder {
    start = $state<RouteStart | null>(null);
    filters = $state<RouteFilters>({ radiusKm: 10, shape: 'loop', distanceKm: {}, climbM: {}, hardest: [0, 2], sort: 'nearest' });
    matches = $state.raw<RouteMatch[]>([]);
    status = $state<'loading' | 'ready' | 'failed'>('ready');
    progress = $state({ loaded: 0, total: 0 });
    /** With no match: the next radius that has matches, and their count. */
    wider = $state.raw<{ radiusKm: number; count: number } | null>(null);
    /** With no match: the filter without which some routes within the radius match. */
    blocker = $state<RouteFilter | null>(null);
    /** The listed rows; the map draws their lines. */
    shown = $state(20);
    detail = $state.raw<RouteDetail | null>(null);
    hovered = $state<number | null>(null);
    /** A routed plan of the detail, for its profile, figures and time. */
    routed = $state.raw<{ plan: RoutePlan; line: RoutingLine } | null>(null);
    private readonly cells = new Map<string, Promise<CatalogRecord[] | null>>();
    private readonly records = new Map<number, CatalogRecord>();
    private serial = 0;

    /** The plan of the detail, from the route's own start; a long route joins its stages. Null when a long route is too long for one plan; undefined while its stages load. */
    readonly plan = $derived.by((): RoutePlan | null | undefined => {
        const detail = this.detail;
        if (!detail) return undefined;
        const { route, stages } = detail;
        if (route.stages) return stages?.length === route.stages.length ? joinedPlan(stages.map(stage => routePlan(stage as RouteRecord))) : undefined;
        return routePlan(route as RouteRecord);
    });

    /** The routed line of the current plan only, so figures and time never mix two plans. */
    readonly preview = $derived(this.routed && this.routed.plan === this.plan ? this.routed.line : null);

    /** The listed routes as numbered lines; the other matches as start dots. */
    readonly mapView = $derived.by((): SignedRoutesView | null => {
        const start = this.start, detail = this.detail;
        if (!start) return null;
        const startOf = ({ start_udeg, line_udeg }: CatalogRecord): Coordinate => {
            const [lon, lat] = start_udeg ?? line_udeg!;
            return [lon / 1e6, lat / 1e6];
        };
        return {
            center: start.coordinate, radiusKm: this.filters.radiusKm,
            lines: this.matches.slice(0, this.shown).map(({ route }, i) => {
                const line = recordLine(route, route.stages?.flatMap(id => this.records.get(id) ?? []));
                return { id: route.id, number: i + 1, rank: route.rank, line: line.length ? line : [startOf(route)],
                    label: `${route.name ?? route.ref} · ${(route.length_m / 1000).toFixed(1)} km · ↑ ${route.ascent_m} m` };
            }),
            dots: this.matches.slice(this.shown).map(({ route }) => ({ id: route.id, rank: route.rank, at: startOf(route) })),
            selected: detail ? { id: detail.route.id, line: recordLine(detail.route, detail.stages) } : null,
        };
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
        const query = { ...$state.snapshot(this.filters), start: start.coordinate, activity };
        this.status = 'loading';
        this.progress = { loaded: 0, total: 0 };
        const counted = (cell: string) => {
            if (id === this.serial) this.progress.total++;
            return this.cell(cell).finally(() => { if (id === this.serial) this.progress.loaded++; });
        };
        try {
            const matches = await searchRoutes(query, counted);
            let wider: RouteFinder['wider'] = null, blocker: RouteFilter | null = null;
            const graded = activity === 'hiking' || activity === 'mtb';
            const active = (key: RouteFilter) => key === 'hardest' ? graded && (query.hardest[0] > 0 || query.hardest[1] < 3)
                : query[key].from !== undefined || query[key].to !== undefined;
            for (const key of matches.length ? [] : (['distanceKm', 'climbM', 'hardest'] as const).filter(active)) {
                if ((await searchRoutes({ ...query, [key]: open[key] }, counted)).length) { blocker = key; break; }
            }
            if (!matches.length && !blocker && query.radiusKm < RADII.at(-1)!) {
                const wide = await searchRoutes({ ...query, radiusKm: RADII.at(-1)! }, counted);
                const radiusKm = RADII.find(km => km > query.radiusKm && wide.some(match => match.distanceM <= km * 1000));
                if (radiusKm) wider = { radiusKm, count: wide.filter(match => match.distanceM <= radiusKm * 1000).length };
            }
            if (id !== this.serial) return;
            this.matches = matches;
            this.wider = wider;
            this.blocker = blocker;
            this.shown = 20;
            this.status = 'ready';
        } catch {
            if (id === this.serial) this.status = 'failed';
        }
    }

    /** Removes a filter: no distance or climb bound, or every grade. */
    clear(filter: RouteFilter) {
        if (filter === 'hardest') this.filters.hardest = [0, 3];
        else this.filters[filter] = {};
    }

    /** Shows a record, and loads the stages of its long route. */
    async select(id: number | null) {
        this.routed = null;
        const route = id === null ? undefined : this.records.get(id);
        if (!route) { this.detail = null; return; }
        const detail: RouteDetail = { route, distanceM: this.matches.find(match => match.route.id === id)?.distanceM };
        this.detail = detail;
        const family = route.stages ? route : route.parent ? this.records.get(route.parent) : undefined;
        if (!family?.stages) return;
        const stages = await this.stagesOf(family).catch(() => undefined);
        if (this.detail === detail) this.detail = stages ? { ...detail, family, stages } : { ...detail, failed: true };
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
        return route.stages ? joinedPlan((await this.stagesOf(route)).map(stage => routePlan(stage as RouteRecord))) : routePlan(route as RouteRecord);
    }
}
