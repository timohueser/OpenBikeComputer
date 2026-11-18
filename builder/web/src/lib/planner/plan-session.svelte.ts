import { SvelteMap } from 'svelte/reactivity';
import { anchorMarker, emptyTrip, guessedTrip, hasEndpoints, insertPoint, orderedRoutePoints, pinNight, planTitle, routeLegsAround, type RoutePoint, type Trip } from './editor';
import type { Coordinate } from './geo';
import { importedTrip, planOnRoads, readTracks, type ImportedLine } from './gpx-import';
import type { Library } from '../library/store';
import { downloadPlan, importPlan, isPlan, newPlan, planSummary, type Plan } from './library';
import { coordinateName, townName, visitName } from './point-names';
import { presetName } from './riding-profiles';
import { optionOrder } from './route-options';
import { LegCache } from './route-legs';
import { routePreview } from './route-preview';
import { calculateLine, profileId, requestAlternatives, requestShape, routingKey, type EngineRoute, type RoutingLine } from './routing';
import { planTrip, routeBase, type RoutePlan } from './signed-route-plan';
import type { CatalogRecord } from './signed-routes';

type Routed = { trip: Trip; line?: RoutingLine };
/** An Undo step. */
type Step = { trip: Trip };
export type RouteOptions = { state: 'unavailable' | 'loading' | 'stale' } | { state: 'error'; message: string }
    | { state: 'ready'; routes: EngineRoute[]; choice: string };
type PlaceName = (coordinate: Coordinate) => string | undefined;

const historyLimit = 50;
/** Undo and redo show these recent lines at once; the leg cache calculates older lines again without a request. */
const lineLimit = 10;

function storageError(error: unknown): string {
    if (error instanceof Error && error.name === 'QuotaExceededError') return 'Browser storage is full. Download your plan, remove unused plans, and retry saving.';
    return error instanceof Error ? error.message : 'Could not save in this browser. Download your plan and try again.';
}

// A moved night keeps its number, and a moved marker takes the leg nearest to it. A moved point no longer stands for its
// mapped place. Moving a point routes its drawn legs again.
function movedPoint(trip: Trip, line: RoutingLine | undefined, id: string, coordinate: Coordinate): Trip {
    const point = trip.points.find(p => p.id === id);
    if (!point) return trip;
    if (point.kind === 'night') return routeLegsAround(pinNight(trip, line, point.night!, coordinate, point.label), id);
    const moved = { ...point, coordinate, label: point.autoLabel ? coordinateName(coordinate) : point.label, placeId: undefined, placeKind: undefined };
    return routeLegsAround({ ...trip, points: trip.points.map(p => p !== point ? p : p.kind === 'marker' ? anchorMarker(trip, line, moved) : moved) }, id);
}

/**
 * The open plan: its trip, Undo history, routed line, drag preview and saved record in the browser library. The planner
 * page shows it and calls its commands. A change of the points starts the calculation of the new line.
 */
export class PlanSession {
    /** The saved record of the trip: its id, name and revision. */
    plan = $state.raw<Plan>(newPlan(emptyTrip()));
    /** The library has opened, or failed to open. */
    ready = $state(false);
    /** A library action runs. */
    busy = $state(false);
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

    // The steps in Undo order; `at` is the open one. Raw state: a change replaces a step and never edits it in place.
    private history = $state.raw<Step[]>([{ trip: emptyTrip() }]);
    private at = $state(0);
    readonly trip = $derived(this.history[this.at].trip);
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
    private optionsRequest?: { key: string; abort: AbortController };
    private optionsError = $state.raw<{ key: string; message: string } | null>(null);
    /** The route options of the trip, the rider's current route (`choice`) first. Options found for another trip are
     * `stale` and never apply. */
    readonly routeOptions = $derived.by((): RouteOptions => {
        const key = routingKey(this.trip), pick = this.options?.picks[key];
        if (pick) return { state: 'ready', routes: optionOrder(this.options!.routes, pick.id), choice: pick.id };
        if (this.routeError || (this.line && !this.line.primary)) return { state: 'unavailable' };
        if (this.optionsError?.key === key) return { state: 'error', message: this.optionsError.message };
        return { state: this.options ? 'stale' : 'loading' };
    });
    /** Settlement names near the `via` of corridor options, by route id. */
    optionPlaces = $state.raw<Record<string, string>>({});
    // The option the rider previews. Only `pickAlternative` changes the trip.
    private previewId = $state<string | null>(null);
    /** The previewed option, while it is an option of the trip other than the current route. */
    readonly compared = $derived.by(() => {
        const options = this.routeOptions;
        return options.state === 'ready' && this.previewId !== options.choice ? options.routes.find(route => route.id === this.previewId) : undefined;
    });

