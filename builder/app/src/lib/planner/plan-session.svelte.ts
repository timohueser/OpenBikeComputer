import { SvelteMap } from 'svelte/reactivity';
import { anchorMarker, emptyTrip, hasEndpoints, insertPoint, orderedRoutePoints, pinNight, planTitle, routeLegsAround, type RoutePoint, type Trip } from './editor';
import type { Coordinate } from './geo';
import { importedTrip, planOnRoads, readTracks, type ImportedLine } from './gpx-import';
import { downloadPlan, importPlan, newPlan, PlanLibrary, type Plan } from './library';
import { coordinateName, visitName } from './point-names';
import { presetName } from './riding-profiles';
import { LegCache } from './route-legs';
import { routePreview } from './route-preview';
import { calculateLine, profileId, requestAlternatives, requestShape, routingKey, type EngineRoute, type RoutingLine } from './routing';
import { planTrip, routeBase, type RoutePlan } from './signed-route-plan';
import type { CatalogRecord } from './signed-routes';
import { isTrip } from './trip-validation';
import { versionSummary, type Version } from './versions';

type Routed = { trip: Trip; line?: RoutingLine };
type Created = { plan: Plan; versions?: Version[] };
type PlaceName = (coordinate: Coordinate) => string | undefined;

const historyLimit = 50;
/** Undo and redo show these recent lines at once; the leg cache calculates older lines again without a request. */
const lineLimit = 10;

function storageError(error: unknown): string {
    if (error instanceof Error && error.name === 'QuotaExceededError') return 'Browser storage is full. Download your plan, remove unused plans, and retry saving.';
    return error instanceof Error ? error.message : 'Could not save in this browser. Download your plan and try again.';
}

// A moved night keeps its number, and a moved marker takes the leg nearest to it. Moving a point routes its drawn legs again.
function movedPoint(trip: Trip, line: RoutingLine | undefined, id: string, coordinate: Coordinate): Trip {
    const point = trip.points.find(p => p.id === id);
    if (!point) return trip;
    if (point.kind === 'night') return routeLegsAround(pinNight(trip, line, point.night!, coordinate, point.label), id);
    const moved = { ...point, coordinate, label: point.autoLabel ? coordinateName(coordinate) : point.label };
    return routeLegsAround({ ...trip, points: trip.points.map(p => p !== point ? p : p.kind === 'marker' ? anchorMarker(trip, line, moved) : moved) }, id);
}

/**
 * The open plan: its trip, Undo history, routed line, drag preview and saved record in the browser library. The planner
 * page shows it and calls its commands. A change of the points starts the calculation of the new line.
 */
export class PlanSession {
    /** The saved record of the trip: its id, name and revision. */
    plan = $state.raw<Plan>(newPlan(emptyTrip()));
    /** The saved versions of the plan, newest first. */
    versions = $state.raw<Version[]>([]);
    /** The library has opened, or failed to open. */
    ready = $state(false);
    /** A library action or a version save runs. */
    busy = $state(false);
    savedAt = $state<number | null>(null);
    saveError = $state('');
    plans = $state.raw<Plan[]>([]);
    /** Records of the library that are not valid plans; `plans` leaves them out. */
    unreadable = $state(0);
    libraryError = $state('');
    /** GPX files read and waiting for the rider to keep their lines or plan them on roads. */
    gpxLines = $state.raw<ImportedLine[] | null>(null);
    routeError = $state('');
    dragging = $state(false);
    previewStatus = $state('');
    alternativesStatus = $state('');

    // The trips in Undo order; `at` is the open one. Raw state: a change replaces a trip and never edits it in place.
    private history = $state.raw<Trip[]>([emptyTrip()]);
    private at = $state(0);
    readonly trip = $derived(this.history[this.at]);
    readonly canUndo = $derived(this.at > 0);
    readonly canRedo = $derived(this.at < this.history.length - 1);

