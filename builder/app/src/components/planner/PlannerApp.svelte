<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import PlannerMap from './PlannerMap.svelte';
    import Icon from './PlannerIcon.svelte';
    import Profile from './PlannerProfile.svelte';
    import Query from './PlannerQuery.svelte';
    import Resize from './PanelResize.svelte';
    import TripBar from './TripBar.svelte';
    import PlanLine from './PlanLine.svelte';
    import Segmented from './Segmented.svelte';
    import Itinerary from './Itinerary.svelte';
    import RouteList from './RouteList.svelte';
    import RouteStats from './RouteStats.svelte';
    import WaysList from './WaysList.svelte';
    import QueryResults from './QueryResults.svelte';
    import MapCallout, { type CalloutKind, type EditableKind } from './MapCallout.svelte';
    import LayerMenu from './LayerMenu.svelte';
    import type { OverlayOptions } from '../../lib/planner/route-overlays';
    import NearbyLandmark from './NearbyLandmark.svelte';
    import { presetName } from '../../lib/planner/riding-profiles';
    import { isTrip } from '../../lib/planner/trip-validation';
    import {
        addClickedPoint, addPointNear, addRestDay, applyBudget, coordinateAt, cumulative, emptyTrip,
        insertPoint, itineraryDays, kilometres, nearestProgress, nightOrderConflicts, overnightCandidates,
        overnightWindow, pinNight, removeRestDay, reorderPoint, routeCoordinates, routeSlice, routeStops,
        setDrawnLeg, setLegMode, setSplit, setEndpoint, removeRoutePoint, TripHistory, tripDays, routingKey, planOf, storedPlan,
        type Coordinate, type Day, type LegMode, type Place, type PointKind, type RoutePoint, type Trip,
    } from '../../lib/planner/editor';
    import { categoryIds, type PlaceCategory } from '../../lib/planner/poi-kinds';
    import { corridorPlaces, routeDistance } from '../../lib/planner/place-index';
    import { landmarks } from '../../lib/planner/landmarks';
    import { MAP_BOUNDS, PLACES_URL } from '../../lib/planner/map-data';
    import { coordinateName, visitName } from '../../lib/planner/point-names';
    import { SEARCH_URL, HOSTED_SEARCH, SEARCH_REGIONS } from '../../lib/planner/search/config';
    import { dayColor } from '../../lib/planner/day-colors';
    import { profileAscent } from '../../lib/planner/profile-data';
    import { searchPlaces, type SearchState, type SearchContext, type Where } from '../../lib/planner/search/types';
    import { asPlace } from '../../lib/planner/search/presentation';
    import { buildQueryRoute } from '../../lib/planner/search/route-client';
    import { applyQueryChanges } from '../../lib/planner/search/actions';
    import type { MapPoint, MapSegment } from '../../lib/planner/map-types';
    import type { Version } from '../../lib/planner/versions';

    import { routePreview } from '../../lib/planner/route-preview';

    const storageKey = 'obc-planner-routing-v2';
    const siteBase = import.meta.env.VITE_SITE_BASE || '/';
    import { calculateLine, requestAlternatives, selectRoute, type EngineRoute } from '../../lib/planner/routing';
    import { LegCache } from '../../lib/planner/route-legs';
    const defaultLabels: Record<EditableKind, string> = {
        via: 'Shaping point',
        pass: 'Pass here',
        waypoint: 'Visit',
        night: 'Overnight spot',
        marker: 'Marker',
    };

    // Raw state: a change replaces the trip and never edits it in place, and a route has tens of thousands of points.
    let trip = $state.raw<Trip>(emptyTrip());
    const legs = new LegCache();
    let mounted = false;
    const hasEndpoints = $derived(trip.points.some(p => p.kind === 'start') && trip.points.some(p => p.kind === 'finish'));
    const nextEndpoint = $derived(trip.points.some(p => p.kind === 'start') ? 'finish' : 'start');
    let previewTrip = $state.raw<Trip | null>(null);
    let draggingPoint = $state(false);
    let previewStatus = $state('');
    const shownTrip = $derived(previewTrip ?? trip);
    const preview = routePreview((draft: Trip, signal: AbortSignal) => calculateLine(draft, signal, legs), (draft, line) => {
        previewTrip = { ...draft, routing: line };
        previewStatus = 'Route preview · release to keep';
    }, error => { previewStatus = error instanceof Error ? error.message : 'Preview unavailable.'; });
    let routingStatus = $state('Choose a start and finish');
    let routeAttempt = $state(0);
    const routingInput = $derived(routingKey(trip));
    // Undo returns a plan without its route, often with an unchanged routing key.
    const routed = $derived(trip.routing?.key === routingInput);
    $effect(() => {
        const key = routingInput;
        void routeAttempt;
        if (!hasEndpoints) { routingStatus = 'Choose a start and finish'; return; }
        if (routed) return;
        const plan = untrack(() => trip);
        const abort = new AbortController();
        routingStatus = 'Calculating route…';
        calculateLine(plan, abort.signal, legs).then(line => {
                if (abort.signal.aborted || key !== routingInput) return;
                trip = { ...trip, routing: line };
            }).catch(error => { if (!abort.signal.aborted) routingStatus = error instanceof Error ? error.message : 'Routing is unavailable.'; });
        return () => abort.abort();
    });
    function pickRoute(route: EngineRoute) {
        const next = { ...trip, preset: presetName(route.profile) };
        next.routing = selectRoute(next, route, trip.routing?.alternatives ?? [route]);
        commit(next, 'Route preference changed');
    }
    const currentRoute = $derived(hasEndpoints && shownTrip.routing?.key === routingKey(shownTrip) ? shownTrip.routing : undefined);
    const routingMessage = $derived(draggingPoint ? previewStatus : currentRoute
        ? `${currentRoute.unknownSurfaceKm.toFixed(1)} km unknown surface${currentRoute.pushingKm ? ` · ${currentRoute.pushingKm.toFixed(1)} km pushing` : ''}${currentRoute.unroutedKm ? ` · ${currentRoute.unroutedKm.toFixed(1)} km manual / access unverified` : ''}${currentRoute.elevation.some(h => h === null) ? ' · elevation incomplete' : ''}`
        : routingStatus);
    let list = $state<'plan' | 'ways'>('plan');
    let waysStatus = $state('');
    const needsAlternatives = $derived(trip.routing?.key === routingInput && !trip.routing.alternativesReady);
    $effect(() => {
        const key = routingInput;
        if (list !== 'ways' || draggingPoint || !needsAlternatives) return;
        const plan = untrack(() => trip);
        const abort = new AbortController();
        waysStatus = 'Finding alternative routes…';
        requestAlternatives(plan, plan.routing!, abort.signal).then(alternatives => {
            if (abort.signal.aborted || key !== routingInput) return;
            const routing = { ...trip.routing!, alternatives, alternativesReady: true };
            trip = { ...trip, routing };
            waysStatus = '';
        }).catch(error => { if (!abort.signal.aborted) waysStatus = error instanceof Error ? error.message : 'Alternatives unavailable.'; });
        return () => abort.abort();
    });
    let searching = $state(false);
    let planEditing = $state(false);
    // Riding numbers stay stable when the itinerary includes rest days.
    let night = $state(1);
    let expandedDay = $state<number | null>(null);
    let changingOvernight = $state(false);
    let selectedId = $state<string | null>(null);
    // The point whose panel row lights up for a moment after it was added or pinned.
    let revealId = $state<string | null>(null);
    let revealTimer: ReturnType<typeof setTimeout> | undefined;
    // A spot picked on the map, and the point it replaces when a point becomes an overnight.
    let pending = $state<Coordinate | null>(null);
    let pendingSource = $state<string | null>(null);
    let picking = $state(false);
    // Where an add or leg callout opened; a leg callout also names its leg.
    let spot = $state<{ coordinate: Coordinate; legEndId?: string } | null>(null);
    let drawing = $state<string | null>(null);
    // The selected place when it is not a fixture: a basemap place or a landmark.
    let mapPlace = $state<Place | null>(null);
    let hiddenCategories = $state<PlaceCategory[]>([]);
    let highlightedCategories = $state<PlaceCategory[]>([]);
    let corridor = $state<Place[]>([]);
    let corridorLoad = $state<'done' | 'loading' | 'failed'>('done');
    let theme = $state<'light' | 'dark'>('light');
    let autoCenter = $state(false);
    let hoveredId = $state<string | null>(null);
    let hillshade = $state(true);
    let contours = $state(true);
    let mapOverlays = $state<OverlayOptions>({ network: 'cycling', access: true });
    let showRoute = $state(true);
    let query = $state('');
    let searchState = $state<SearchState>({ loading: false, error: '', answer: null });
    let searchBox: Query | undefined;
    let searchRevision = $state(0);
    let viewBounds = $state<[number, number, number, number]>(MAP_BOUNDS ?? [7.77,47.965,7.96,48.06]);
    let here = $state<Coordinate | undefined>();
    let pointing = $state<Where | undefined>();
    let applyingQuery = $state(false);
    let queryApplyError = $state('');
    let searchRegion = $state(SEARCH_REGIONS[0]);
    let overnightPlaces = $state<Place[]>([]);
    let overnightNote = $state('');
    let message = $state('Plan a ride in Baden-Württemberg');
    // The status line offers Undo after a version restore, until the next change.
    let undoable = $state(false);
    let draftSavedAt = $state<number | null>(null);
    let draftError = $state('');
    let visibleRange = $state<[number, number] | null>(null);
    let hoverProgress = $state<number | null>(null);
    let sideWidth = $state(360);
    let profileHeight = $state(260);
    let viewportHeight = $state(900);
    let viewportWidth = $state(1200);
    let mapHeight = $state(600);
    let map: PlannerMap | undefined;
    const history = new TripHistory();
    let revision = $state(0);

    const maxProfile = $derived(Math.max(210, Math.min(340, viewportHeight - 400)));
    const maxSide = $derived(Math.max(320, Math.min(460, viewportWidth - 540)));
    const canUndo = $derived.by(() => { void revision; return history.canUndo; });
    const canRedo = $derived.by(() => { void revision; return history.canRedo; });
    const coordinates = $derived(routeCoordinates(shownTrip));
    const lengths = $derived(cumulative(coordinates));
    const total = $derived(lengths.at(-1)!);
    const stops = $derived(routeStops(shownTrip));
    const days = $derived(tripDays(shownTrip));
    const multi = $derived(trip.mode !== 'route');
    const itinerary = $derived(itineraryDays(trip));
    const focusedDay = $derived(multi && list === 'plan' && !searching ? itinerary.find(d => !d.rest && d.ridingNumber === expandedDay) ?? null : null);
    const dayLabels = $derived(Object.fromEntries(itinerary.filter(d => !d.rest).map(d => [d.ridingNumber, d.number])));
    const searchPlan = $derived<SearchContext['plan']>({ coordinates, km: lengths, seconds: currentRoute?.unroutedKm === 0 ? currentRoute.elapsed : undefined,
        days: itinerary.map(d => ({ number: d.number, from: d.from * total, to: d.to * total, rest: d.rest })),
        points: trip.points.map(p => ({ id: p.id, label: p.label, coordinate: p.coordinate, kind: p.kind, placeKind: p.placeKind })) });
    const searchContext = $derived<SearchContext>({ view: viewBounds, here, pointing, startDate: trip.startDate, plan: searchPlan });
    const conflicts = $derived(multi ? nightOrderConflicts(trip) : []);
    const activeDay = $derived(days[Math.min(night - 1, days.length - 1)]);
    const area = $derived(night < days.length ? overnightWindow(trip, night) : null);
    const overnightContext = $derived(
        multi && !!currentRoute && expandedDay !== null && !searching && list === 'plan' && night < days.length && (!activeDay?.pinned || changingOvernight),
    );
    const highlighted = $derived(overnightContext && area && !area.blocked ? routeSlice(coordinates, area.from, area.to) : []);
    const candidates = $derived(multi ? overnightCandidates(trip, night, overnightPlaces) : []);
    // One stretch per leg and day, so the map can tell legs apart and colour days.
    const segments = $derived.by(() => {
        const length = stops.at(-1)?.distance || 1;
        return stops.slice(1).flatMap((stop, i) => days.flatMap((day): MapSegment[] => {
            const from = Math.max(day.from, stops[i].distance / length);
            const to = Math.min(day.to, stop.distance / length);
            if (to <= from) return [];
            return [{ coordinates: routeSlice(coordinates, from, to), color: dayColor(day.number, theme), legEndId: stop.point.id, leg: stop.point.leg ?? 'routed' }];
        }));
    });
    const results = $derived((searchState.answer?.results ?? []).map(result => ({ place: asPlace(result) })));
    const visiblePlaces = $derived(searching ? results.map(result => result.place) : overnightContext ? candidates.map(candidate => candidate.place) : []);
    const highlights = $derived(highlightedCategories);
    const highlightLimit = 300;
    // Basemap places within 5 km of the route, nearest first.
    const highlightCandidates = $derived.by(() => {
        if (!highlights.length || coordinates.length < 2) return [];
        const pinned = new Set(visiblePlaces.map(place => place.id));
        const distance = routeDistance(coordinates, 5);
        const nearby = corridor
            .filter(place => highlights.includes(place.category))
            .map(place => ({ place, off: distance(place.coordinate) }))
            .filter(({ off }) => Number.isFinite(off))
            .sort((a, b) => a.off - b.off)
            .map(({ place }) => place);
        return nearby.filter(place => !pinned.has(place.id));
    });
    const highlightedPlaces = $derived(highlightCandidates.slice(0, highlightLimit));
    const placeNote = $derived(
        !highlights.length ? ''
        : corridorLoad === 'loading' ? 'Loading places along the route…'
        : corridorLoad === 'failed' ? 'Places along the route could not load'
        : highlightCandidates.length > highlightLimit ? `Showing ${highlightLimit} of ${highlightCandidates.length} highlighted places, nearest the route first`
        : '',
    );
    // The next landmark along the route within 15 km that the route does not visit yet.
    const nearbyLandmark = $derived.by(() => {
        const distance = routeDistance(coordinates, 15);
        return landmarks
            .filter(landmark => Number.isFinite(distance(landmark.coordinate)) && !trip.points.some(p => kilometres(p.coordinate, landmark.coordinate) < .3))
            .sort((a, b) => nearestProgress(coordinates, a.coordinate) - nearestProgress(coordinates, b.coordinate))[0];
    });
    const selectedPlace = $derived(visiblePlaces.find(p => p.id === selectedId) ?? corridor.find(p => p.id === selectedId) ?? (mapPlace?.id === selectedId ? mapPlace : undefined));
    const selectedPoint = $derived(trip.points.find(p => p.id === selectedId));
    const previewCoordinate = $derived(selectedId === 'pending' ? pending : selectedPlace?.coordinate ?? null);
    const mapPoints = $derived.by(() => {
        const pins: MapPoint[] = trip.points.map(p => ({
            ...p,
            kind: !multi && p.kind === 'night' ? 'waypoint' : p.kind,
            color: multi && p.kind === 'night' ? dayColor(p.night!, theme) : undefined,
            markerLabel: multi && p.kind === 'night' ? String(dayLabels[p.night!]) : undefined,
            fixed: p.kind === 'night',
        }));
        const shown = selectedPlace && !visiblePlaces.includes(selectedPlace) ? [...visiblePlaces, selectedPlace] : visiblePlaces;
        for (const p of shown) {
            if (p.id === selectedId || !pins.some(pin => pin.coordinate[0] === p.coordinate[0] && pin.coordinate[1] === p.coordinate[1])) pins.push({ ...p });
        }
        if (pending) pins.push({ id: 'pending', coordinate: pending, label: 'Overnight spot', kind: 'place', appearance: 'suggested' });
        // Day ends come last, so they stay on top of a candidate place at the same spot and can be dragged.
        for (const day of multi && currentRoute ? days.slice(0, -1) : []) {
            if (day.pinned) continue;
            const label = dayLabels[day.number] ?? day.number;
            pins.push({
                id: `dayend-${day.number}`, kind: 'dayend', night: day.number, coordinate: coordinateAt(coordinates, day.to),
                label: `Day ${label} ends here for now`, markerLabel: String(label), color: dayColor(day.number, theme),
                appearance: day.split ? 'moved' : undefined,
            });
        }
        return pins;
    });
    const calloutKind = $derived<CalloutKind | null>(
        selectedId === 'add' || selectedId === 'leg' ? selectedId
        : selectedId?.startsWith('dayend-') ? 'dayend'
        : selectedPoint ? 'point'
        : selectedPlace || selectedId === 'pending' ? 'place'
        : null,
    );
    const calloutCoordinate = $derived.by(() => {
        if (calloutKind === 'add' || calloutKind === 'leg') return spot?.coordinate ?? null;
        if (calloutKind === 'place') return previewCoordinate;
        if (!showRoute && selectedPoint?.kind !== 'marker') return null;
        return mapPoints.find(p => p.id === selectedId)?.coordinate ?? null;
    });
    const legMode = $derived(trip.points.find(p => p.id === spot?.legEndId)?.leg ?? 'routed');
    // The map reports the vertices it shows; one more on each side covers the stretch that runs off screen.
    const profileWindow = $derived.by(() => {
        if (!visibleRange || total <= 0) return { from: 0, to: 1 };
        const last = lengths.length - 1;
        const from = lengths[Math.max(0, Math.min(last, visibleRange[0] - 1))] / total;
        const to = lengths[Math.max(0, Math.min(last, visibleRange[1] + 1))] / total;
        return to > from ? { from, to } : { from: 0, to: 1 };
    });

    $effect(() => {
        if (!highlights.length || coordinates.length < 2) return;
        const route = coordinates;
        let current = true;
        corridorLoad = 'loading';
        corridorPlaces(PLACES_URL, route).then(
            found => { if (current) { corridor = found; corridorLoad = 'done'; } },
            () => { if (current) corridorLoad = 'failed'; },
        );
        return () => { current = false; };
    });

    // A query runs again when the map view settles or when anything else it sends changes.
    $effect(() => { void [searchPlan, here, pointing, trip.startDate]; untrack(() => searchRevision++); });

    $effect(() => {
        const plan = searchPlan, region = searchRegion, day = dayLabels[night];
        overnightPlaces = [];
        if (!overnightContext || !day) { overnightNote = ''; return; }
        const abort = new AbortController();
        overnightNote = 'Loading nearby overnight places…';
        // The day end and the radius set the area, so the map view does not change the answer.
        untrack(() => searchPlaces('sleep', { view: viewBounds, plan }, region, 6, abort.signal, { type: 'places', what: ['sleep'], where: { day, part: 'end' }, radius: { value: 5, unit: 'km' } })).then(answer => {
            if (abort.signal.aborted) return;
            overnightPlaces = (answer.results ?? []).map(asPlace);
            overnightNote = answer.type === 'unresolved' ? answer.note ?? '' : overnightPlaces.length ? '' : 'No mapped overnight places within 5 km. Search a wider area or pick on the map.';
        }).catch(() => { if (!abort.signal.aborted) overnightNote = 'Overnight search is unavailable. Retry or pick on the map.'; });
        return () => abort.abort();
    });

    onMount(() => {
        mounted = true;
        try {
            const raw = localStorage.getItem(storageKey);
            const saved = raw ? JSON.parse(raw) : null;
            if (isTrip(saved)) trip = planOf(saved);
            else if (raw) draftError = 'Saved draft is invalid · new plan opened';
        } catch (error) {
            draftError = error instanceof SyntaxError ? 'Saved draft is invalid · new plan opened' : 'Draft · browser storage unavailable';
        }
        try { autoCenter = localStorage.getItem('obc-planner-auto-center') === 'true'; } catch { /* Optional browser preference. */ }
        trip.points.filter(point => point.autoLabel).forEach(point => void nameVisit(point));
        return () => { mounted = false; preview.cancel(); };
    });

    $effect(() => {
        document.documentElement.dataset.theme = theme;
    });

    $effect(() => {
        try { localStorage.setItem('obc-planner-auto-center', String(autoCenter)); } catch { /* Optional browser preference. */ }
    });

    function save() {
        try {
            localStorage.setItem(storageKey, JSON.stringify(storedPlan(trip)));
            draftSavedAt = Date.now();
            draftError = '';
        } catch {
            draftError = 'Draft · could not save locally';
        }
    }

    function commit(next: Trip, description: string) {
        preview.cancel();
        previewTrip = null;
        draggingPoint = false;
        trip = history.commit(trip, next);
        if (!hasEndpoints) { expandedDay = null; list = 'plan'; planEditing = false; hoverProgress = null; }
        revision++;
        message = description;
        undoable = false;
        save();
    }

    function edit(change: Partial<Trip>, description: string) {
        commit({ ...trip, ...change }, description);
    }

    function clearSelection() {
        selectedId = null;
        pending = null;
        pendingSource = null;
        spot = null;
        picking = false;
    }

    function afterHistory(description: string) {
        preview.cancel();
        previewTrip = null;
        draggingPoint = false;
        revision++;
        clearSelection();
        night = Math.max(1, Math.min(night, tripDays(trip).length));
        if (expandedDay !== null) expandedDay = night;
        save();
        message = description;
        undoable = false;
        trip.points.filter(point => point.autoLabel).forEach(point => void nameVisit(point));
    }

    /** Opens the day a point lies in and lights its row for a moment. */
    function reveal(id: string, ridingDay: number | null) {
        if (ridingDay !== null && multi) {
            night = ridingDay;
            expandedDay = ridingDay;
            list = 'plan';
            changingOvernight = false;
            exitSearch();
        }
        revealId = id;
        clearTimeout(revealTimer);
        revealTimer = setTimeout(() => revealId = null, 1200);
    }

    function dayOf(coordinate: Coordinate): number | null {
        const at = nearestProgress(coordinates, coordinate);
        return (days.find(d => at <= d.to) ?? days.at(-1))?.number ?? null;
    }

    function undo() {
        trip = history.undo(trip);
        afterHistory('Change undone');
    }

    function redo() {
        trip = history.redo(trip);
        afterHistory('Change restored');
    }

    function exitSearch() {
        if (!searching) return;
        searching = false;
        query = '';
    }

    function clearSearch() {
        if (!searching) return;
        searching = false;
        if (multi && list === 'plan' && expandedDay !== null) showDay(expandedDay);
        else { clearSelection(); map?.fitRoute(); }
    }

    /** Opens a riding day and fits its route on the map. */
    function showDay(riding: number) {
        night = riding;
        expandedDay = riding;
        list = 'plan';
        changingOvernight = false;
        exitSearch();
        clearSelection();
        const day = days[riding - 1];
        map?.fitCoordinates(routeSlice(coordinates, Math.max(0, day.from - .04), Math.min(1, day.to + .04)));
    }

    function showAllDays() {
        expandedDay = null;
        changingOvernight = false;
        clearSelection();
        map?.fitRoute();
    }

    function showDayEnd(day: Day) {
        if (day.pinned) inspectPoint(day.pinned);
        else if (day.number === days.length) inspectPoint(trip.points.find(p => p.kind === 'finish')!);
        else map?.fitCoordinates(routeSlice(coordinates, Math.max(0, (area?.from ?? day.to) - .05), Math.min(1, (area?.to ?? day.to) + .05)));
    }

    function selectPlace(place: Place) {
        choosePlace(place);
        map?.showPlace(place.coordinate, 12);
    }

    /** Opens a mapped place without changing the search area. */
    function choosePlace(place: Place) {
        clearSelection();
        mapPlace = place;
        selectedId = place.id;
    }

    function selectPoint(id: string) {
        if (id === 'pending') return;
        const place = visiblePlaces.find(p => p.id === id) ?? corridor.find(p => p.id === id) ?? (mapPlace?.id === id ? mapPlace : undefined);
        if (place) { choosePlace(place); return; }
        clearSelection();
        selectedId = id;
        const number = id.startsWith('dayend-') ? Number(id.slice('dayend-'.length)) : trip.points.find(p => p.id === id)?.night;
        if (number && multi) {
            night = number;
            expandedDay = number;
            list = 'plan';
            changingOvernight = false;
            exitSearch();
        }
    }

    function inspectPoint(point: RoutePoint) {
        selectPoint(point.id);
        map?.showPlace(point.coordinate, 12);
    }

    function pickOnMap() {
        clearSelection();
        picking = true;
    }

    function emptyClick(coordinate: Coordinate) {
        if (picking) {
            picking = false;
            pending = coordinate;
            selectedId = 'pending';
        } else if (selectedId) {
            clearSelection();
        } else {
            spot = { coordinate };
            selectedId = 'add';
        }
    }

    function addHere(kind: EditableKind) {
        const coordinate = spot!.coordinate;
        if (kind !== 'night') {
            addPoint(coordinate, kind);
            return;
        }
        clearSelection();
        pending = coordinate;
        selectedId = 'pending';
    }

    function legClick(legEndId: string, coordinate: Coordinate) {
        clearSelection();
        spot = { coordinate, legEndId };
        selectedId = 'leg';
    }

    function setLeg(mode: LegMode) {
        const legEndId = spot!.legEndId!;
        if (mode === 'drawn') {
            clearSelection();
            drawing = legEndId;
            return;
        }
        commit(setLegMode(trip, legEndId, mode), mode === 'straight' ? 'Leg set to a straight line' : 'Leg set to routed');
    }

    function insert(legEndId: string, coordinate: Coordinate) {
        clearSelection();
        commit(insertPoint(trip, legEndId, coordinate), 'Shaping point inserted');
        if (autoCenter) map?.centerOn(coordinate);
    }

    function drawn(legEndId: string, line: Coordinate[]) {
        drawing = null;
        commit(setDrawnLeg(trip, legEndId, line), 'Leg drawn');
    }

    function moveDayEnd(number: number, progress: number) {
        commit(setSplit(trip, number, progress), 'Day end moved');
    }

    function stayHere(sleepDay: number) {
        if (!previewCoordinate) return;
        const coordinate: Coordinate = [...previewCoordinate];
        const source = trip.points.find(p => p.id === pendingSource);
        const label = selectedPlace?.label ?? (source && !['via', 'pass'].includes(source.kind) ? source.label : 'Overnight spot');
        const current = trip;
        const next = pinNight(current, sleepDay, coordinate, label, pendingSource ?? undefined);
        if (next === current) return;
        next.points.find(p => p.night === sleepDay)!.placeKind = selectedPlace?.placeKind ?? source?.placeKind;
        commit(next, 'Overnight pinned');
        clearSelection();
        reveal(`night-${sleepDay}`, sleepDay);
        if (autoCenter) map?.centerOn(coordinate);
    }

    function newPoint(coordinate: Coordinate, kind: EditableKind, label?: string): RoutePoint {
        const autoLabel = kind === 'waypoint' && label === undefined;
        return { id: crypto.randomUUID(), coordinate: [...coordinate], label: label ?? (autoLabel ? coordinateName(coordinate) : defaultLabels[kind]),
            autoLabel: autoLabel || undefined, kind, progress: nearestProgress(coordinates, coordinate) };
    }

    async function nameVisit(point: RoutePoint) {
        if (!point.autoLabel || !['waypoint', 'detour'].includes(point.kind) || point.label !== coordinateName(point.coordinate)) return;
        const coordinate: Coordinate = [...point.coordinate];
        const label = await visitName(coordinate, searchRegion);
        if (!label) return;
        const current = trip.points.find(p => p.id === point.id);
        if (!mounted || !current?.autoLabel || current.coordinate[0] !== coordinate[0] || current.coordinate[1] !== coordinate[1]) return;
        // Generated names are metadata, so a lookup does not add an Undo step.
        trip = { ...trip, points: trip.points.map(p => p.id === point.id ? { ...p, label } : p) };
        save();
    }

    const added: Partial<Record<PointKind, string>> = { via: 'Shaping point added', waypoint: 'Visit added', night: 'Overnight pinned', marker: 'Marker added', pass: 'Pass added' };

    function addPoint(coordinate: Coordinate, kind: EditableKind) {
        const point = newPoint(coordinate, kind);
        commit(addClickedPoint(trip, point), added[kind]!);
        clearSelection();
        selectedId = point.id;
        if (kind !== 'via') reveal(point.id, dayOf(point.coordinate));
        if (autoCenter) map?.centerOn(point.coordinate);
        void nameVisit(point);
    }

    function addVisit(place: Place) {
        const point = newPoint(place.coordinate, 'waypoint', place.label);
        commit(addPointNear(trip, point), 'Visit added');
        clearSelection();
        selectedId = point.id;
        reveal(point.id, dayOf(point.coordinate));
        if (autoCenter) map?.centerOn(point.coordinate);
    }

    function movedPoint(id: string, coordinate: Coordinate): Trip {
        const next = { ...trip };
        const point = next.points.find(p => p.id === id);
        if (point?.kind === 'night') return pinNight(next, point.night!, coordinate, point.label);
        next.points = next.points.map(p => p.id === id
            ? { ...p, coordinate, label: p.autoLabel ? coordinateName(coordinate) : p.label,
                progress: p.kind === 'start' || p.kind === 'finish' ? p.progress : nearestProgress(coordinates, coordinate) }
            : p);
        return next;
    }

    function previewPoint(id: string, coordinate: Coordinate) {
        if (!hasEndpoints) return;
        draggingPoint = true;
        previewStatus = 'Hold still to preview the route';
        preview.move(movedPoint(id, coordinate));
    }

    function movePoint(id: string, coordinate: Coordinate) {
        const next = movedPoint(id, coordinate);
        if (previewTrip?.routing?.key === routingKey(next)) next.routing = previewTrip.routing;
        commit(next, 'Point moved');
        const moved = next.points.find(p => p.id === id);
        if (moved) void nameVisit(moved);
    }

    function removePoint() {
        if (!selectedPoint) return;
        commit(removeRoutePoint(trip, selectedPoint.id), 'Point removed');
        pointing = undefined;
        clearSelection();
    }

    function chooseEndpoint(kind: 'start' | 'finish') {
        const coordinate = spot?.coordinate ?? selectedPlace?.coordinate;
        if (!coordinate) return;
        commit(setEndpoint(trip, kind, coordinate, selectedPlace?.label), kind === 'start' ? 'Start set' : 'Finish set');
        showRoute = true;
        clearSelection();
        exitSearch();
        list = 'plan';
        if (autoCenter) map?.centerOn(coordinate);
    }

    function newPlan() {
        commit({ ...emptyTrip(trip.mode), bike: trip.bike, preset: trip.preset }, 'New plan · Undo to restore');
        clearSelection();
        exitSearch();
        drawing = null;
        pointing = undefined;
        showRoute = true;
    }

    function rename(label: string) {
        if (!selectedPoint) return;
        const next = { ...trip };
        next.points = next.points.map(p => p.id === selectedId ? { ...p, label, autoLabel: undefined } : p);
        commit(next, 'Point renamed');
    }

    function changeKind(kind: EditableKind | 'detour') {
        const point = selectedPoint;
        if (!point) return;
        if (kind === 'night') {
            pendingSource = point.id;
            pending = [...point.coordinate];
            selectedId = 'pending';
            return;
        }
        // A night point takes a fresh id, so its `night-N` id stays free for pinning.
        const id = point.kind === 'night' ? crypto.randomUUID() : point.id;
        const autoLabel = ['waypoint', 'detour'].includes(kind) && (point.kind === 'via' || point.autoLabel);
        const label = autoLabel ? coordinateName(point.coordinate) : point.label;
        const next = { ...trip };
        next.points = next.points.map(p => p.id === point.id ? { ...p, id, kind, night: undefined, label, autoLabel: autoLabel || undefined,
            anchor: kind === 'detour' ? coordinateAt(coordinates, nearestProgress(coordinates, point.coordinate)) : undefined } : p);
        next.routeOrder = next.routeOrder?.map(old => old === point.id ? id : old);
        commit(next, 'Point type updated');
        selectedId = id;
        void nameVisit(next.points.find(p => p.id === id)!);
    }

    function changeTrip(change: Partial<Trip>, description: string) {
        edit(change, description);
        if (!('mode' in change)) return;
        night = 1;
        expandedDay = null;
        list = 'plan';
        planEditing = false;
        exitSearch();
        clearSelection();
    }

    function applyPlan(budget: Trip['budget'], target: number, limit: number, climb: number) {
        commit({ ...applyBudget(trip, budget, target, limit), climbTarget: climb || undefined }, 'Day plan updated');
        night = Math.min(night, trip.days);
        if (expandedDay !== null) expandedDay = night;
    }

    function locate() {
        if (!navigator.geolocation) { message = 'This browser cannot read your location.'; return; }
        navigator.geolocation.getCurrentPosition(position => {
            here = [position.coords.longitude, position.coords.latitude];
            message = 'Location set for search';
        }, () => { message = 'Location unavailable. Allow location access or use a named place.'; }, { timeout: 10000 });
    }

    async function loadSearchSample() {
        try {
            const response = await fetch(`${SEARCH_URL}/sample`);
            if (!response.ok) throw new Error('The example route is unavailable. Retry shortly.');
            const { coordinates: line } = await response.json() as { coordinates: Coordinate[] };
            if (line.length < 2) throw new Error('The sample route could not load.');
            commit({ ...emptyTrip('trip'), routeOrder: [], points: [
                { id: 'start', label: 'Black Forest start', kind: 'start', coordinate: line[0], progress: 0 },
                { id: 'finish', label: 'Black Forest finish', kind: 'finish', coordinate: line.at(-1)!, progress: 1, leg: 'drawn', drawn: line.slice(1,-1) },
            ] }, 'Black Forest test route loaded · three provisional days');
            exitSearch(); clearSelection(); pointing = undefined; map?.fitCoordinates(line);
        } catch (error) { message = (error as Error).message; }
    }

    async function applySearch() {
        const answer = $state.snapshot(searchState.answer);
        if (!answer?.changes || applyingQuery) return;
        const before = trip;
        applyingQuery = true; queryApplyError = '';
        try {
            let routingNote = '';
            const next = await applyQueryChanges(before, answer.changes, (points,bike,goal) => buildQueryRoute(points,bike,goal,note => routingNote = note), next => calculateLine(next, new AbortController().signal, legs));
            if (JSON.stringify(answer.changes) !== JSON.stringify(searchState.answer?.changes) || before !== trip) throw new Error('The plan changed. Review the search again.');
            commit(next, [answer.description ?? 'Query applied', routingNote].filter(Boolean).join(' · '));
            exitSearch(); clearSelection();
        } catch (error) { queryApplyError = (error as Error).message; }
        finally { applyingQuery = false; }
    }

    function nameRest(index: number, name: string) {
        const names = (trip.restAfter ?? []).map((_, i) => i === index ? name.trim() : trip.restNames?.[i] ?? '');
        edit({ restNames: names }, 'Rest day named');
    }

    function restoreVersion(saved: Trip, name: string) {
        commit(planOf(saved), `Restored ‘${name}’`);
        undoable = true;
        clearSelection();
        night = Math.max(1, Math.min(night, tripDays(trip).length));
        if (expandedDay !== null) expandedDay = night;
    }

    function keyboard(event: KeyboardEvent) {
        if ((event.target as HTMLElement)?.closest('input, select, textarea')) return;
        if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'z') {
            event.preventDefault();
            if (event.shiftKey) redo();
            else undo();
        }
        if (event.key === 'Escape') {
            if (!selectedId && !picking && !drawing && focusedDay) showAllDays();
            clearSelection();
            drawing = null;
        }
    }