    /** The lines that `findLines` found by routing key, or the message of a failed calculation. */
    foundLines = $state.raw<Record<string, RoutingLine | string>>({});
    private linesRequest?: AbortController;
    private wantedLines = new Set<string>();

    private readonly legs = new LegCache();
    private library?: Library;
    // The calculation in flight, or the one that failed, so a label edit neither restarts nor clears it.
    private request?: { key: string; abort: AbortController };
    private lastSave: Promise<void> = Promise.resolve();
    // A line arrived while `busy`; its summary is saved after the library action.
    private unsaved = false;
    private closed = false;
    private stopFollowing = () => {};
    private readonly previews = routePreview((draft: Trip, signal: AbortSignal) => this.calculate(draft, signal), (trip, line) => {
        this.preview = this.last = { trip, line };
        this.previewStatus = 'Route preview · release to keep';
    }, error => { this.previewStatus = error instanceof Error ? error.message : 'Preview unavailable.'; });

    /** `opened` runs after another plan replaces the trip, so the page can reset its view. */
    constructor(private readonly opened: (plan: Plan) => void = () => {}) {}

    /** Opens the plan that was active when the planner closed, and follows the changes of the open plan by other pages. */
    async start(library: Library): Promise<void> {
        this.library = library;
        this.stopFollowing = library.onChange((store, key) => { if (store === 'routes' && key === this.plan.id) void this.follow(key); });
        try {
            const id = await library.state('active');
            const plan = id ? await this.get(library, id) : undefined;
            if (plan && !this.closed) this.install(plan);
        } catch (error) { this.saveError = storageError(error); }
        finally { this.ready = true; }
    }

    close(): void {
        this.closed = true;
        this.stopFollowing();
        this.cancelPreview();
        this.request?.abort.abort();
        this.optionsRequest?.abort.abort();
        this.linesRequest?.abort();
    }

    /** Calculates the line of any plan with the leg cache of the session, such as a route of the catalog. */
    calculate(trip: Trip, signal: AbortSignal): Promise<RoutingLine> {
        return calculateLine(trip, signal, this.legs);
    }

    /** Replaces the trip with an edited copy as one Undo step. `line`, when known, is the line of `next`. A day count that
     * the rider did not set follows the line of the step, here or when the line arrives. */
    commit(next: Trip, line?: RoutingLine): void {
        this.step({ trip: next }, line);
    }

    undo(): boolean {
        return this.canUndo && this.travel(this.at - 1);
    }

    redo(): boolean {
        return this.canRedo && this.travel(this.at + 1);
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
        this.show(this.history.map((step, i) => i === this.at ? { ...step, trip: named } : step), this.at);
        this.autosave();
    }

    /** Requests the alternatives of the line when they are not known and no request for the trip is in flight. A request
     * for another trip stops, and an answer for another trip is dropped. A corridor without `via` cannot become a plan, so
     * it is no option. A corridor option is named once its settlement is known. */
    findAlternatives(): void {
        const trip = this.trip, primary = this.line?.primary, key = routingKey(trip);
        if (this.optionsRequest && this.optionsRequest.key !== key) {
            this.optionsRequest.abort.abort();
            this.optionsRequest = undefined;
        }
        if (!primary || this.routeOptions.state === 'ready' || this.optionsRequest) return;
        const request = this.optionsRequest = { key, abort: new AbortController() };
        this.optionsError = null;
        requestAlternatives(trip, primary, request.abort.signal).then(answer => {
            if (request.abort.signal.aborted) return;
            this.optionsRequest = undefined;
            if (routingKey(this.trip) !== key) return;
            const routes = answer.filter(route => route.reason !== 'corridor' || route.via);
            this.options = { routes, picks: { [key]: { id: primary.id } } };
            for (const route of routes) {
                if (route.via && !this.optionPlaces[route.id]) void townName(route.via).then(name => { if (name) this.optionPlaces = { ...this.optionPlaces, [route.id]: name }; });
            }
        }, error => {
            if (request.abort.signal.aborted) return;
            this.optionsRequest = undefined;
            if (routingKey(this.trip) === key) this.optionsError = { key, message: error instanceof Error ? error.message : 'Route options are unavailable.' };
        });
    }

    /** Shows option `id` beside the current route, or ends the preview with null. The trip, its history and the saved
     * plan stay as they are. */
    previewOption(id: string | null): void {
        this.previewId = id;
    }