    private readonly lines = new SvelteMap<string, RoutingLine>();
    /** The line of the trip, once it is calculated. */
    readonly line = $derived(hasEndpoints(this.trip) ? this.lines.get(routingKey(this.trip)) : undefined);
    private preview = $state.raw<Required<Routed> | null>(null);
    private last = $state.raw<Required<Routed> | null>(null);
    /** What the map, the panel and the profile show: the drag preview, the trip, or the last routed trip while the line of
     * the trip is calculated. A failed calculation shows no line. */
    readonly shown = $derived.by((): Routed => {
        const current: Routed = this.preview ?? { trip: this.trip, line: this.line };
        return current.line || this.routeError || !this.last ? current : this.last;
    });
    /** The shown line belongs to an earlier trip. */
    readonly stale = $derived(this.shown.trip !== (this.preview?.trip ?? this.trip));

    // The alternatives found for one trip, and by routing key the trips that it and its picks gave: the checked route and
    // the shaping point that a corridor pick added.
    private options = $state.raw<{ routes: EngineRoute[]; picks: Record<string, { id: string; via?: string }> } | null>(null);
    /** The alternatives of the trip and the id of the checked one, when they are known. */
    readonly alternatives = $derived.by(() => {
        const pick = this.options?.picks[routingKey(this.trip)];
        return pick && { routes: this.options!.routes, choice: pick.id };
    });

    private readonly legs = new LegCache();
    private library?: PlanLibrary;
    // The calculation in flight, or the one that failed, so a label edit neither restarts nor clears it.
    private request?: { key: string; abort: AbortController };
    private lastSave: Promise<void> = Promise.resolve();
    // A line arrived while `busy`; its summary is saved afterwards, so a failed version save leaves no trace.
    private unsaved = false;
    private closed = false;
    private readonly previews = routePreview((draft: Trip, signal: AbortSignal) => this.calculate(draft, signal), (trip, line) => {
        this.preview = this.last = { trip, line };
        this.previewStatus = 'Route preview · release to keep';
    }, error => { this.previewStatus = error instanceof Error ? error.message : 'Preview unavailable.'; });

    /** `opened` runs after another plan replaces the trip, so the page can reset its view. */
    constructor(private readonly opened: (plan: Plan) => void = () => {}) {}

    /** Opens the library and the plan that was active when the planner closed. */
    async start(factory?: IDBFactory): Promise<void> {
        try {
            this.library = new PlanLibrary(factory);
            const plan = await this.library.active();
            if (plan) {
                const versions = await this.library.versions(plan.id);
                if (!this.closed) this.install(plan, versions);
            }
        } catch (error) { this.saveError = storageError(error); }
        finally { this.ready = true; }
    }

    close(): void {
        this.closed = true;
        this.cancelPreview();
        this.request?.abort.abort();
        void this.library?.close();
    }

    /** Calculates the line of any plan with the leg cache of the session, such as a route of the catalog. */
    calculate(trip: Trip, signal: AbortSignal): Promise<RoutingLine> {
        return calculateLine(trip, signal, this.legs);
    }

    /** Replaces the trip with an edited copy as one Undo step. `line`, when known, is the line of `next`. */
    commit(next: Trip, line?: RoutingLine): void {
        const kept = this.history.slice(0, this.at + 1).slice(-historyLimit);
        if (line) this.remember(routingKey(next), line);
        this.cancelPreview();
        this.show([...kept, next], kept.length);
        this.autosave();
    }

    undo(): boolean {
        return this.canUndo && this.travel(this.at - 1);
    }

    redo(): boolean {
        return this.canRedo && this.travel(this.at + 1);
    }

    /** Restores a saved version as one Undo step; false when it is not a valid plan. */
    restore(saved: Trip): boolean {
        if (!isTrip(saved)) return false;
        this.commit(saved);
        return true;
    }

    /** Shows the route with point `id` at `coordinate` once the pointer rests there. The trip does not change. */
    previewMove(id: string, coordinate: Coordinate): void {
        if (!hasEndpoints(this.trip)) return;
        this.dragging = true;
        this.previewStatus = 'Hold still to preview the route';
        this.previews.move(movedPoint(this.trip, this.line, id, coordinate));
    }