</script>

<svelte:window onkeydown={keyboard} bind:innerHeight={viewportHeight} bind:innerWidth={viewportWidth} />

<div class="planner-shell" style:--side-width={`${Math.min(sideWidth, maxSide)}px`}>
    <header class="site-header">
        <a class="brand" href={siteBase}><img src={`${siteBase}brand/app-icon.svg`} alt="" /><span>OpenBikeComputer</span></a>
        <nav aria-label="Main navigation">
            {#if HOSTED_SEARCH}
                <a href={`${siteBase}docs/`}>Docs</a>
                <a href={`${siteBase}blog/`}>Blog</a>
                <a href={`${siteBase}builder/`}>Maps</a>
            {:else if import.meta.env.MODE !== 'planner'}<a href="/map-study.html">Map study</a>{/if}
            <span aria-current="page">Route planner</span>
        </nav>
        <button type="button" class="theme" aria-label={theme === 'light' ? 'Use dark theme' : 'Use light theme'} onclick={() => theme = theme === 'light' ? 'dark' : 'light'}>
            <Icon name={theme === 'light' ? 'moon' : 'sun'} />
        </button>
    </header>
    <TripBar
        {trip} {canUndo} {canRedo} {draftSavedAt} {draftError}
        onNew={newPlan} onChange={changeTrip} onUndo={undo} onRedo={redo} onRestore={restoreVersion}
        onSaved={(version: Version) => message = `Version saved · ${version.summary}`}
    />
    <main>
        <aside class="planner-pane" aria-label="Trip planning">
            {#if overnightContext && overnightNote}<p class="search-note" role="status">{overnightNote}</p>{/if}
            <Query bind:this={searchBox} bind:text={query} bind:region={searchRegion} bind:searchState={searchState} context={searchContext} selection={calloutCoordinate ? { anchor: calloutCoordinate } : undefined} revision={searchRevision} onResults={coordinates => { clearSelection(); map?.fitSearchResults(coordinates); }} onSearch={() => { searching = true; queryApplyError = ''; }} onClear={clearSearch} onLocation={locate} onPointing={where => pointing = where} onSample={loadSearchSample} onDate={date => edit({ startDate: date || undefined }, 'Trip date changed')} />
            {#if searching}
                <div class="pane-scroll">
                    <QueryResults state={searchState} {selectedId} onSelect={selectPlace} applying={applyingQuery} applyError={queryApplyError} onApply={applySearch} onMore={() => searchBox?.more()} onRetry={() => searchBox?.retry()} onStretch={line => { pointing = {along:{ref:'km',from:{value:nearestProgress(coordinates,line[0])*total,unit:'km'},to:{value:nearestProgress(coordinates,line.at(-1)!)*total,unit:'km'}}}; map?.fitCoordinates(line); }} />
                </div>
            {:else if !hasEndpoints}
                <div class="start-plan pane-scroll">
                    <Icon name="route" size={24} />
                    <h2>{stops.length ? `Choose your ${nextEndpoint}` : 'Where would you like to ride?'}</h2>
                    <p>{stops.length ? `Click another place on the map and choose “${nextEndpoint === 'start' ? 'Start' : 'Finish'} here”.` : 'Click a place on the map, or search above. Choose “Start here” or “Finish here”.'}</p>
                    {#each stops as { point } (point.id)}
                        <button type="button" class="chosen-endpoint" onclick={() => inspectPoint(point)}>
                            <Icon name="pin" size={18} /><span><small>{point.kind === 'start' ? 'Start' : 'Finish'}</small><strong>{point.label}</strong></span><Icon name="chevron" size={16} />
                        </button>
                    {/each}
                    <p class="next-step">{multi ? 'Choose both points to see the route and split it into riding days.' : 'Your route appears when you have a start and finish. Add stops along the way.'}</p>
                </div>
            {:else}
                {#if !currentRoute}
                    <div class="route-status" role="status">
                        <p>{routingStatus}</p>
                        {#if routingStatus !== 'Calculating route…'}
                            {#if canUndo}<button type="button" class="planner-action" onclick={undo}>Undo last change</button>{/if}
                            <button type="button" class="planner-action" onclick={() => routeAttempt++}>Retry routing</button>
                            <p>Move a point or choose another place in Baden-Württemberg.</p>
                        {/if}
                    </div>
                {/if}
                {#if !focusedDay && currentRoute}
                    <div class="trip-summary"><RouteStats distance={total} ascent={currentRoute?.elevation.every(h => h !== null) ? profileAscent(0, 1, currentRoute) : null} hours={currentRoute ? currentRoute.seconds / 3600 : null} /></div>
                {/if}
                {#if currentRoute && (!focusedDay || planEditing)}
                    <PlanLine {trip} dayCount={itinerary.length} bind:editing={planEditing} onApply={applyPlan} />
                {/if}
                {#if !focusedDay && currentRoute}
                    <div class="list-switch">
                        <Segmented compact label="List" value={list} onChange={(value) => list = value}
                            options={[{ value: 'plan', label: multi ? 'Days' : 'Route' }, { value: 'ways', label: currentRoute?.alternativesReady ? `Route options · ${currentRoute.alternatives.length}` : 'Route options' }]} />
                    </div>
                {/if}
                <div class="pane-scroll">
                    {#if list === 'ways'}
                        <WaysList status={needsAlternatives ? waysStatus || 'Open Route options to find alternatives.' : ''} routes={currentRoute?.alternatives ?? []} choiceId={currentRoute?.choiceId ?? ''} onPick={pickRoute} />
                    {:else if multi && currentRoute}
                        <Itinerary
                            {trip} {itinerary} {days} {theme} {expandedDay} {candidates} {conflicts} {selectedId} {revealId} {hoveredId} onHover={(id) => hoveredId = id}
                            changing={changingOvernight}
                            onToggle={showDay} onOverview={showAllDays}
                            onInspect={inspectPoint}
                            onShowEnd={showDayEnd}
                            onSelectPlace={selectPlace}
                            onPick={pickOnMap}
                            onChangeOvernight={(changing) => { changingOvernight = changing; clearSelection(); }}
                            onEditTarget={() => planEditing = true}
                            onShowConflict={([before, after]) => map?.fitCoordinates([before.coordinate, after.coordinate])}
                            onAddRest={(after) => commit(addRestDay(trip, after), 'Rest day added')}
                            onRemoveRest={(index) => commit(removeRestDay(trip, index), 'Rest day removed')}
                            onNameRest={nameRest}
                        />
                    {:else}
                        <RouteList {stops} {hoveredId} onHover={(id) => hoveredId = id} measured={!!currentRoute} onInspect={inspectPoint}
                            onReorder={(id, offset) => commit(reorderPoint(trip, id, offset), 'Stops reordered · changed legs follow roads')} />
                    {/if}
                    {#if nearbyLandmark && !focusedDay}
                        <NearbyLandmark landmark={nearbyLandmark} onRide={addVisit} onShow={selectPlace} />
                    {/if}
                </div>
            {/if}
        </aside>
        <Resize value={Math.min(sideWidth, maxSide)} min={320} max={maxSide} axis="x" label="Sidebar width" onResize={(value) => sideWidth = value} />
        <section class="geography" aria-label="Map and elevation">
            <div class="map-area" bind:clientHeight={mapHeight} style:--map-height={`${mapHeight}px`}>
                <PlannerMap
                    bind:this={map} {segments} {coordinates} points={mapPoints} {selectedId} {hoveredId} onPointHover={(id) => hoveredId = id} callout={calloutCoordinate} {drawing}
                    {theme} {hillshade} {contours} {mapOverlays} {showRoute} {hoverProgress} highlightedCoordinates={highlighted} pickMode={picking}
                    highlightedPlaceIds={searching ? results.map(result => result.place.id) : []}
                    shownCategories={categoryIds.filter(category => !hiddenCategories.includes(category))} {highlightedPlaces} {landmarks}
                    onBounds={(bounds, preserveSearch) => { viewBounds = bounds; if (!preserveSearch) searchRevision++; }} onEmptyClick={emptyClick} onPointSelect={selectPoint} onPointMove={movePoint} onPointPreview={previewPoint} onDayEndDrag={moveDayEnd}
                    onLegClick={legClick} onInsert={insert} onDrawn={drawn} onPlaceClick={choosePlace}
                    onVisibleRange={(range) => visibleRange = range}
                >
                    {#snippet popup()}
                        {#if calloutKind}
                            {#key selectedId}
                                <MapCallout
                                    kind={calloutKind} {trip} {days} {dayLabels} {night} {candidates} {legMode}
                                    onEndpoint={chooseEndpoint} point={selectedPoint} place={selectedPlace} coordinate={previewCoordinate}
                                    onClose={clearSelection}
                                    onAddHere={addHere}
                                    onLegMode={setLeg}
                                    onInsert={() => insert(spot!.legEndId!, spot!.coordinate)}
                                    onPick={pickOnMap}
                                    onSelectPlace={selectPlace}
                                    onStay={stayHere}
                                    onAddVisit={addVisit}
                                    onRename={rename}
                                    onKind={changeKind}
                                    onRemove={removePoint}
                                />
                            {/key}
                        {/if}
                    {/snippet}
                </PlannerMap>
                <div class="map-controls" aria-label="Map controls">
                    <div class="control-group">
                        <button type="button" onclick={() => map?.zoomBy(1)} aria-label="Zoom in"><Icon name="plus" /></button>
                        <button type="button" onclick={() => map?.zoomBy(-1)} aria-label="Zoom out"><Icon name="minus" /></button>
                    </div>
                    <div class="control-group">
                        <button type="button" disabled={!trip.points.length} onclick={() => focusedDay ? showDay(focusedDay.ridingNumber) : coordinates.length ? map?.fitRoute() : map?.fitCoordinates(trip.points.map(p => p.coordinate))} aria-label={focusedDay ? `Show day ${focusedDay.number} on map` : 'Show whole route'}><Icon name="fit" /></button>
                        <button type="button" disabled={!hasEndpoints} class:chosen={showRoute} aria-label={showRoute ? 'Hide route' : 'Show route'} aria-pressed={showRoute} onclick={() => showRoute = !showRoute}><Icon name="eye" /></button>
                    </div>
                    <LayerMenu {theme} bind:autoCenter bind:mapOverlays bind:hillshade bind:contours bind:hidden={hiddenCategories} bind:highlighted={highlightedCategories} />
                </div>
                {#if picking || drawing}
                    <div class="mode-chip" role="status">
                        {picking ? `Click the route or the map to end day ${dayLabels[night] ?? night} here · Esc` : 'Drawing the leg · Esc cancels'}
                        <button type="button" onclick={() => { picking = false; drawing = null; }}>Cancel</button>
                    </div>
                {/if}
            </div>
            <Resize value={Math.min(profileHeight, maxProfile)} min={210} max={maxProfile} axis="y" label="Elevation height" onResize={(value) => profileHeight = value} />
            {#if !currentRoute}
                <section class="empty-profile" aria-label="Elevation profile" style:height={`${Math.min(profileHeight, maxProfile)}px`}>
                    <h2>Elevation &amp; surface</h2>
                    <div><Icon name="route" size={24} /><p>{hasEndpoints ? 'The profile appears when your route is ready.' : 'See the climbs and surfaces along your route.'}</p><small>{hasEndpoints ? routingStatus : stops.length ? `Choose a ${nextEndpoint} to see the profile.` : 'Choose a start and finish to get started.'}</small></div>
                </section>
            {:else}
            {#key focusedDay?.number ?? 'overview'}
            <Profile
                lineData={currentRoute} singleRoute={!multi} height={Math.min(profileHeight, maxProfile)} {total} {days} {dayLabels} {theme}
                activeNight={focusedDay?.ridingNumber ?? 0} band={overnightContext ? area : null} window={profileWindow}
                focus={focusedDay ? { from: focusedDay.from, to: focusedDay.to, label: `Day ${focusedDay.number}` } : null}
                onNight={(riding) => showDay(riding)} onDayEndDrag={moveDayEnd} onHover={(progress) => hoverProgress = progress}
            />
            {/key}
            {/if}
            <div class="status-line" role="status">
                <span class:save-error={!!draftError}>{draftError || `${message} · ${routingMessage}`}</span>
                {#if undoable}<span>·</span><button type="button" class="planner-action" onclick={undo}>Undo</button>{/if}
                {#if placeNote}<span>· {placeNote}</span>{/if}
                <span class="lab-note">Regional map, search and routing{#if import.meta.env.VITE_PLANNER_DATA_URL} · <a href={import.meta.env.VITE_PLANNER_DATA_URL}>Routing data · ODbL</a>{/if}</span>
                <span class="legal"><a href={`${siteBase}docs/impressum/`}>Impressum</a> · <a href={`${siteBase}docs/datenschutz/`}>Datenschutz</a></span>
            </div>
        </section>
    </main>
</div>

<style>
    .start-plan { padding: 24px 20px; }
    .start-plan > :global(svg), .empty-profile :global(svg) { color: var(--ink-soft); }
    .start-plan h2 { margin: 16px 0 8px; font: 600 20px/1.3 var(--sans); text-wrap: balance; }
    .start-plan p { margin: 0; color: var(--ink-soft); line-height: 1.6; }
    .start-plan .next-step { margin-top: 24px; font-size: 13px; }
    .chosen-endpoint { display: flex; width: 100%; align-items: center; gap: 12px; margin-top: 24px; padding: 12px 0; text-align: left; border-block: 1px solid var(--line); }
    .chosen-endpoint:hover { color: var(--link); }
    .chosen-endpoint span { flex: 1; min-width: 0; overflow-wrap: anywhere; }
    .chosen-endpoint small, .chosen-endpoint strong { display: block; }
    .chosen-endpoint small { margin-bottom: 4px; color: var(--ink-soft); }
    .route-status { padding: 8px 16px; color: var(--ink-soft); }
    .empty-profile { flex: none; display: flex; flex-direction: column; padding: 16px 24px; background: var(--panel); }
    .empty-profile h2 { margin: 0; font: 600 14px var(--sans); }
    .empty-profile div { flex: 1; display: flex; flex-direction: column; align-items: center; justify-content: center; text-align: center; }
    .empty-profile p { margin: 12px 0 4px; }
    .empty-profile small { color: var(--ink-soft); }

    .search-note { margin: 8px 16px 0; color: var(--ink-soft); font-size: 13px; line-height: 1.4; }
    :global(*) {
        box-sizing: border-box;
        scrollbar-width: thin;
        scrollbar-color: var(--line-strong) transparent;
    }
    :global(::selection) {
        background: var(--parchment-3);
        color: var(--ink);
    }
    :global(body) {
        margin: 0;
        background: var(--parchment);
        color: var(--ink);
        font: 14px/1.4 var(--sans);
        -webkit-font-smoothing: antialiased;
    }
    :global(button),
    :global(input),
    :global(select) {
        font: inherit;
    }
    :global(button) {
        cursor: pointer;
    }
    :global(button:disabled) {
        opacity: .35;
        cursor: default;
    }
    :global(button:focus-visible),
    :global(input:focus-visible),
    :global(select:focus-visible),
    :global(a:focus-visible),
    :global(summary:focus-visible) {
        outline: 2px solid var(--forest);
        outline-offset: 2px;
    }
    button {
        padding: 0;
        border: 0;
        background: none;
        color: inherit;
    }
    /* Links are rust in light and tan in dark, so amber stays the one action colour in both. */
    .planner-shell {
        --query-place: #3b654c;
        --query-area: #365f84;
        --query-time: #965125;
        --planner-shadow: 0 6px 18px rgba(28, 27, 20, .12);
        --link: var(--forest);
        display: flex;
        flex-direction: column;
        height: 100dvh;
        min-width: 760px;
        min-height: 580px;
    }
    :global([data-theme="dark"]) .planner-shell {
        --query-place: #aed0b5;
        --query-area: #a9cce9;
        --query-time: #edb58b;
        --planner-shadow: 0 6px 18px rgba(0, 0, 0, .4);
        --link: var(--wood);
    }
    .planner-shell :global(.planner-action) {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        min-height: 32px;
        padding: 5px 10px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: inherit;
        font-weight: 600;
        text-decoration: none;
        cursor: pointer;
    }
    .planner-shell :global(.planner-action:hover:not(:disabled)) { background: var(--parchment-2); border-color: var(--ink-faint); }
    .planner-shell :global(.planner-action.quiet) {
        color: var(--ink-soft);
    }
    .site-header {
        display: flex;
        align-items: center;
        gap: 28px;
        height: 48px;
        flex: none;
        padding: 0 20px;
        background: var(--rust);
        color: var(--cream);
    }
    .brand {
        display: flex;
        align-items: center;
        gap: 10px;
        color: inherit;
        text-decoration: none;
    }
    .brand img {
        width: 28px;
        height: 28px;
        border-radius: 6px;
    }
    .brand span {
        font: 700 17px var(--mono);
    }
    .site-header nav {
        display: flex;
        align-items: center;
        gap: 24px;
        height: 100%;
        font-size: 13px;
        min-width: 0;
        overflow-x: auto;
        white-space: nowrap;
    }
    .site-header nav a {
        color: inherit;
        text-decoration: none;
    }
    .site-header nav a:hover {
        text-decoration: underline;
        text-underline-offset: 4px;
    }
    .site-header nav span {
        display: flex;
        align-items: center;
        height: 100%;
        font-weight: 700;
        border-bottom: 3px solid var(--amber);
    }
    .theme {
        display: grid;
        place-items: center;
        width: 32px;
        height: 32px;
        margin-left: auto;
        border-radius: 6px;
    }
    .theme:hover {
        background: color-mix(in srgb, var(--rust) 80%, var(--on-amber));
    }
    main {
        display: grid;
        grid-template-columns: var(--side-width) 8px minmax(0, 1fr);
        flex: 1;
        min-height: 0;
    }
    .planner-pane {
        display: flex;
        flex-direction: column;
        min-height: 0;
        background: var(--panel);
    }
    .trip-summary { padding: 0 16px; }
    .list-switch {
        display: flex;
        padding: 12px 16px 8px;
        border-top: 1px solid var(--line);
    }
    .pane-scroll {
        flex: 1;
        min-height: 0;
        overflow: auto;
    }
    .geography {
        display: flex;
        flex-direction: column;
        min-width: 0;
        min-height: 0;
    }
    .map-area {
        position: relative;
        flex: 1;
        min-height: 170px;
    }
    .map-controls {
        position: absolute;
        top: 16px;
        right: 16px;
        z-index: 3;
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    .control-group {
        border-radius: 8px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
    }
    .control-group button {
        display: grid;
        place-items: center;
        width: 36px;
        height: 36px;
        color: var(--ink);
        cursor: pointer;
    }
    .control-group button:hover {
        color: var(--link);
    }
    .control-group button + button {
        border-top: 1px solid var(--line);
    }
    .control-group .chosen {
        color: var(--link);
    }
    .mode-chip {
        position: absolute;
        top: 16px;
        left: 16px;
        z-index: 3;
        display: flex;
        align-items: center;
        gap: 12px;
        padding: 6px 12px;
        border-radius: 6px;
        background: var(--ink);
        color: var(--panel);
        font: 600 13px var(--sans);
        box-shadow: var(--planner-shadow);
    }
    .mode-chip button {
        color: inherit;
        font-weight: 600;
        text-decoration: underline;
        text-underline-offset: 3px;
    }
    .status-line {
        display: flex;
        align-items: center;
        gap: 12px;
        height: 38px;
        flex: none;
        padding: 0 16px;
        border-top: 1px solid var(--line);
        background: var(--panel);
        font-size: 11px;
        color: var(--ink-soft);
    }
    .save-error { color: var(--coral); }
    .lab-note {
        margin-left: auto;
    }
    .legal { white-space: nowrap; }
    @media (max-width: 1150px) {
        .brand span {
            font-size: 14px;
        }
    }
    @media (max-width: 700px) {
        .planner-shell { min-width: 0; height: auto; min-height: 100dvh; }
        main { grid-template-columns: minmax(0, 1fr); }
        main > :global([aria-label="Sidebar width"]) { display: none; }
        .planner-pane { max-height: 50dvh; }
        .geography { height: 75dvh; }
        .status-line { height: auto; min-height: 38px; flex-wrap: wrap; padding-block: 8px; }
        .lab-note { display: none; }
        .legal { margin-left: auto; }
        .site-header { gap: 12px; padding-inline: 12px; }
        .brand { flex: none; }
        .brand span { display: none; }
        .site-header nav { gap: 16px; }
        .theme { flex: none; }
    }
</style>
