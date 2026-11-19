import { SvelteMap } from 'svelte/reactivity';
import { anchorMarker, emptyTrip, guessedTrip, hasEndpoints, insertPoint, orderedRoutePoints, pinNight, planTitle, routeLegsAround, type RoutePoint, type Trip } from './editor';
import type { Coordinate } from './geo';
import { importedTrip, planOnRoads, readTracks, type ImportedLine } from './gpx-import';
import type { DraftRecord, RouteSummary } from '../library/records';
import { OUTDATED, type Library } from '../library/store';
import { downloadPlan, importPlan, isPlan, newPlan, planSummary, type Plan } from './library';
import { coordinateName, townName, visitName } from './point-names';
import { presetName } from './riding-profiles';
import { optionOrder } from './route-options';
import { LegCache } from './route-legs';
import { routePreview } from './route-preview';
import { calculateLine, profileId, requestAlternatives, requestShape, routingKey, type EngineRoute, type RoutingLine } from './routing';
import { planTrip, routeBase, type RoutePlan } from './signed-route-plan';
import type { CatalogRecord } from './signed-routes';
import { isTrip } from './trip-validation';

type Routed = { trip: Trip; line?: RoutingLine };
/** An Undo step. `clean`: the step is the stored route, or the state with which a route that is not stored opened. At
 * most one step has it. */
type Step = {
    trip: Trip;
    clean?: true;
    /** The clean step before the save in flight: it is clean again when the save fails. A step replaced in place keeps
     * both flags. */
    before?: true;
};
export type RouteOptions = { state: 'unavailable' | 'loading' | 'stale' } | { state: 'error'; message: string }
    | { state: 'ready'; routes: EngineRoute[]; choice: string };
type PlaceName = (coordinate: Coordinate) => string | undefined;
type Offer = DraftRecord & { trip: Trip };

const historyLimit = 50;
/** Undo and redo show these recent lines at once; the leg cache calculates older lines again without a request. */
const lineLimit = 10;

function storageError(error: unknown): string {
    if (error instanceof Error && error.name === 'QuotaExceededError') return 'Browser storage is full. Delete routes you do not need, then save again.';
    if (error instanceof Error && error.name === OUTDATED) return 'The app was updated in another tab. Download the plan to keep your changes, then reload.';
    return error instanceof Error ? error.message : 'Could not save in this browser. Download your plan and try again.';
}

/** The `sessionStorage` key of the id of the route whose draft this tab writes. A reload, and a tab that the browser
 * copies or restores, keeps it. */