    /** Calculates the lines of `trips` that `foundLines` has not, and with `retry` the failed ones. One request runs at a
     * time; when it answers, the lines of trips that the last call did not name are dropped. */
    findLines(trips: Trip[], retry = false): void {
        const wanted = new Map(trips.map(trip => [routingKey(trip), trip] as const));
        this.wantedLines = new Set(wanted.keys());
        const missing = [...wanted].filter(([key]) => this.foundLines[key] === undefined || (retry && typeof this.foundLines[key] === 'string'));
        if (!missing.length || this.linesRequest) return;
        const abort = this.linesRequest = new AbortController();
        void (async () => {
            // One trip after the other, so that a later trip finds the shared legs in the cache.
            const found: (readonly [string, RoutingLine | string])[] = [];
            for (const [key, trip] of missing) found.push(await (this.lines.get(key) ? Promise.resolve(this.lines.get(key)!) : this.calculate(trip, abort.signal)).then(
                line => [key, line] as const, error => [key, error instanceof Error ? error.message : 'Routing is unavailable.'] as const));
            return found;
        })().then(found => {
            if (abort.signal.aborted) return;
            this.linesRequest = undefined;
            const lines: Record<string, RoutingLine | string> = { ...this.foundLines, ...Object.fromEntries(found) };
            this.foundLines = Object.fromEntries(Object.entries(lines).filter(([key]) => this.wantedLines.has(key)));
        });
    }