    /** Moves point `id`. A preview of the same points gives the line at once. */
    movePoint(id: string, coordinate: Coordinate): void {
        const moved = movedPoint(this.trip, this.line, id, coordinate), preview = this.preview;
        this.commit(moved, preview && routingKey(preview.trip) === routingKey(moved) ? preview.line : undefined);
    }

    /** Names a visit whose label is still its coordinate after the nearest address. A name is metadata: no Undo step. */
    async nameVisit(point: RoutePoint): Promise<void> {
        if (!point.autoLabel || !['waypoint', 'detour'].includes(point.kind) || point.label !== coordinateName(point.coordinate)) return;
        const coordinate: Coordinate = [...point.coordinate];
        const label = await visitName(coordinate);
        const current = this.trip.points.find(p => p.id === point.id);
        if (!label || this.closed || !current?.autoLabel || current.coordinate[0] !== coordinate[0] || current.coordinate[1] !== coordinate[1]) return;
        const named = { ...this.trip, points: this.trip.points.map(p => p.id === point.id ? { ...p, label } : p) };
        this.show(this.history.map((trip, i) => i === this.at ? named : trip), this.at);
        this.autosave();
    }

    /** Requests the alternatives of the line when they are not known. Returns the cancel of that request. */
    findAlternatives(): (() => void) | undefined {
        const trip = this.trip, primary = this.line?.primary;
        if (!primary || this.alternatives) return;
        const abort = new AbortController();
        this.alternativesStatus = 'Finding alternative routes…';
        requestAlternatives(trip, primary, abort.signal).then(routes => {
            if (abort.signal.aborted) return;
            this.options = { routes, picks: { [routingKey(trip)]: { id: primary.id } } };
            this.alternativesStatus = '';
        }, error => { if (!abort.signal.aborted) this.alternativesStatus = error instanceof Error ? error.message : 'Alternatives unavailable.'; });
        return () => abort.abort();
    }

    /** Applies one of the alternatives to the trip they were found for: a corridor becomes one shaping point at its `via`,
     * the profile of the route becomes the preset. The list stays, with the pick checked. A profile route keeps its legs,
     * so its line needs no request. False when the route is already the checked one. */
    pickAlternative(route: EngineRoute): boolean {
        const options = this.options, current = options?.picks[routingKey(this.trip)];
        if (!options || !current || current.id === route.id) return false;
        const { trip } = this, via = current.via;
        const found = via ? { ...trip, points: trip.points.filter(p => p.id !== via), routeOrder: trip.routeOrder.filter(id => id !== via) } : trip;
        const preset = { ...found, preset: presetName(route.profile) };
        const next = route.via ? insertPoint(preset, orderedRoutePoints(preset).at(-1)!.id, route.via) : preset;
        if (!route.via) this.legs.add(orderedRoutePoints(next).map(p => p.coordinate), route);
        const added = next.points.find(p => !found.points.some(q => q.id === p.id));
        this.options = { ...options, picks: { ...options.picks, [routingKey(next)]: { id: route.id, via: added?.id } } };
        this.commit(next);
        return true;
    }

    retry(): void {
        this.route(true);
    }

    /** Saves the trip, and `versions` when given. Writes run in order. A failed write shows its error until a later write
     * succeeds. */
    save(versions?: Version[]): Promise<void> {
        if (!this.ready) return Promise.resolve();
        const plan = this.plan = this.snapshot();
        if (!plan.trip.points.length && !plan.revision) return Promise.resolve();
        const library = this.library;
        this.lastSave = library ? library.save(plan, versions).then(async saved => {
            await library.activate(saved.id);
            if (this.plan.id !== saved.id) return;
            this.plan = { ...this.plan, revision: saved.revision };
            if (versions) this.versions = versions;
            this.savedAt = saved.updatedAt;
            this.saveError = '';
            this.plans = this.plans.map(p => p.id === saved.id ? saved : p);
            if (saved.revision === 1) void navigator.storage?.persist?.().catch(() => {});
        }) : Promise.reject(new Error('Browser storage is unavailable. Download your plan to keep it.'));
        this.lastSave.catch(error => { if (this.plan.id === plan.id) this.saveError = storageError(error); });
        return this.lastSave;
    }