const DRAFT_KEY = 'obc-draft';
function ownDraft(): string | null {
    try { return sessionStorage.getItem(DRAFT_KEY); } catch { return null; }
}
function setOwnDraft(id: string | null): void {
    try { if (id) sessionStorage.setItem(DRAFT_KEY, id); else sessionStorage.removeItem(DRAFT_KEY); } catch { /* The draft then lasts only for this page. */ }
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
 * page shows it and calls its commands. A change of the points starts the calculation of the new line. Only `save` and
 * `saveCopy` store the open state; each change writes the draft of the route instead.
 */
export class PlanSession {
    /** The record of the route as it is stored, or as it opened when it is not stored. */
    plan = $state.raw<Plan>(newPlan(emptyTrip()));
    /** The route has a record in the library. */
    stored = $state(false);
    /** The draft of the open route that this tab did not write, which the rider can restore, or null. */
    offer = $state.raw<Offer | null>(null);
    /** The trip of the offer that "×" or the first edit removed, until the next change. `restore` brings it back. */
    earlier = $state.raw<Trip | null>(null);
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
    private history = $state.raw<Step[]>([{ trip: emptyTrip(), clean: true }]);
    private at = $state(0);
    readonly trip = $derived(this.history[this.at].trip);
    /** The open step differs from the stored route, or from the state with which a route that is not stored opened. */
    readonly changed = $derived(!this.history[this.at].clean);
    /** "Save" is enabled: a stored route with changes, or a route that is not stored and has points. */
    readonly canSave = $derived(this.stored ? this.changed : this.trip.points.length > 0);
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
    private lastWrite: Promise<unknown> = Promise.resolve();
    // The save in flight. Each open of a route, each discard and each later save waits for it, so a save always
    // ends on the route that it started with, and a failed save shows on that route.
    private saving: Promise<unknown> = Promise.resolve();
    private inFlight = 0;
    private closed = false;
    private stopFollowing = () => {};
    private readonly previews = routePreview((draft: Trip, signal: AbortSignal) => this.calculate(draft, signal), (trip, line) => {
        this.preview = this.last = { trip, line };
        this.previewStatus = 'Route preview · release to keep';
    }, error => { this.previewStatus = error instanceof Error ? error.message : 'Preview unavailable.'; });

    /** `opened` runs after another plan replaces the trip, so the page can reset its view. */
    constructor(private readonly opened: (plan: Plan) => void = () => {}) {}

    /** Continues the own draft of the tab with no message, else opens the route that was active when the planner closed,
     * else offers a draft of a route that is not stored. It follows the changes of the open route by other pages. */
    async start(library: Library): Promise<void> {
        this.library = library;
        this.stopFollowing = library.onChange((store, key) => { if (store === 'routes' && key === this.plan.id) void this.follow(key); });
        try {
            const own = ownDraft(), draft = own ? await library.draft(own) : undefined;
            if (draft && isTrip(draft.trip)) {
                const route = await this.get(library, draft.id);
                const plan = route ?? { ...newPlan({ ...emptyTrip(draft.trip.mode), bike: draft.trip.bike, preset: draft.trip.preset }), id: draft.id };
                if (!this.closed) this.install(plan, !!route, draft.trip);
                return;
            }
            if (own) setOwnDraft(null);
            const id = await library.state('active');
            const plan = id ? await this.get(library, id) : undefined;
            const offer = await this.offered(library, plan?.id);
            if (this.closed) return;
            if (plan) this.install(plan, true, undefined, offer);
            else if (offer) this.install({ ...this.plan, id: offer.id }, false, undefined, offer);
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
        if (this.changed) this.keepDraft();
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

    /** Stores the open state, and deletes its draft. `name` names a route that is not stored; a stored route keeps its
     * name. The open step becomes the only clean step before the write: a draft write queued before runs first, and a
     * line that arrives during the save writes no draft. A failed save gives the flags of before back. True when the
     * route is stored. */
    save(name = ''): Promise<boolean> {
        return this.store(() => ({ ...this.snapshot(), ...!this.stored && { name: name.trim().slice(0, 120) } }), saved => {
            this.plan = saved;
            this.plans = this.plans.map(p => p.id === saved.id ? saved : p);
        });
    }

    /** Stores the open state as a new route named `name`, and opens it with the Undo history. The first route stays as
     * it is stored, and its own draft goes; a draft that it offers stays for the next open. */
    saveCopy(name: string): Promise<boolean> {
        const from = this.plan.id, offered = !!this.offer;
        return this.store(() => ({ ...newPlan(this.trip, name), summary: this.snapshot().summary }), saved => {
            this.plan = saved;
            this.offer = this.earlier = null;
            if (!offered) void this.write(library => library.removeDraft(from));
        });
    }

    /** Applies the offer, or the trip that it had before it was removed, as one Undo step, and so makes it the own
     * draft. */
    restore(): boolean {
        const trip = this.offer?.trip ?? this.earlier;
        if (!trip) return false;
        this.offer = null;
        this.commit(trip);
        return true;
    }

    /** Deletes the draft of the offer. `restore` brings its trip back until the next change. */
    dismiss(): void {
        const offer = this.offer;
        if (!offer) return;
        this.offer = null;
        this.earlier = offer.trip;
        void this.write(library => library.removeDraft(offer.id)).catch(() => {});
    }

    /** Waits for the save in flight, so that `changed` tells whether the open route has changes that are not stored. */
    async whenSaved(): Promise<void> {
        await this.saving;
    }

    /** Waits for the last write; true when it and every write before it succeeded. */
    saved(): Promise<boolean> {
        return this.lastWrite.then(() => !this.saveError, () => false);
    }

    /** Commits the stored route, or the state with which a route that is not stored opened, as one Undo step that is the
     * only clean step. Its Undo is the plain Undo, which writes the draft again. */
    async discard(): Promise<boolean> {
        await this.saving;
        if (!this.changed) return false;
        this.step({ trip: this.plan.trip, clean: true });
        return true;
    }

    /** Downloads a plan of the library; the open plan with its latest edits, also when the library fails. */
    download(plan: Plan = this.plan): void {
        downloadPlan(plan.id === this.plan.id ? this.snapshot() : plan);
    }

    listPlans(): Promise<boolean> {
        return this.act(async library => {
            await this.lastWrite.catch(() => {});
            await this.list(library);
        });
    }

    /** Opens plan `id` of the library and offers its draft. The draft of the open route stays. */
    open(id: string): Promise<boolean> {
        return this.act(async library => {
            const latest = await this.get(library, id);
            if (!latest) throw new Error('This plan was deleted. Reopen My plans.');
            const offer = await this.offered(library, id);
            await library.setState('active', latest.id);
            this.install(latest, true, undefined, offer);
        });
    }

    /** `follow` gives the open plan its new name. The changes and the draft of the open plan stay. */
    rename(plan: Plan, name: string): Promise<boolean> {
        return this.act(async library => {
            await library.renameRoute(plan.id, name);
            await this.list(library);
        });
    }

    /** Stores a copy of the stored record `plan`, also when it is open with changes: "My plans" shows the stored route. */
    duplicate(plan: Plan): Promise<boolean> {
        return this.create(() => ({ ...newPlan(plan.trip, `${plan.name || planTitle(plan.trip)} (copy)`), summary: plan.summary }));
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
            await this.lastWrite.catch(() => {});
            await library.removeRoute(plan.id);
            if (plan.id === this.plan.id) this.install(newPlan(emptyTrip(this.trip.mode)), false);
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

    /** Opens a signed route as a new route that is not stored. */
    planSignedRoute(route: CatalogRecord, plan: RoutePlan, placeName: PlaceName): Promise<boolean> {
        const trip = planTrip(routeBase(route, this.trip), plan, route.loop);
        return this.openNew(newPlan({ ...trip, points: trip.points.map(p => p.kind === 'start' || p.kind === 'finish' ? { ...p, label: placeName(p.coordinate) ?? p.label } : p) }));
    }

    /** Opens a new empty plan with the activity and preset of the open plan. It takes the id of the draft that it offers,
     * so the first edit overwrites that draft. */
    newPlan(): Promise<boolean> {
        return this.openNew(newPlan({ ...emptyTrip(this.trip.mode), bike: this.trip.bike, preset: this.trip.preset }), true);
    }

    /** Takes the stored name of the open plan after a write of this tab; a removed plan gives a new empty plan, and its
     * failed save shows no error, as the error belongs to another plan. */
    private async follow(id: string): Promise<void> {
        await this.saving;
        const record = await this.library!.item(id).catch(() => null);
        if (this.closed || this.plan.id !== id || record === null) return;
        if (!record) this.install(newPlan(emptyTrip(this.trip.mode)), false);
        else if (record.name !== this.plan.name) this.plan = { ...this.plan, name: record.name };
    }

    private openNew(plan: Plan, offers = false): Promise<boolean> {
        return this.act(async library => {
            const offer = offers ? await this.offered(library) : null;
            await library.setState('active', null);
            this.install(offer ? { ...plan, id: offer.id } : plan, false, undefined, offer);
        });
    }

    /** The draft of route `id`, or with no `id` the newest draft of a route that is not stored, other than the open
     * one: so each open of an empty plan offers the next one. */
    private async offered(library: Library, id?: string): Promise<Offer | null> {
        const draft = id ? await library.draft(id) : await library.newestUnstoredDraft(this.plan.id);
        return draft && isTrip(draft.trip) ? draft as Offer : null;
    }

    /** The open state as a record. With no calculated line, the summary is null, so no page shows the numbers of another
     * state; the line that arrives later writes it. */
    private snapshot(): Plan {
        return { ...this.plan, trip: this.trip, summary: this.line ? planSummary(this.trip, this.line) : null, updatedAt: Date.now() };
    }

    /** Writes the record that `make` gives, with the open step as the only clean step, and runs `stored` after the
     * write. */
    private store(make: () => Plan, stored: (saved: Plan) => void): Promise<boolean> {
        // A save takes the state of the moment the rider asks; a save asked during another one takes the state after it.
        const start = () => this.storeNow(make(), stored);
        // The count drops before the caller hears the end, so a save that the caller then asks starts at once.
        const run = (this.inFlight ? this.saving.then(start) : start()).finally(() => this.inFlight--);
        this.inFlight++;
        // Never rejects: each switch, discard and later save waits for it.
        this.saving = run.then(() => {}, () => {});
        return run;
    }

    private async storeNow(route: Plan, stored: (saved: Plan) => void): Promise<boolean> {
        const at = this.at;
        this.history = this.history.map(({ clean, before: _, ...step }, i) => i === at ? { ...step, clean: true, ...clean && { before: true } } : clean ? { ...step, before: true } : step);
        try {
            const saved = await this.write(async library => {
                const saved = await library.saveRoute(route);
                await library.setState('active', saved.id);
                return saved;
            });
            this.history = this.history.map(({ before: _, ...step }) => step);
            this.stored = true;
            stored(saved);
            // An edit during the save goes to the draft of the route that is now open.
            if (this.changed) {
                this.keepDraft();
                return true;
            }
            setOwnDraft(null);
            // The line arrived during the save.
            if (!saved.summary && this.line) this.writeSummary(planSummary(this.trip, this.line));
            return true;
        } catch {
            // The transaction deleted no draft.
            this.history = this.history.map(({ clean: _, before, ...step }) => before ? { ...step, clean: true } : step);
            // An Undo or a Redo during the save wrote or deleted the draft by the flags of then. The error of the save
            // stays: a draft write is no save.
            this.keepDraft(false);
            return false;
        }
    }

    /** Writes the open state to the draft of the route when it is changed, and deletes the own draft when it is not.
     * With `clears` false, its success keeps the error of an earlier write. */
    private keepDraft(clears = true): void {
        const id = this.plan.id;
        if (!this.library) return;
        if (this.changed) {
            setOwnDraft(id);
            void this.write(library => library.saveDraft({ id, updatedAt: Date.now(), trip: this.trip }), clears).catch(() => {});
        } else if (ownDraft() === id) {
            setOwnDraft(null);
            void this.write(library => library.removeDraft(id), clears).catch(() => {});
        }
    }

    /** Runs a write of the open route. Writes run in order. A failed write shows its error until a later write of the
     * same route succeeds. */
    private write<T>(work: (library: Library) => Promise<T>, clears = true): Promise<T> {
        const id = this.plan.id;
        if (!this.library) {
            this.saveError = 'Browser storage is unavailable. Download your plan to keep it.';
            return Promise.reject(new Error(this.saveError));
        }
        const result = work(this.library);
        this.lastWrite = result;
        result.then(() => { if (clears && this.plan.id === id) this.saveError = ''; },
            error => { if (this.plan.id === id) this.saveError = storageError(error); });
        return result;
    }

    private step(next: Step, line?: RoutingLine): void {
        this.earlier = this.offer?.trip ?? null;
        this.offer = null;
        const kept = this.history.slice(0, this.at + 1).slice(-historyLimit).map(({ clean, ...step }) => next.clean || !clean ? step : { ...step, clean });
        if (line) this.remember(routingKey(next.trip), line);
        this.cancelPreview();
        this.show([...kept, { ...next, trip: guessedTrip(next.trip, this.lines.get(routingKey(next.trip))) }], kept.length);
        this.keepDraft();
    }

    /** Runs a library action; its error shows in `libraryError`. True when it completed and was not cancelled. */
    private async act(work: (library: Library) => Promise<boolean | void>): Promise<boolean> {
        this.busy = true;
        this.libraryError = '';
        try {
            if (!this.library) throw new Error('Browser storage is unavailable. Download your plan to keep it.');
            await this.saving;
            return await work(this.library) !== false;
        } catch (error) {
            this.libraryError = storageError(error);
            return false;
        } finally { this.busy = false; }
    }

    /** Stores a route that the rider adds, such as an import or a copy, and opens it. */
    private create(make: () => Plan | Promise<Plan>): Promise<boolean> {
        return this.act(async library => {
            const plan = await make();
            const saved = await library.saveRoute(plan);
            await library.setState('active', saved.id);
            this.install(saved, true);
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

    /** Opens `plan` as one clean step, and `draft` as the Undo step after it. The tab then writes the draft of `plan`
     * only. */
    private install(plan: Plan, stored: boolean, draft?: Trip, offer: Offer | null = null): void {
        this.cancelPreview();
        this.plan = plan;
        this.stored = stored;
        this.offer = offer;
        this.earlier = null;
        this.last = null;
        this.options = null;
        this.saveError = '';
        setOwnDraft(draft ? plan.id : null);
        this.show([{ trip: plan.trip, clean: true }, ...draft ? [{ trip: draft }] : []], draft ? 1 : 0);
        this.opened(plan);
    }

    private travel(at: number): true {
        this.earlier = null;
        this.cancelPreview();
        this.show(this.history, at);
        this.keepDraft();
        return true;
    }

    private show(history: Step[], at: number): void {
        this.history = history;
        this.at = at;
        const trip = this.trip, line = this.line;
        this.last = line ? { trip, line } : hasEndpoints(trip) ? this.last : null;
        this.route();
    }

    private writeSummary(summary: RouteSummary): void {
        const { id, trip } = this.plan;
        this.plan = { ...this.plan, summary };
        void this.write(library => library.setSummary(id, summary, trip)).catch(() => {});
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
            // A clean step stays clean: the guessed days and the visit names are calculated again at each open. A stored
            // route that lacks its summary gets only the summary.
            if (this.changed) this.keepDraft();
            else if (this.stored && !this.plan.summary) this.writeSummary(planSummary(this.trip, line));
        }, error => {
            if (!request.abort.signal.aborted) this.routeError = error instanceof Error ? error.message : 'Routing is unavailable.';
        });
    }
}