    /** Applies one of the alternatives to the trip they were found for, as one Undo step: a corridor becomes one shaping
     * point at its `via`, the profile of the route becomes the preset. The list stays, with the pick as the current
     * route. A profile route keeps its legs, so its line needs no request. False when the route is already the current
     * one or the options belong to another trip. */
    pickAlternative(route: EngineRoute): boolean {
        const options = this.options, current = options?.picks[routingKey(this.trip)];
        if (!options || !current || current.id === route.id || !options.routes.includes(route)) return false;
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

    /** Saves the trip. Writes run in order. A failed write shows its error until a later write succeeds. */
    save(): Promise<void> {
        if (!this.ready) return Promise.resolve();
        const plan = this.plan = this.snapshot();
        if (!plan.trip.points.length && !plan.revision) return Promise.resolve();
        // `ready` is set after `start` gave the library.
        const library = this.library!;
        this.lastSave = library.saveRoute(plan).then(async saved => {
            await library.setState('active', saved.id);
            if (this.plan.id !== saved.id) return;
            this.plan = { ...this.plan, revision: saved.revision };
            this.saveError = '';
            this.plans = this.plans.map(p => p.id === saved.id ? saved : p);
        });
        this.lastSave.catch(error => { if (this.plan.id === plan.id) this.saveError = storageError(error); });
        return this.lastSave;
    }

    /** Waits for the last save; true when it and every save before it succeeded. */
    saved(): Promise<boolean> {
        return this.lastSave.then(() => !this.saveError, () => false);
    }

    /** Downloads a plan of the library; the open plan with its latest edits, also when the library fails. */
    download(plan: Plan = this.plan): void {
        downloadPlan(plan.id === this.plan.id ? this.snapshot() : plan);
    }

    listPlans(): Promise<boolean> {
        return this.act(async library => {
            await this.lastSave.catch(() => {});
            await this.list(library);
        });
    }

    /** Opens plan `id` of the library. When the open plan has unsaved edits, only `discard` lets them go. */
    open(id: string, discard: () => boolean): Promise<boolean> {
        return this.act(async library => {
            try { await this.lastSave; } catch { if (!discard()) return false; }
            const latest = await this.get(library, id);
            if (!latest) throw new Error('This plan was deleted. Reopen My plans.');
            await library.setState('active', latest.id);
            this.install(latest);
        });
    }

    /** `follow` gives the open plan its new name. */
    rename(plan: Plan, name: string): Promise<boolean> {
        return this.act(async library => {
            await library.renameRoute(plan.id, name);
            await this.list(library);
        });
    }

    duplicate(plan: Plan): Promise<boolean> {
        const source = plan.id === this.plan.id ? this.snapshot() : plan;
        return this.create(() => ({ ...newPlan(source.trip, `${source.name || planTitle(source.trip)} (copy)`), summary: source.summary }));
    }

    /** Deletes the records of the library that are not valid plans. */
    removeUnreadable(): Promise<boolean> {
        return this.act(async library => {
            for (const id of await this.unreadableIds(library)) await library.removeRoute(id);
            await this.list(library);
        });
    }

    /** Deletes a plan; deleting the open plan opens a new empty one. */
    remove(plan: Plan): Promise<boolean> {
        return this.act(async library => {
            await this.lastSave.catch(() => {});
            const active = plan.id === this.plan.id;
            await library.removeRoute(plan.id, (active ? this.plan : plan).revision);
            if (active) this.install(newPlan(emptyTrip(this.trip.mode)));
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
            return newPlan({ ...trip, points: trip.points.map(p => p.kind === 'via' || p.kind === 'marker' ? p : { ...p, label: placeName(p.coordinate) ?? p.label }) }, lines[0].name);
        });
        return done ? failed : undefined;
    }

    async planSignedRoute(route: CatalogRecord, plan: RoutePlan, placeName: PlaceName): Promise<boolean> {
        const trip = planTrip(routeBase(route, this.trip), plan, route.loop);
        // A failure before the install leaves the open plan as it was, so it is no save error of that plan; a failed
        // list after the install still opened the new plan.
        const before = this.plan;
        return await this.create(() => newPlan({ ...trip, points: trip.points.map(p => p.kind === 'start' || p.kind === 'finish' ? { ...p, label: placeName(p.coordinate) ?? p.label } : p) })) || this.plan !== before;
    }

    /** Opens a new empty plan with the activity and preset of the open plan. */
    async newPlan(): Promise<boolean> {
        const done = await this.act(async library => {
            await this.lastSave;
            await library.setState('active', null);
            this.install(newPlan({ ...emptyTrip(this.trip.mode), bike: this.trip.bike, preset: this.trip.preset }));
        });
        if (!done) this.saveError = this.libraryError;
        return done;
    }

    /** Takes the stored name of the open plan after a write of this tab; a removed plan gives a new empty plan, and its
     * failed save shows no error, as the error belongs to another plan. The library keeps the revision that this tab
     * knows. */
    private async follow(id: string): Promise<void> {
        const record = await this.library!.item(id).catch(() => null);
        if (this.closed || this.plan.id !== id || record === null) return;
        if (!record) this.install(newPlan(emptyTrip(this.trip.mode)));
        else if (record.name !== this.plan.name) this.plan = { ...this.plan, name: record.name };
    }

    private snapshot(): Plan {
        // A save with no calculated line keeps the last summary.
        const summary = this.line ? planSummary(this.trip, this.line) : this.plan.summary;
        return { ...this.plan, trip: this.trip, summary, updatedAt: Date.now() };
    }

    private step(next: Step, line?: RoutingLine): void {
        const kept = this.history.slice(0, this.at + 1).slice(-historyLimit);
        if (line) this.remember(routingKey(next.trip), line);
        this.cancelPreview();
        this.show([...kept, { ...next, trip: guessedTrip(next.trip, this.lines.get(routingKey(next.trip))) }], kept.length);
        this.autosave();
    }

    private autosave(): void {
        this.save().catch(() => {});
    }

    /** Runs a library action; its error shows in `libraryError`. True when it completed and was not cancelled. */
    private async act(work: (library: Library) => Promise<boolean | void>): Promise<boolean> {
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

    private create(make: () => Plan | Promise<Plan>): Promise<boolean> {
        return this.act(async library => {
            const plan = await make();
            await this.lastSave;
            const saved = await library.saveRoute(plan);
            await library.setState('active', saved.id);
            this.install(saved);
            await this.list(library);
        });
    }

    /** The routes of the library that are plans, most recently edited first, and the number of the others. */
    private async list(library: Library): Promise<void> {
        const routes = await library.routes();
        this.plans = routes.filter(isPlan).sort((a, b) => b.updatedAt - a.updatedAt);
        this.unreadable = routes.length - this.plans.length + (await library.unreadableRoutes()).length;
    }

    /** The ids of the records that are no route, and of the routes that are no plan. */
    private async unreadableIds(library: Library): Promise<string[]> {
        return [...await library.unreadableRoutes(), ...(await library.routes()).filter(route => !isPlan(route)).map(route => route.id)];
    }

    private async get(library: Library, id: string): Promise<Plan | undefined> {
        const route = await library.route(id);
        if (route && !isPlan(route)) throw new Error('Saved plan is invalid. Open another plan or import a backup.');
        return route;
    }

    private install(plan: Plan): void {
        this.cancelPreview();
        this.plan = plan;
        this.last = null;
        this.options = null;
        this.saveError = '';
        this.show([{ trip: plan.trip }], 0);
        this.opened(plan);
    }

    private travel(at: number): true {
        this.cancelPreview();
        this.show(this.history, at);
        this.autosave();
        return true;
    }

    private show(history: Step[], at: number): void {
        this.history = history;
        this.at = at;
        const trip = this.trip, line = this.line;
        this.last = line ? { trip, line } : hasEndpoints(trip) ? this.last : null;
        this.route();
    }

    /** Ends the drag preview and the option preview. */
    private cancelPreview(): void {
        this.previews.cancel();
        this.preview = null;
        this.dragging = false;
        this.previewId = null;
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
            // Undo and Redo reuse the cached line and never guess, so every step of this route takes the guess now.
            this.history = this.history.map(step => routingKey(step.trip) === key ? { ...step, trip: guessedTrip(step.trip, line) } : step);
            this.last = { trip: this.trip, line };
            if (this.busy) this.unsaved = true;
            else this.autosave();
        }, error => {
            if (!request.abort.signal.aborted) this.routeError = error instanceof Error ? error.message : 'Routing is unavailable.';
        });
    }
}