    /** Saves the versions of the plan; a failed save keeps the versions it had. */
    async saveVersions(versions: Version[]): Promise<void> {
        this.busy = true;
        try { await this.save(versions); }
        finally { this.idle(); }
    }

    /** Downloads a plan of the library; the open plan with its latest edits, also when the library fails. */
    async download(plan: Plan = this.plan): Promise<void> {
        if (plan.id === this.plan.id) downloadPlan(this.snapshot(), this.versions);
        else await this.act(async library => downloadPlan(plan, await library.versions(plan.id)));
    }

    listPlans(): Promise<boolean> {
        return this.act(async library => {
            await this.lastSave.catch(() => {});
            await this.list(library);
        });
    }

    /** Opens a plan of the library. When the open plan has unsaved edits, only `discard` lets them go. */
    open(plan: Plan, discard: () => boolean): Promise<boolean> {
        return this.act(async library => {
            try { await this.lastSave; } catch { if (!discard()) return false; }
            const latest = await library.get(plan.id);
            if (!latest) throw new Error('This plan was deleted. Reopen My plans.');
            const versions = await library.versions(latest.id);
            await library.activate(latest.id);
            this.install(latest, versions);
        });
    }

    rename(plan: Plan, name: string): Promise<boolean> {
        return this.act(async library => {
            await this.lastSave;
            const active = plan.id === this.plan.id;
            const saved = await library.save({ ...(active ? this.snapshot() : plan), name, updatedAt: Date.now() });
            if (active) this.plan = { ...this.plan, name, revision: saved.revision };
            await this.list(library);
        });
    }

    duplicate(plan: Plan): Promise<boolean> {
        const open = plan.id === this.plan.id, source = open ? this.snapshot() : plan;
        return this.create(async library => ({ plan: newPlan(source.trip, `${source.name || planTitle(source.trip)} (copy)`),
            versions: open ? this.versions : await library.versions(plan.id) }));
    }

    /** Deletes a plan; deleting the open plan opens a new empty one. */
    remove(plan: Plan): Promise<boolean> {
        return this.act(async library => {
            await this.lastSave.catch(() => {});
            const active = plan.id === this.plan.id;
            await library.remove(active ? this.plan : plan);
            if (active) this.install(newPlan(emptyTrip(this.trip.mode)), []);
            await this.list(library);
        });
    }

    /** Plan files open one by one; GPX files wait in `gpxLines` for one choice for all of them. */
    async importFiles(files: File[]): Promise<void> {
        this.gpxLines = null;
        const gpx = files.filter(file => /\.gpx$/i.test(file.name));
        if (!gpx.length) {
            for (const file of files) await this.create(async () => importPlan(await file.text()));
            return;
        }
        await this.act(async () => {
            if (gpx.length < files.length) throw new Error('Import GPX files and plan files separately.');
            this.gpxLines = readTracks(await Promise.all(gpx.map(async file => ({ name: file.name, text: await file.text() }))));
        });
    }

    /** Opens the waiting GPX lines as one plan, planned on roads or kept as drawn. Returns the names of the files that kept
     * their line, or undefined when the import failed. */
    async importGpx(roads: boolean, placeName: PlaceName): Promise<string[] | undefined> {
        const lines = this.gpxLines!;
        this.gpxLines = null;
        let failed: string[] = [];
        const done = await this.create(async () => {
            const planned = roads ? await planOnRoads(lines, line => requestShape(line, profileId(this.trip))) : { lines, failed: [] };
            const trip = importedTrip(this.trip, planned.lines);
            failed = planned.failed;
            return { plan: newPlan({ ...trip, points: trip.points.map(p => p.kind === 'via' || p.kind === 'marker' ? p : { ...p, label: placeName(p.coordinate) ?? p.label }) }, lines[0].name) };
        });
        return done ? failed : undefined;
    }

