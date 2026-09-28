<script lang="ts">
    import { onMount } from 'svelte';
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
    import WaysList, { ways } from './WaysList.svelte';
    import QueryResults from './QueryResults.svelte';
    import MapCallout, { type CalloutKind, type EditableKind } from './MapCallout.svelte';
    import LayerMenu from './LayerMenu.svelte';
    import NearbyLandmark from './NearbyLandmark.svelte';
    import { isTrip } from '../../lib/planner/trip-validation';
    import {
        addClickedPoint, addPointNear, addRestDay, anchorProgress, applyBudget, coordinateAt, cumulative, initialTrip,
        insertPoint, itineraryDays, kilometres, nearestProgress, nightOrderConflicts, offRoute, overnightCandidates,
        overnightWindow, pinNight, places, removeRestDay, reorderPoint, routeCoordinates, routeSlice, routeStops,
        setDrawnLeg, setLegMode, setSplit, TripHistory, tripDays,
        type Coordinate, type Day, type LegMode, type Place, type PointKind, type RoutePoint, type Trip,
    } from '../../lib/planner/editor';
    import { categoryIds, type PlaceCategory } from '../../lib/planner/poi-kinds';
    import { corridorPlaces } from '../../lib/planner/place-index';
    import { landmarks } from '../../lib/planner/landmarks';
    import { BASEMAP_URL } from '../../lib/planner/map-data';
    import { dayColor } from '../../lib/planner/day-colors';
    import { profileAscent } from '../../lib/planner/profile-data';
    import type { PlannerQueryValue } from '../../lib/planner/query';
    import type { MapPoint, MapSegment } from '../../lib/planner/map-types';
    import type { Version } from '../../lib/planner/versions';

    const storageKey = 'obc-planner-lab-v1';
    const defaultLabels: Record<EditableKind, string> = {
        via: 'Shaping point',
        pass: 'Pass here',
        waypoint: 'Visit',
        night: 'Overnight spot',
        marker: 'Marker',
    };

    let trip = $state<Trip>(initialTrip());
    let list = $state<'plan' | 'ways'>('plan');
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
    // A place chosen while picking offers the night whatever its kind.
    let pickedForNight = $state(false);
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
    let hillshade = $state(true);
    let contours = $state(true);
    let showRoute = $state(true);
    let query = $state('');
    let searchValue = $state<PlannerQueryValue>({ text: '', category: 'all', day: null, within: null });
    let message = $state('Example route and places');
    // The status line offers Undo after a version restore, until the next change.
    let undoable = $state(false);
    let draftSavedAt = $state<number | null>(null);
    let draftError = $state('');
    let visibleRange = $state<[number, number] | null>(null);
    let hoverProgress = $state<number | null>(null);
    let sideWidth = $state(360);
    let profileHeight = $state(190);
    let viewportHeight = $state(900);
    let viewportWidth = $state(1200);
    let mapHeight = $state(600);
    let map: PlannerMap | undefined;
    const history = new TripHistory();
    let revision = $state(0);

    const maxProfile = $derived(Math.max(140, Math.min(340, viewportHeight - 400)));
    const maxSide = $derived(Math.max(320, Math.min(460, viewportWidth - 540)));
    const canUndo = $derived.by(() => { void revision; return history.canUndo; });
    const canRedo = $derived.by(() => { void revision; return history.canRedo; });
    const coordinates = $derived(routeCoordinates(trip));
    const lengths = $derived(cumulative(coordinates));
    const total = $derived(lengths.at(-1)!);
    const stops = $derived(routeStops(trip));
    const days = $derived(tripDays(trip));
    const multi = $derived(trip.mode !== 'route');
    const itinerary = $derived(itineraryDays(trip));
    const focusedDay = $derived(multi && list === 'plan' && !searching ? itinerary.find(d => !d.rest && d.ridingNumber === expandedDay) ?? null : null);
    const dayLabels = $derived(Object.fromEntries(itinerary.filter(d => !d.rest).map(d => [d.ridingNumber, d.number])));
    const conflicts = $derived(multi ? nightOrderConflicts(trip) : []);
    const activeDay = $derived(days[Math.min(night - 1, days.length - 1)]);
    const area = $derived(night < days.length ? overnightWindow(trip, night) : null);
    const overnightContext = $derived(
        multi && expandedDay !== null && !searching && list === 'plan' && night < days.length && (!activeDay?.pinned || changingOvernight),
    );
    const highlighted = $derived(overnightContext && area && !area.blocked ? routeSlice(coordinates, area.from, area.to) : []);
    const candidates = $derived(multi ? overnightCandidates(trip, night) : []);
    // One stretch per leg and day, so the map can tell legs apart and colour days.
    const segments = $derived.by(() => {
        const length = stops.at(-1)!.distance || 1;
        return stops.slice(1).flatMap((stop, i) => days.flatMap((day): MapSegment[] => {
            const from = Math.max(day.from, stops[i].distance / length);
            const to = Math.min(day.to, stop.distance / length);
            if (to <= from) return [];
            return [{ coordinates: routeSlice(coordinates, from, to), color: dayColor(day.number, theme), legEndId: stop.point.id, leg: stop.point.leg ?? 'routed' }];
        }));
    });
    const searchDay = $derived(searchValue.day === null ? null : itinerary.find(d => d.number === searchValue.day && !d.rest) ?? null);
    // A category search also lists the basemap places along the route; "all places" stays with the examples.
    const searchable = $derived(searching && searchValue.category !== 'all'
        ? [...places, ...corridor.filter(place => place.category === searchValue.category && offRoute(coordinates, place.coordinate) <= 5)]
        : places);
    const searchLoading = $derived(searching && searchValue.category !== 'all' && corridorLoad === 'loading');
    const results = $derived.by(() => {
        const dayEnd = searchDay ? coordinateAt(coordinates, searchDay.to) : null;
        return searchable
            .filter(place => searchValue.category === 'all' || place.category === searchValue.category)
            .map(place => {
                const at = nearestProgress(coordinates, place.coordinate);
                return { place, at, where: whereAlong(at), off: offRoute(coordinates, place.coordinate) };
            })
            .filter(result => searchValue.within === null || (dayEnd ? kilometres(result.place.coordinate, dayEnd) : result.off) <= searchValue.within)
            .sort((a, b) => searchDay ? Math.abs(a.at - searchDay.to) - Math.abs(b.at - searchDay.to) : a.at - b.at);
    });
    const visiblePlaces = $derived(searching ? results.map(result => result.place) : overnightContext ? candidates.map(candidate => candidate.place) : []);
    // The search category highlights its places too while the search is open.
    const highlights = $derived(searching && searchValue.category !== 'all' && !highlightedCategories.includes(searchValue.category)
        ? [...highlightedCategories, searchValue.category] : highlightedCategories);
    const highlightLimit = 300;
    // Fixture places of a highlighted category always show; basemap places within 5 km of the route, nearest first.
    const highlightCandidates = $derived.by(() => {
        if (!highlights.length) return [];
        const pinned = new Set(visiblePlaces.map(place => place.id));
        const nearby = corridor
            .filter(place => highlights.includes(place.category))
            .map(place => ({ place, off: offRoute(coordinates, place.coordinate) }))
            .filter(({ off }) => off <= 5)
            .sort((a, b) => a.off - b.off)
            .map(({ place }) => place);
        return [...places.filter(place => highlights.includes(place.category)), ...nearby].filter(place => !pinned.has(place.id));
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
    const nearbyLandmark = $derived(landmarks
        .filter(landmark => offRoute(coordinates, landmark.coordinate) <= 15 && !trip.points.some(p => kilometres(p.coordinate, landmark.coordinate) < .3))
        .sort((a, b) => nearestProgress(coordinates, a.coordinate) - nearestProgress(coordinates, b.coordinate))[0]);
    const selectedPlace = $derived(places.find(p => p.id === selectedId) ?? corridor.find(p => p.id === selectedId) ?? (mapPlace?.id === selectedId ? mapPlace : undefined));
    const selectedPoint = $derived(trip.points.find(p => p.id === selectedId));
    const previewCoordinate = $derived(selectedId === 'pending' ? pending : selectedPlace?.coordinate ?? null);
    const mapPoints = $derived.by(() => {
        const pins: MapPoint[] = trip.points.map(p => ({
            ...p,
            kind: !multi && p.kind === 'night' ? 'waypoint' : p.kind,
            color: multi && p.kind === 'night' ? dayColor(p.night!, theme) : undefined,
            markerLabel: multi && p.kind === 'night' ? String(dayLabels[p.night!]) : undefined,
            fixed: p.kind === 'night' || (['waypoint', 'detour'].includes(p.kind) && p.label !== defaultLabels.waypoint),
        }));
        const shown = selectedPlace && places.includes(selectedPlace) && !visiblePlaces.includes(selectedPlace) ? [...visiblePlaces, selectedPlace] : visiblePlaces;
        for (const p of shown) {
            if (p.id === selectedId || !pins.some(pin => pin.coordinate[0] === p.coordinate[0] && pin.coordinate[1] === p.coordinate[1])) pins.push({ ...p });
        }
        if (pending) pins.push({ id: 'pending', coordinate: pending, label: 'Overnight spot', kind: 'place', appearance: 'suggested' });
        // Day ends come last, so they stay on top of a candidate place at the same spot and can be dragged.
        for (const day of multi ? days.slice(0, -1) : []) {
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

    /** Where a route position sits in the trip, in the rider's words. */
    function whereAlong(at: number): string {
        if (!multi) return `${(at * total).toFixed(1)} km from the start`;
        const day = days.find(d => at <= d.to) ?? days.at(-1)!;
        if (searchDay && day.number === searchDay.ridingNumber) return `${((day.to - at) * total).toFixed(1)} km before day ${dayLabels[day.number]} ends`;
        return `${((at - day.from) * total).toFixed(1)} km into day ${dayLabels[day.number]}`;
    }

    $effect(() => {
        if (!highlights.length) return;
        const route = coordinates;
        let current = true;
        corridorLoad = 'loading';
        corridorPlaces(BASEMAP_URL.replace(/^pmtiles:\/\//, ''), route).then(
            found => { if (current) { corridor = found; corridorLoad = 'done'; } },
            () => { if (current) corridorLoad = 'failed'; },
        );
        return () => { current = false; };
    });

    onMount(() => {
        try {
            const raw = localStorage.getItem(storageKey);
            const saved = raw ? JSON.parse(raw) : null;
            if (isTrip(saved)) trip = saved;
            else if (raw) draftError = 'Saved draft is invalid · example route loaded';
        } catch (error) {
            draftError = error instanceof SyntaxError ? 'Saved draft is invalid · example route loaded' : 'Draft · browser storage unavailable';
        }
    });

    $effect(() => {
        document.documentElement.dataset.theme = theme;
    });

    function save() {
        try {
            localStorage.setItem(storageKey, JSON.stringify(trip));
            draftSavedAt = Date.now();
            draftError = '';
        } catch {
            draftError = 'Draft · could not save locally';
        }
    }

    function commit(next: Trip, description: string) {
        trip = history.commit($state.snapshot(trip), next);
        revision++;
        message = description;
        undoable = false;
        save();
    }

    function edit(change: Partial<Trip>, description: string) {
        commit({ ...$state.snapshot(trip), ...change }, description);
    }

    function clearSelection() {
        selectedId = null;
        pending = null;
        pendingSource = null;
        spot = null;
        picking = false;
        pickedForNight = false;
    }

    function afterHistory(description: string) {
        revision++;
        clearSelection();
        night = Math.min(night, tripDays(trip).length);
        if (expandedDay !== null) expandedDay = night;
        save();
        message = description;
        undoable = false;
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

    function dayOf(coordinate: Coordinate): number {
        const at = nearestProgress(coordinates, coordinate);
        return (days.find(d => at <= d.to) ?? days.at(-1)!).number;
    }

    function undo() {
        trip = history.undo($state.snapshot(trip));
        afterHistory('Change undone');
    }

    function redo() {
        trip = history.redo($state.snapshot(trip));
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

    /** Opens a place clicked on the map; while picking, the place is offered for the night. */
    function choosePlace(place: Place) {
        const forNight = picking;
        clearSelection();
        if (!places.includes(place)) mapPlace = place;
        selectedId = place.id;
        pickedForNight = forNight;
    }

    function selectPoint(id: string) {
        if (id === 'pending') return;
        const forNight = picking;
        clearSelection();
        selectedId = id;
        pickedForNight = forNight;
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
        commit(setLegMode($state.snapshot(trip), legEndId, mode), mode === 'straight' ? 'Leg set to a straight line' : 'Leg set to routed');
    }

    function insert(legEndId: string, coordinate: Coordinate) {
        clearSelection();
        commit(insertPoint($state.snapshot(trip), legEndId, coordinate), 'Shaping point inserted');
    }

    function drawn(legEndId: string, line: Coordinate[]) {
        drawing = null;
        commit(setDrawnLeg($state.snapshot(trip), legEndId, line), 'Leg drawn');
    }

    function moveDayEnd(number: number, progress: number) {
        commit(setSplit($state.snapshot(trip), number, progress), 'Day end moved');
    }

    function stayHere(sleepDay: number) {
        if (!previewCoordinate) return;
        const coordinate: Coordinate = [...previewCoordinate];
        const source = trip.points.find(p => p.id === pendingSource);
        const label = selectedPlace?.label ?? (source && !['via', 'pass'].includes(source.kind) ? source.label : 'Overnight spot');
        const current = $state.snapshot(trip);
        const next = pinNight(current, sleepDay, coordinate, label, pendingSource ?? undefined);
        if (next === current) return;
        commit(next, 'Overnight pinned');
        clearSelection();
        reveal(`night-${sleepDay}`, sleepDay);
    }

    function newPoint(coordinate: Coordinate, kind: EditableKind, label?: string): RoutePoint {
        return { id: crypto.randomUUID(), coordinate: [...coordinate], label: label ?? defaultLabels[kind], kind, progress: anchorProgress(coordinate) };
    }

    const added: Partial<Record<PointKind, string>> = { via: 'Shaping point added', waypoint: 'Visit added', night: 'Overnight pinned', marker: 'Marker added', pass: 'Pass added' };

    function addPoint(coordinate: Coordinate, kind: EditableKind) {
        const point = newPoint(coordinate, kind);
        commit(addClickedPoint($state.snapshot(trip), point), added[kind]!);
        clearSelection();
        selectedId = point.id;
        reveal(point.id, dayOf(point.coordinate));
    }

    function addVisit(place: Place) {
        const point = newPoint(place.coordinate, 'waypoint', place.label);
        commit(addPointNear($state.snapshot(trip), point), 'Visit added');
        clearSelection();
        selectedId = point.id;
        reveal(point.id, dayOf(point.coordinate));
    }

    function movePoint(id: string, coordinate: Coordinate) {
        const point = trip.points.find(p => p.id === id);
        if (!point) return;
        if (point.kind === 'night') {
            commit(pinNight($state.snapshot(trip), point.night!, coordinate, point.label), 'Overnight moved');
            return;
        }
        const next = $state.snapshot(trip);
        next.points = next.points.map(p => p.id === id
            ? { ...p, coordinate, progress: p.kind === 'start' || p.kind === 'finish' ? p.progress : anchorProgress(coordinate) }
            : p);
        commit(next, 'Point moved');
    }

    function removePoint() {
        if (!selectedPoint || selectedPoint.kind === 'start' || selectedPoint.kind === 'finish') return;
        const next = $state.snapshot(trip);
        next.points = next.points.filter(p => p.id !== selectedId);
        next.routeOrder = next.routeOrder?.filter(id => id !== selectedId);
        commit(next, 'Point removed');
        clearSelection();
    }

    function rename(label: string) {
        if (!selectedPoint) return;
        const next = $state.snapshot(trip);
        next.points = next.points.map(p => p.id === selectedId ? { ...p, label } : p);
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
        const label = kind === 'via' ? defaultLabels.via
            : point.kind === 'via' ? defaultLabels[kind === 'detour' ? 'waypoint' : kind]
            : point.label;
        const next = $state.snapshot(trip);
        next.points = next.points.map(p => p.id === point.id ? { ...p, id, kind, night: undefined, label } : p);
        next.routeOrder = next.routeOrder?.map(old => old === point.id ? id : old);
        commit(next, 'Point type updated');
        selectedId = id;
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
        commit({ ...applyBudget($state.snapshot(trip), budget, target, limit), climbTarget: climb || undefined }, 'Day plan updated');
        night = Math.min(night, trip.days);
        if (expandedDay !== null) expandedDay = night;
    }

    function search(value: PlannerQueryValue) {
        searchValue = value;
        searching = true;
        clearSelection();
        if (searchDay) night = searchDay.ridingNumber;
        const region = [...results.map(result => result.place.coordinate), ...(searchDay ? routeSlice(coordinates, searchDay.from, searchDay.to) : [])];
        if (region.length) map?.fitCoordinates(region);
    }

    function nameRest(index: number, name: string) {
        const names = (trip.restAfter ?? []).map((_, i) => i === index ? name.trim() : trip.restNames?.[i] ?? '');
        edit({ restNames: names }, 'Rest day named');
    }

    function restoreVersion(saved: Trip, name: string) {
        commit(saved, `Restored ‘${name}’`);
        undoable = true;
        clearSelection();
        night = Math.min(night, tripDays(trip).length);
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
        <a class="brand" href="/"><img src="/brand/app-icon.svg" alt="" /><span>OpenBikeComputer</span></a>
        <nav aria-label="Preview navigation">
            <a href="/map-study.html">Map study</a>
            <span aria-current="page">Planner</span>
        </nav>
        <button type="button" class="theme" aria-label={theme === 'light' ? 'Use dark theme' : 'Use light theme'} onclick={() => theme = theme === 'light' ? 'dark' : 'light'}>
            <Icon name={theme === 'light' ? 'moon' : 'sun'} />
        </button>
    </header>
    <TripBar
        {trip} {canUndo} {canRedo} {draftSavedAt} {draftError}
        onChange={changeTrip} onUndo={undo} onRedo={redo} onRestore={restoreVersion}
        onSaved={(version: Version) => message = `Version saved · ${version.summary}`}
    />
    <main>
        <aside class="planner-pane" aria-label="Trip planning">
            <Query bind:text={query} days={multi ? itinerary.filter(d => !d.rest).map(d => d.number) : []} onSearch={search} onClear={clearSearch} />
            {#if searching}
                <div class="pane-scroll">
                    <QueryResults {results} {selectedId} loading={searchLoading} searchedDay={searchDay?.number ?? null} onSelect={selectPlace}
                        days={multi ? itinerary.filter(d => !d.rest).map(d => ({ number: d.number, color: dayColor(d.ridingNumber, theme) })) : []} />
                </div>
            {:else}
                {#if !focusedDay}
                    <div class="trip-summary"><RouteStats distance={total} ascent={profileAscent()} hours={total / 15} /></div>
                {/if}
                {#if !focusedDay || planEditing}
                    <PlanLine {trip} dayCount={itinerary.length} bind:editing={planEditing} onApply={applyPlan} />
                {/if}
                {#if !focusedDay}
                    <div class="list-switch">
                        <Segmented compact label="List" value={list} onChange={(value) => list = value}
                            options={[{ value: 'plan', label: multi ? 'Days' : 'Route' }, { value: 'ways', label: `Route options · ${ways.length}` }]} />
                    </div>
                {/if}
                <div class="pane-scroll">
                    {#if list === 'ways'}
                        <WaysList {trip} onPick={(variant) => edit({ variant }, 'Way changed')} />
                    {:else if multi}
                        <Itinerary
                            {trip} {itinerary} {days} {theme} {expandedDay} {candidates} {conflicts} {selectedId} {revealId}
                            changing={changingOvernight}
                            onToggle={showDay} onOverview={showAllDays}
                            onInspect={inspectPoint}
                            onShowEnd={showDayEnd}
                            onSelectPlace={selectPlace}
                            onPick={pickOnMap}
                            onChangeOvernight={(changing) => { changingOvernight = changing; clearSelection(); }}
                            onEditTarget={() => planEditing = true}
                            onShowConflict={([before, after]) => map?.fitCoordinates([before.coordinate, after.coordinate])}
                            onAddRest={(after) => commit(addRestDay($state.snapshot(trip), after), 'Rest day added')}
                            onRemoveRest={(index) => commit(removeRestDay($state.snapshot(trip), index), 'Rest day removed')}
                            onNameRest={nameRest}
                        />
                    {:else}
                        <RouteList {stops} onInspect={inspectPoint}
                            onReorder={(id, offset) => commit(reorderPoint($state.snapshot(trip), id, offset), 'Stop reordered')} />
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
                    bind:this={map} {segments} {coordinates} points={mapPoints} {selectedId} callout={calloutCoordinate} {drawing}
                    {theme} {hillshade} {contours} {showRoute} {hoverProgress} highlightedCoordinates={highlighted} pickMode={picking}
                    highlightedPlaceIds={searching ? results.map(result => result.place.id) : []}
                    shownCategories={categoryIds.filter(category => !hiddenCategories.includes(category))} {highlightedPlaces} {landmarks}
                    onEmptyClick={emptyClick} onPointSelect={selectPoint} onPointMove={movePoint} onDayEndDrag={moveDayEnd}
                    onLegClick={legClick} onInsert={insert} onDrawn={drawn} onPlaceClick={choosePlace}
                    onVisibleRange={(range) => visibleRange = range}
                >
                    {#snippet popup()}
                        {#if calloutKind}
                            {#key selectedId}
                                <MapCallout
                                    kind={calloutKind} {trip} {days} {dayLabels} {night} {candidates} {legMode}
                                    point={selectedPoint} place={selectedPlace} coordinate={previewCoordinate} forNight={pickedForNight}
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
                        <button type="button" onclick={() => focusedDay ? showDay(focusedDay.ridingNumber) : map?.fitRoute()} aria-label={focusedDay ? `Show day ${focusedDay.number} on map` : 'Show whole route'}><Icon name="fit" /></button>
                        <button type="button" class:chosen={showRoute} aria-label={showRoute ? 'Hide route' : 'Show route'} aria-pressed={showRoute} onclick={() => showRoute = !showRoute}><Icon name="eye" /></button>
                    </div>
                    <LayerMenu bind:hillshade bind:contours bind:hidden={hiddenCategories} bind:highlighted={highlightedCategories} />
                </div>
                {#if picking || drawing}
                    <div class="mode-chip" role="status">
                        {picking ? `Click the route or the map to end day ${dayLabels[night] ?? night} here · Esc` : 'Drawing the leg · Esc cancels'}
                        <button type="button" onclick={() => { picking = false; drawing = null; }}>Cancel</button>
                    </div>
                {/if}
            </div>
            <Resize value={Math.min(profileHeight, maxProfile)} min={130} max={maxProfile} axis="y" label="Elevation height" onResize={(value) => profileHeight = value} />
            {#key focusedDay?.number ?? 'overview'}
            <Profile
                height={Math.min(profileHeight, maxProfile)} {total} {days} {dayLabels} {theme}
                activeNight={focusedDay?.ridingNumber ?? 0} band={overnightContext ? area : null} window={profileWindow}
                focus={focusedDay ? { from: focusedDay.from, to: focusedDay.to, label: `Day ${focusedDay.number}` } : null}
                onNight={(riding) => showDay(riding)} onDayEndDrag={moveDayEnd} onHover={(progress) => hoverProgress = progress}
            />
            {/key}
            <div class="status-line" role="status">
                <span class:save-error={!!draftError}>{draftError || message}</span>
                {#if undoable}<span>·</span><button type="button" class="planner-action" onclick={undo}>Undo</button>{/if}
                {#if placeNote}<span>· {placeNote}</span>{/if}
                <span class="lab-note">Routing & elevation mocked</span>
            </div>
        </section>
    </main>
</div>

<style>
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
        --planner-shadow: 0 6px 18px rgba(28, 27, 20, .12);
        --link: var(--forest);
        display: flex;
        flex-direction: column;
        height: 100dvh;
        min-width: 760px;
        min-height: 580px;
    }
    :global([data-theme="dark"]) .planner-shell {
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
    @media (max-width: 1150px) {
        .brand span {
            font-size: 14px;
        }
    }
</style>