    async planSignedRoute(route: CatalogRecord, plan: RoutePlan, placeName: PlaceName): Promise<boolean> {
        const trip = planTrip(routeBase(route, this.trip), plan, route.loop);
        const done = await this.create(() => ({ plan: newPlan({ ...trip, points: trip.points.map(p => p.kind === 'start' || p.kind === 'finish' ? { ...p, label: placeName(p.coordinate) ?? p.label } : p) }) }));
        if (!done) this.saveError = this.libraryError;
        return done;
    }

    /** Opens a new empty plan with the activity and preset of the open plan. */
    async newPlan(): Promise<boolean> {
        const done = await this.act(async library => {
            await this.lastSave;
            await library.activate(null);
            this.install(newPlan({ ...emptyTrip(this.trip.mode), bike: this.trip.bike, preset: this.trip.preset }), []);
        });
        if (!done) this.saveError = this.libraryError;
        return done;
    }

    private snapshot(): Plan {
        return { ...this.plan, trip: this.trip, summary: versionSummary(this.trip, this.line), updatedAt: Date.now() };
    }

    private autosave(): void {
        this.save().catch(() => {});
    }

    /** Runs a library action; its error shows in `libraryError`. True when it completed and was not cancelled. */
    private async act(work: (library: PlanLibrary) => Promise<boolean | void>): Promise<boolean> {
        this.busy = true;
        this.libraryError = '';
        try {
            if (!this.library) throw new Error('Browser storage is unavailable. Download your plan to keep it.');
            return await work(this.library) !== false;
        } catch (error) {
            this.libraryError = storageError(error);
            return false;
        } finally { this.idle(); }
    }

    private idle(): void {
        this.busy = false;
        if (this.unsaved) { this.unsaved = false; this.autosave(); }
    }

    private create(make: (library: PlanLibrary) => Created | Promise<Created>): Promise<boolean> {
        return this.act(async library => {
            const { plan, versions = [] } = await make(library);
            await this.lastSave;
            const saved = await library.save(plan, versions);
            await library.activate(saved.id);
            this.install(saved, versions);
            await this.list(library);
        });
    }

    private async list(library: PlanLibrary): Promise<void> {
        ({ plans: this.plans, unreadable: this.unreadable } = await library.list());
    }

    private install(plan: Plan, versions: Version[]): void {
        this.cancelPreview();
        this.plan = plan;
        this.versions = versions;
        this.last = null;
        this.options = null;
        this.savedAt = plan.revision ? plan.updatedAt : null;
        this.saveError = '';
        this.show([plan.trip], 0);
        this.opened(plan);
    }

    private travel(at: number): true {
        this.cancelPreview();
        this.show(this.history, at);
        this.autosave();
        return true;
    }

    private show(history: Trip[], at: number): void {
        this.history = history;
        this.at = at;
        const trip = this.trip, line = this.line;
        this.last = line ? { trip, line } : hasEndpoints(trip) ? this.last : null;
        this.route();
    }

    private cancelPreview(): void {
        this.previews.cancel();
        this.preview = null;
        this.dragging = false;
    }

    private remember(key: string, line: RoutingLine): void {
        this.lines.delete(key);
        this.lines.set(key, line);
        if (this.lines.size > lineLimit) this.lines.delete(this.lines.keys().next().value!);
    }

    /** Calculates the line of the trip, unless it is known or the same calculation is in flight or failed. */
    private route(retry = false): void {
        const trip = this.trip, key = routingKey(trip);
        if (!retry && this.request?.key === key) return;
        this.request?.abort.abort();
        this.request = undefined;
        this.routeError = '';
        if (!hasEndpoints(trip) || this.lines.has(key)) return;
        const request = this.request = { key, abort: new AbortController() };
        this.calculate(trip, request.abort.signal).then(line => {
            if (request.abort.signal.aborted) return;
            this.request = undefined;
            this.remember(key, line);
            this.last = { trip: this.trip, line };
            if (this.busy) this.unsaved = true;
            else this.autosave();
        }, error => {
            if (!request.abort.signal.aborted) this.routeError = error instanceof Error ? error.message : 'Routing is unavailable.';
        });
    }
}
