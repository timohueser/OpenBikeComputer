<script lang="ts">
    import { onMount, tick, untrack } from 'svelte';
    import PlannerMap from './PlannerMap.svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import Profile from './PlannerProfile.svelte';
    import DrawerTitle from './DrawerTitle.svelte';
    import Query from './PlannerQuery.svelte';
    import Resize from './PanelResize.svelte';
    import TripBar from './TripBar.svelte';
    import RidePrefs from './RidePrefsPrototype.svelte';
    import TypeProto from './TypeProto.svelte';
    import SaveWindow, { type SaveAnswer, type SaveMode } from './SaveWindow.svelte';
    import Itinerary from './Itinerary.svelte';
    import RouteList from './RouteList.svelte';
    import RouteStats from './RouteStats.svelte';
    import CompareList from './CompareList.svelte';
    import CompareProfile from './CompareProfile.svelte';
    import CompareScreen from './CompareScreen.svelte';
    import { optionLetter, optionName, otherWays, routeRow, type Compared } from '../../lib/planner/route-options';
    import QueryResults from './QueryResults.svelte';
    import MapCallout, { type CalloutKind, type EditableKind } from './MapCallout.svelte';
    import MapSettings from './MapSettings.svelte';
    import PlaceKinds from './PlaceKinds.svelte';
    import LayerLegend from './LayerLegend.svelte';
    import LayerDateBar from './LayerDateBar.svelte';
    import LayerInspect from './LayerInspect.svelte';
    import LayerStrip from './LayerStrip.svelte';
    import TrailViewControl from './TrailViewControl.svelte';
    import TrailViewWindow from './TrailViewWindow.svelte';
    import type { Near } from '../../lib/planner/trailview-first';
    import { routeColours } from '../../lib/planner/trailview-route';
    import { hasTrailView } from '../../lib/planner/trailview';
    import { dataLayers } from '../../lib/planner/layers/registry';
    import { addDays, type DataLayer } from '../../lib/planner/layers/data-layer';
    import type { OverlayOptions } from '../../lib/planner/route-overlays';
    import { presetName, type BikeType } from '../../lib/planner/riding-profiles';
    import type { RoutePlan } from '../../lib/planner/signed-route-plan';
    import type { CatalogRecord } from '../../lib/planner/signed-routes';
    import LibraryPanel from './PlanLibrary.svelte';
    import type { Plan } from '../../lib/planner/library';
    import type { Library } from '../../lib/library/store';
    import { PlanSession } from '../../lib/planner/plan-session.svelte';
    import { router } from '../../lib/router.svelte';
    import { ROUTES } from '../../lib/routes';
    import {
        addClickedPoint, addPointNear, addRestDay, addTransfer, allStay, closeLoop, releaseNight, trainAfter, routeToTrip, hasEndpoints as endpointsChosen,
        insertPoint, planTitle, nightOrderConflicts, dragPointOut,
        orderedRoutePoints, pinNight, pinNights, planView, daySegments, removeRestDay, reorderPoint, replacePoint, startLoopHere,
        setDrawnLeg, setLegMode, setSplit, setEndpoint, removeRoutePoint,
        type Day, type LegMode, type Place, type PlaceRef, type PointKind, type RoutePoint, type Trip,
    } from '../../lib/planner/editor';
    import { coordinateAt, cumulative, nearestOnLine, nearestProgress, routeSlice, type Coordinate } from '../../lib/planner/geo';
    import { placeChoice } from '../../lib/planner/place-choice.svelte';
    import { config } from '../../lib/planner/map-data';
    import { beforeReload } from '../../lib/planner/release';
    import { coordinateName, townName } from '../../lib/planner/point-names';
    import { dayColor } from '../../lib/planner/day-colors';
    import { profileHeightAt, profileSamples, sampleIndex } from '../../lib/planner/profile-data';
    import { StopFinder, stretchKey, type NightChoices } from '../../lib/planner/night-stops.svelte';
    import { nightKinds, nightWindows, proposal, proposeNights, suits, withProposals, type Proposal, type Stop } from '../../lib/planner/stops';
    import { searchPlan } from '../../lib/planner/search/plan';
    import { placeDetails, withDetails, type SearchPlace, type SearchState, type SearchContext, type Where } from '../../lib/planner/search/types';
    import { placeSource } from '../../lib/planner/place-index';
    import { asPlace, category, sleepCategory, stayKind, stayOptions, trainNightText } from '../../lib/planner/search/presentation';
    import { applyQueryChanges } from '../../lib/planner/search/actions';
    import type { MapPoint, MapSegment } from '../../lib/planner/map-types';
    import { appearance } from '../../lib/theme.svelte';
    import { routingKey, type EngineRoute, type RoutingLine } from '../../lib/planner/routing';
    import { importedTrip, readTracks } from '../../lib/planner/gpx-import';
    import { DRAWER_CLOSED, DRAWER_MIN, resizeDrawer } from '../../lib/planner/drawer';
    import { closureNote, closureStretches, type Stretch } from '../../lib/planner/route-closures';
    import { alpineNote, alpineStretches } from '../../lib/planner/route-difficulty';
    import { gapNote, routeGaps } from '../../lib/planner/route-gaps';
    import { distanceIn } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';
    const defaultLabels: Record<EditableKind, string> = {
        via: 'Shaping point',
        pass: 'Pass here',
        waypoint: 'Visit',
        night: 'Overnight spot',
        marker: 'Marker',
    };

    // The page gives the library, so that the cover job hears the saves of the planner.
    // While `active` is false the planner is hidden under another page and reads no keys.
    let { library, active = true }: { library: Library; active?: boolean } = $props();
    const session = new PlanSession(opened);
    const trip = $derived(session.trip);
    const planId = $derived(session.plan.id);
    let fitPlan = $state(false);
    let libraryOpen = $state(false);
    let dropping = $state(false);
    const hasEndpoints = $derived(endpointsChosen(trip));
    const nextEndpoint = $derived(trip.points.some(p => p.kind === 'start') ? 'finish' : 'start');
    const shown = $derived(session.shown);
    // The line of the trip or of the drag preview. While a new line is calculated, only the shown line is an earlier one.
    const currentRoute = $derived(session.stale ? undefined : shown.line);
    const visualRoute = $derived(shown.line);
    const routingStatus = $derived(!hasEndpoints ? 'Choose a start and finish' : session.routeError || 'Calculating route…');
    function useOption(route: EngineRoute, name: string) {
        const preset = presetName(route.profile);
        if (session.pickAlternative(route)) afterEdit(name === preset ? `Using ${preset}` : `Using ${name} · ${preset} preset`);
    }
    $effect(() => {
        if (fitPlan && currentRoute && map) {
            const id = planId;
            fitPlan = false;
            void tick().then(() => { if (planId === id) map?.fitRoute(); });
        }
    });
    const distance = (km: number) => `${distanceIn(km * 1000).toFixed(1)} ${settings.units}`;
    const routingMessage = $derived(session.dragging ? session.previewStatus : currentRoute
        ? `${distance(currentRoute.unknownSurfaceKm)} unknown surface${trip.bike !== 'hiking' && currentRoute.pushingKm ? ` · ${distance(currentRoute.pushingKm)} pushing` : ''}${currentRoute.unroutedKm ? ` · ${distance(currentRoute.unroutedKm)} manual / access unverified` : ''}${currentRoute.unknownElevationKm ? ' · elevation incomplete' : ''}`
        : routingStatus);
    let list = $state<'plan' | 'ways'>('plan');
    // The compare screen of saved routes replaces the trip bar and the planner below it.
    let comparing = $state(false);
    const options = $derived(session.routeOptions);
    // The first route of a plan asks for its options once, so the Route options link can say what they offer.
    let firstOptions = $state(false);
    $effect(() => {
        if ((list === 'ways' || firstOptions) && !session.dragging && session.line) untrack(() => { firstOptions = false; session.findAlternatives(); });
    });
    const optionStatus = { unavailable: 'Route options need one routed line from start to finish.', loading: 'Finding route options…',
        stale: 'Your plan changed. Finding route options for it…' };
    const optionRows = $derived(options.state === 'ready' ? options.routes.map((route, i) =>
        i ? routeRow(route, optionName(route, session.optionPlaces)) : { ...routeRow(route, 'Your route'), detail: presetName(route.profile) }) : []);
    const optionsReason = $derived(otherWays(optionRows));
    // The previewed row and your route; the map and the drawer show them until the rider leaves the list.
    const comparison = $derived.by(() => {
        const index = list === 'ways' ? optionRows.findIndex(row => row.id === session.compared?.id) : -1;
        return index > 0 ? { option: optionRows[index], current: optionRows[0], letter: optionLetter(index) } : null;
    });
    $effect(() => { if (list !== 'ways' || searching) untrack(() => session.previewOption(null)); });

    let listLinks = $state<HTMLDivElement>();
    /** An applied row ends its comparison, so the link that opens the comparison takes the focus. */
    async function focusLink() {
        await tick();
        listLinks?.querySelector<HTMLElement>('[data-list="ways"]')?.focus();
    }

    function useOptionRow(row: Compared) {
        const route = options.state === 'ready' && options.routes.find(r => r.id === row.id);
        if (route) useOption(route, row.name);
        void focusLink();
    }

    let searching = $state(false);
    // Riding numbers stay stable when the itinerary includes rest days.
    let night = $state(1);
    let expandedDay = $state<number | null>(null);
    // The night whose towns are listed: one opened in the ladder, or a pinned night the rider changes.
    let choosingNight = $state<number | null>(null);
    let stopId = $state<string | null>(null);
    const stopFinder = new StopFinder();
    let selectedId = $state<string | null>(null);
    // The point whose panel row lights up for a moment after it was added or pinned.
    let revealId = $state<string | null>(null);
    let revealTimer: ReturnType<typeof setTimeout> | undefined;
    // A spot picked on the map, and the point it replaces when a point becomes an overnight.
    let pending = $state<Coordinate | null>(null);
    let pendingSource = $state<string | null>(null);
    // What the next map click does instead of its usual action: end a day there, draw a leg, set where a train or bus after a
    // night arrives, or open TrailView. A new selection ends it.
    let mapMode = $state<{ kind: 'night' } | { kind: 'draw'; legEndId: string } | { kind: 'transfer'; night: number } | { kind: 'look' } | null>(null);
    const picking = $derived(mapMode?.kind === 'night');
    const drawing = $derived(mapMode?.kind === 'draw' ? mapMode.legEndId : null);
    // Where an add or leg callout opened; a leg callout also names its leg.
    let spot = $state<{ coordinate: Coordinate; legEndId?: string } | null>(null);
    // The selected place when it is not a fixture: a basemap place.
    let mapPlace = $state<Place | null>(null);
    const theme = $derived<'light' | 'dark'>(appearance.dark ? 'dark' : 'light');
    let hoveredId = $state<string | null>(null);
    let hillshade = $state(true);
    let contours = $state(true);
    let mapOverlays = $state<OverlayOptions>({ network: 'cycling', access: true });
    let showRoute = $state(true);
    // The map shows one data layer at a time; '' shows none.
    let shownLayerId = $state('');
    const dataLayer = $derived(dataLayers.find(layer => layer.id === shownLayerId));
    let dateBarHeight = $state(0);
    // The layer date follows the trip start until the rider picks another day; it never changes the trip.
    let layerDate = $state<string | null>(null);
    const today = new Date(Date.now() - new Date().getTimezoneOffset() * 60_000).toISOString().slice(0, 10);
    const shownDate = $derived(layerDate ?? trip.startDate ?? today);
    let query = $state('');
    let searchState = $state<SearchState>({ loading: false, error: '', answer: null });
    let searchBox = $state<Query>();
    let searchRevision = $state(0);
    let searchViewRevision = $state(0);
    let viewBounds = $state<[number, number, number, number]>(config.bounds);
    let here = $state<Coordinate | undefined>();
    let pointing = $state<Where | undefined>();
    let applyingQuery = $state(false);
    let queryApplyError = $state('');
    let message = $state(`Plan a ride in ${config.name}`);
    // The status line offers Undo after "Discard changes", until the next change.
    let statusUndo = $state<(() => void) | null>(null);
    // The open window of a save, a copy, a rename or the question before another route replaces the open one.
    let asking = $state<{ mode: SaveMode; answer: (answer: SaveAnswer) => void } | null>(null);
    let visibleRange = $state<[number, number] | null>(null);
    let hoverProgress = $state<number | null>(null);
    // The part of the route the profile selects, as route progress. Another line ends it; the map fits each part.
    let part = $state<[number, number] | null>(null);
    $effect(() => { void visualRoute; part = null; });
    $effect(() => {
        const fit = part;
        untrack(() => { if (fit) map?.fitCoordinates(routeSlice(coordinates, fit[0], fit[1]), true); });
    });
    let sideWidth = $state(360);
    let profileHeight = $state(260);
    let profileOpen = $state(true);
    // The drawer without a profile opens only on request, so it keeps the rider's choice for the route.
    let emptyOpen = $state(false);
    let viewportHeight = $state(900);
    let viewportWidth = $state(1200);
    let mapHeight = $state(600);
    let map = $state<PlannerMap>();
    // The plan came from the Explore page; "Find another route" gives this state back to it.
    let exploreBack = $state.raw<unknown>(undefined);
    const bike = $derived<BikeType>(trip.bike ?? 'touring');
    const activityNetwork = $derived(bike === 'hiking' ? 'hiking' : bike === 'mtb' ? 'mtb' : 'cycling');
    /** A shown route network is the one of the activity; a network that is off stays off. */
    function showActivityNetwork() {
        if (mapOverlays.network !== 'none') mapOverlays = { ...mapOverlays, network: activityNetwork };
    }
    $effect(() => { void activityNetwork; untrack(showActivityNetwork); });

    const minProfile = $derived(viewportWidth <= 760 && dataLayer?.strip ? 320 : DRAWER_MIN);
    const maxProfile = $derived(Math.max(minProfile, Math.min(340, viewportHeight - 400)));
    const hasProfile = $derived(!!visualRoute);
    const drawerOpen = $derived(hasProfile ? profileOpen : emptyOpen);
    const drawerHeight = $derived(drawerOpen ? Math.min(Math.max(profileHeight, minProfile), maxProfile) : DRAWER_CLOSED);
    const maxSide = $derived(Math.max(320, Math.min(460, viewportWidth - 540)));
    function resizeProfile(value: number, byKey: boolean) {
        const next = resizeDrawer(drawerHeight, value, byKey);
        const open = next > DRAWER_CLOSED;
        if (hasProfile) profileOpen = open; else emptyOpen = open;
        if (open) profileHeight = next;
    }
    const multi = $derived(trip.mode !== 'route');
    // The days of the trip without its suggested nights. The towns are searched around their ends, and the suggestions
    // then move them. A drag preview changes the line but not the trip, so they follow the line of the trip.
    const evenDays = $derived(planView(trip, session.line).days);
    // An open day lists the towns of its night until that night is pinned; the ladder lists the night the rider opened.
    const pickerNight = $derived(!multi || !currentRoute || searching || list !== 'plan' ? null
        : expandedDay !== null ? (night < evenDays.length && (!evenDays[night - 1]?.pinned || choosingNight === night) ? night : null)
        : choosingNight !== null && choosingNight < evenDays.length ? choosingNight : null);
    // Towns are searched for every night still to choose, so the ladder can suggest a place for each.
    const stopWindows = $derived(multi ? nightWindows(trip, session.line, pickerNight) : {});
    const routeKey = $derived(routingKey(trip));
    $effect(() => {
        const stretches = Object.values(stopWindows), route = routeKey, context = searchContext;
        if (!session.dragging) untrack(() => stopFinder.want(route, context, stretches));
    });
    const nightStops = (night: number) => stopWindows[night] ? stopFinder.found[stretchKey(routeKey, stopWindows[night])] : undefined;
    const readyStops = (night: number): Stop[] => { const found = nightStops(night); return found?.state === 'ready' ? found.stops : []; };
    const proposals = $derived(multi ? proposeNights(trip, session.line, readyStops) : new Map<number, Proposal>());
    const campsites = $derived([...proposals].flatMap(([night, { place }]) => place && stayKind(place.place.placeKind) === 'camp' ? [{ night, place: place.place }] : []));
    const planned = $derived(withProposals(shown.trip, shown.line, proposals));
    const view = $derived(planView(planned, shown.line));
    const coordinates = $derived(view.coordinates);
    const lengths = $derived(cumulative(coordinates));
    const total = $derived(view.total);
    // The route list follows the points of the trip, also while their new line is calculated.
    const stops = $derived(session.stale ? planView(trip, undefined).stops : view.stops);
    const visualStops = $derived(view.stops);
    // A suggested night is not a day end the rider moved.
    const days = $derived(view.days.map(day => day.split && trip.splits?.[day.number] === undefined ? { ...day, split: false } : day));
    const itinerary = $derived(view.itinerary);
    const focusedDay = $derived(multi && list === 'plan' && !searching ? itinerary.find(d => !d.rest && d.ridingNumber === expandedDay) ?? null : null);
    const dayLabels = $derived(Object.fromEntries(itinerary.filter(d => !d.rest).map(d => [d.ridingNumber, d.number])));
    const rests = $derived(itinerary.filter(d => d.rest).map(d => ({ after: d.ridingNumber, name: planned.restNames?.[d.restIndex!], town: d.pinned?.label ?? proposals.get(d.ridingNumber)?.stop.name ?? '' })));
    const queryPlan = $derived(searchPlan(trip, view));
    const searchContext = $derived<SearchContext>({ view: viewBounds, here, pointing, startDate: trip.startDate, plan: queryPlan });
    const conflicts = $derived(multi ? nightOrderConflicts(shown.trip, shown.line) : []);
    const closures = $derived(closureStretches(visualRoute));
    const alpine = $derived(alpineStretches(visualRoute));
    const gaps = $derived(visualRoute ? routeGaps(visualStops, coordinates) : []);
    // A new route makes the shown stretch stale, so the map no longer marks it.
    let shownStretch = $state.raw<Stretch | null>(null);
    const shownIndex = (stretches: Stretch[]) => shownStretch ? stretches.indexOf(shownStretch) : -1;
    function showNext(stretches: Stretch[]) {
        shownStretch = stretches[shownIndex(stretches) + 1] ?? null;
        if (shownStretch) map?.fitCoordinates(shownStretch.coordinates);
    }
    const stepLabel = (stretches: Stretch[]) => shownIndex(stretches) < 0 ? 'Show' : shownIndex(stretches) + 1 < stretches.length ? 'Next' : 'Hide';
    // The ladder and an open day light up the stretch whose towns they list.
    const nightBand = $derived.by(() => {
        const window = pickerNight !== null ? stopWindows[pickerNight] : undefined;
        return window && total ? { from: window[0] / total, to: window[1] / total, blocked: false } : null;
    });
    const highlighted = $derived(nightBand ? routeSlice(coordinates, nightBand.from, nightBand.to)
        : shownStretch && (shownIndex(closures) >= 0 || shownIndex(alpine) >= 0) ? shownStretch.coordinates : []);
    const openStop = $derived(pickerNight !== null ? readyStops(pickerNight).find(stop => stop.id === stopId) : undefined);
    const routeHeights = $derived(currentRoute ? profileSamples(currentRoute) : []);
    const nights: NightChoices = {
        get choosing() { return pickerNight; },
        get stopId() { return stopId; },
        found: nightStops,
        window: night => stopWindows[night] ?? null,
        kinds: night => nightKinds(trip, night),
        setKinds: (night, kinds) => {
            const { [night]: _, ...others } = trip.nightStay ?? {}, stay = trip.stay ?? allStay;
            const same = !kinds || (kinds.length === stay.length && kinds.every(kind => stay.includes(kind)));
            commit({ ...trip, nightStay: same ? others : { ...others, [night]: kinds } }, same ? `Night ${dayLabels[night]} follows the trip`
                : `Night ${dayLabels[night]}: ${stayOptions.filter(option => kinds.includes(option.kind)).map(option => option.label.toLowerCase()).join(', ')}`);
        },
        release: night => { commit(releaseNight(trip, night), `Night ${dayLabels[night]} is a suggestion again`); stopChoosing(); },
        around: night => {
            if (proposals.has(night)) return proposals.get(night);
            const end = days[night - 1].to * total, kinds = nightKinds(trip, night);
            const stop = readyStops(night).filter(stop => suits(stop, kinds) && Math.abs(stop.km - end) <= 3).sort((a, b) => Math.abs(a.km - end) - Math.abs(b.km - end))[0];
            return stop && proposal(stop, kinds, end);
        },
        neighbours: night => {
            const end = nights.around(night)?.stop.km ?? days[night - 1].to * total, stops = readyStops(night).filter(stop => suits(stop, nightKinds(trip, night)));
            return [stops.filter(stop => stop.km < end - .5).at(-1), stops.find(stop => stop.km > end + .5)];
        },
        get campsites() { return campsites.length; },
        keepCampsites: () => keepCampsites(),
        date: night => trip.startDate ? addDays(trip.startDate, (dayLabels[night] ?? night) - 1) : undefined,
        elevationAt: km => total ? profileHeightAt(km / total, routeHeights) : null,
        // The chosen night is the one a place callout and a map pick end, also in the ladder.
        choose: number => { choosingNight = number; stopId = null; clearSelection(); if (number !== null) { night = number; showNightArea(number); } },
        open: id => { stopId = id; clearSelection(); const stop = pickerNight !== null ? readyStops(pickerNight).find(s => s.id === id) : undefined; if (stop) map?.fitCoordinates(stop.sleep.map(p => p.place.coordinate)); },
        step: (night, direction) => {
            const stop = nights.neighbours(night)[direction < 0 ? 0 : 1];
            if (stop) endDayIn(night, stop);
        },
        endHere: (night, stop) => endDayIn(night, stop),
        stay: (night, place) => stayAt(night, place),
        selectPlace: place => selectPlace(place),
        pick: () => pickOnMap(),
        retry: night => { const window = stopWindows[night]; if (window) stopFinder.retry(stretchKey(routeKey, window)); },
    };
    // One stretch per leg and day, so the map can tell legs apart and colour days.
    const segments = $derived<MapSegment[]>(daySegments(view).map(segment => ({ ...segment, color: dayColor(segment.day, theme) })));
    const results = $derived((searchState.answer?.results ?? []).map(result => ({ place: asPlace(result) })));
    const visiblePlaces = $derived(searching ? results.map(result => result.place)
        : openStop ? openStop.sleep.map(p => p.place)
        : pickerNight !== null ? readyStops(pickerNight).flatMap(stop => stop.sleep.map(p => p.place)) : []);
    const selectedPoint = $derived(trip.points.find(p => p.id === selectedId));
    const shownPlace = $derived((mapPlace?.id === selectedId ? mapPlace : undefined) ?? visiblePlaces.find(p => p.id === selectedId));
    // List searches leave out details such as the website and phone; they load per place id, and null means search has none.
    let details = $state.raw<Record<string, SearchPlace | null>>({});
    let detailsError = $state('');
    const detailsId = $derived(shownPlace?.id ?? selectedPoint?.placeId);
    $effect(() => {
        // A new selection of the same place makes a new `mapPlace`, so a failed request runs again.
        const id = detailsId;
        void mapPlace;
        detailsError = '';
        if (!id || !placeSource(id) || untrack(() => id in details)) return;
        const coordinate = untrack(() => (shownPlace ?? selectedPoint)!.coordinate);
        const abort = new AbortController();
        placeDetails(id, coordinate, abort.signal).then(found => {
            if (!abort.signal.aborted) details = { ...details, [id]: found ?? null };
        }).catch(() => { if (!abort.signal.aborted) detailsError = 'Place details are unavailable. Try selecting the place again.'; });
        return () => abort.abort();
    });
    const selectedPlace = $derived(shownPlace && withDetails(shownPlace, details[shownPlace.id]));
    // A stop made from a mapped place shows that place, by its kind until its details arrive.
    const pointPlace = $derived.by(() => {
        const point = selectedPoint;
        return point?.placeId && point.placeKind ? withDetails({ ...point, id: point.placeId, kind: 'place', category: category(point.placeKind), description: '' },
            details[point.placeId]) : undefined;
    });
    const previewCoordinate = $derived(selectedId === 'pending' ? pending : selectedPlace?.coordinate ?? null);
    const mapPoints = $derived.by(() => {
        const pins: MapPoint[] = trip.points.map(p => ({
            ...p,
            kind: !multi && p.kind === 'night' ? 'waypoint' : p.kind,
            color: multi && p.kind === 'night' ? dayColor(p.night!, theme) : undefined,
            markerLabel: multi && p.kind === 'night' ? String(dayLabels[p.night!]) : undefined,
            // A night at a place to sleep shows its kind instead of its number.
            category: multi && p.kind === 'night' ? sleepCategory(p.placeKind) : undefined,
            fixed: p.kind === 'night',
        }));
        const places = [...new Map([...visiblePlaces, ...(selectedPlace ? [selectedPlace] : [])].map(place => [place.id, place])).values()];
        for (const p of places) {
            if (p.id === selectedId || !pins.some(pin => pin.coordinate[0] === p.coordinate[0] && pin.coordinate[1] === p.coordinate[1])) pins.push({ ...p });
        }
        if (pending) pins.push({ id: 'pending', coordinate: pending, label: 'Overnight spot', kind: 'place', appearance: 'suggested' });
        // Day ends come last, so they stay on top of a candidate place at the same spot and can be dragged.
        for (const day of multi && visualRoute ? days.slice(0, -1) : []) {
            if (day.pinned || trip.points.some(point => point.kind === 'night' && point.night === day.number)) continue;
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

    let lineSamples = $state.raw<{ layer: DataLayer; line: RoutingLine; samples: unknown } | null>(null);
    $effect(() => {
        const layer = dataLayer, line = visualRoute;
        if (!layer || !line || untrack(() => lineSamples?.layer === layer && lineSamples.line === line)) return;
        const abort = new AbortController();
        layer.sample(line, abort.signal, { date: shownDate, theme }).then(samples => { if (!abort.signal.aborted) lineSamples = { layer, line, samples }; }, () => {});
        return () => abort.abort();
    });
    const routeSamples = $derived(dataLayer && visualRoute && lineSamples?.layer === dataLayer && lineSamples.line === visualRoute ? lineSamples.samples : null);
    const layerYear = $derived(dataLayer?.year?.(routeSamples, { date: shownDate, theme }));
    // The night after day n falls n − 1 calendar days after the layer date.
    const layerNotes = $derived.by(() => {
        if (!multi || !dataLayer?.nights || !routeSamples || !visualRoute) return [];
        const points = profileSamples(visualRoute);
        const ends = days.slice(0, -1);
        const stops = ends.map(day => ({ index: Math.min(points.length - 1, sampleIndex(points, day.to)), date: addDays(shownDate, (dayLabels[day.number] ?? day.number) - 1) }));
        return dataLayer.nights(routeSamples, stops).flatMap((text, i) => text ? [{ coordinate: coordinateAt(coordinates, ends[i].to), text }] : []);
    });
    // A map click with a layer shown adds the years at that spot to the callout.
    let pinSamples = $state.raw<{ layer: DataLayer; at: Coordinate; samples: unknown } | null>(null);
    $effect(() => {
        const layer = dataLayer, at = calloutKind === 'add' ? spot?.coordinate : undefined;
        if (!layer || !at) return;
        const abort = new AbortController();
        layer.sample({ coordinates: [at], elevation: [null] }, abort.signal, { date: shownDate, theme }).then(samples => { if (!abort.signal.aborted) pinSamples = { layer, at, samples }; }, () => {});
        return () => abort.abort();
    });
    const pinChart = $derived(dataLayer && calloutKind === 'add' && pinSamples?.layer === dataLayer && pinSamples.at === spot?.coordinate ? dataLayer.chart(pinSamples.samples, 0, { date: shownDate, theme }) : null);

    // A query runs again when anything it sends other than the map view changes.
    $effect(() => { void [queryPlan, here, pointing, trip.startDate]; untrack(() => searchRevision++); });

    onMount(() => {
        void session.start(library);
        beforeReload(() => session.saved());
        return () => session.close();
    });

    /** Resets the view after another plan replaced the trip. */
    function opened() {
        exploreBack = undefined;
        statusUndo = null;
        clearSelection();
        exitSearch();
        pointing = undefined;
        showRoute = true;
        night = 1;
        expandedDay = null;
        visibleRange = null;
        hoverProgress = null;
        part = null;
        fitPlan = true;
        list = 'plan';
        firstOptions = false;
        nameVisits(session.trip);
    }

    function nameVisits(trip: Trip) {
        trip.points.filter(point => point.autoLabel).forEach(point => void session.nameVisit(point));
    }

    // PROTOTYPE: `?bar=C` puts the route preferences at the head of the side panel.
    const protoBar = new URLSearchParams(location.search).get('bar');
    // PROTOTYPE: `?type=T1|T2|T3` selects a home for the choice between a one-day route and a multi-day trip.
    const protoType = new URLSearchParams(location.search).get('type');

    async function newOfMode(mode: Trip['mode']) {
        await newPlan();
        if (trip.mode !== mode) changeTrip({ mode }, mode === 'route' ? 'One-day route' : 'Multi-day trip');
    }

    async function openLibrary() {
        libraryOpen = true;
        await session.listPlans();
    }

    // `#/planner/ID` opens plan ID, and `#/planner/new` a new plan, at `history.state.place` when a page gives one. The
    // address then drops the id, so Back does not open it again.
    $effect(() => {
        const id = router.route === 'planner' ? router.id : undefined;
        if (!id || !session.ready) return;
        router.replace(ROUTES.planner);
        if (id === 'new') untrack(() => {
            const signed = history.state?.signed as { route: CatalogRecord; plan: RoutePlan; back: unknown } | undefined;
            if (!signed) return void newPlan().then(() => showStart(history.state?.place));
            // The entry keeps no route, so the planner never plans it twice.
            history.replaceState({ ...history.state, signed: undefined }, '');
            void planSignedRoute(signed.route, signed.plan, signed.back);
        });
        // The open plan keeps its Undo history and its map view.
        else if (id !== untrack(() => session.plan.id)) untrack(() => void openPlan(id));
    });

    /** A box of some size fits the map; a point shows at the zoom of a place. */
    function showStart(place: { lon: number; lat: number; box?: [number, number, number, number] } | undefined) {
        if (!place) return;
        const [west, south, east, north] = place.box ?? [place.lon, place.lat, place.lon, place.lat];
        if (west < east && south < north) map?.fitCoordinates([[west, south], [east, north]]);
        else map?.showPlace([place.lon, place.lat], 12);
    }

    function ask(mode: SaveMode): Promise<SaveAnswer> {
        return new Promise(answer => asking = { mode, answer });
    }

    /** Asks before another route replaces a route with changes: "Save" saves it, "Discard" leaves its draft. False at
     * "Cancel" or a failed save. */
    async function leave(): Promise<boolean> {
        // A save that fails makes the route changed again.
        await session.whenSaved();
        if (!session.changed || !session.canSave) return true;
        const answer = await ask('leave');
        if (answer === false) return true;
        // An edit or an Undo during the save leaves the route changed, so it asks again.
        return answer !== null && await session.save(answer) && leave();
    }

    async function save() {
        const name = session.stored ? '' : await ask('save');
        if (typeof name === 'string' && await session.save(name)) message = 'Route saved';
    }

    async function saveCopy() {
        const name = await ask('copy');
        if (!name || !await session.saveCopy(name)) return;
        message = 'Copy saved';
        // The trip bar of the copy is a new one.
        await focusMore();
    }

    async function renameRoute() {
        const name = await ask('rename');
        if (name) await session.rename(session.plan, name);
    }

    async function discard() {
        if (!await session.discard()) return;
        afterHistory('Changes discarded');
        statusUndo = undo;
    }

    async function openPlan(id: string) {
        if (!await leave()) return;
        if (!await session.open(id)) {
            if (session.libraryError) message = session.libraryError;
            return;
        }
        libraryOpen = false;
        message = 'Plan opened';
    }

    function deletePlan(plan: Plan) {
        if (window.confirm(`Delete “${plan.name || planTitle(plan.trip)}”? Download it first if you need a backup.`)) void session.remove(plan);
    }

    async function importFiles(files: File[]) {
        // GPX files ask at the choice of their line.
        if (!files.some(file => /\.gpx$/i.test(file.name)) && !await leave()) return;
        if (!libraryOpen) await openLibrary();
        await session.importFiles(files);
    }

    async function duplicate(plan: Plan) {
        if (await leave()) await session.duplicate(plan);
    }

    async function importGpx(roads: boolean) {
        if (!await leave()) return;
        const count = session.gpxLines!.length;
        if (roads) message = 'Planning on roads…';
        const failed = await session.importGpx(roads, coordinate => map?.placeName(coordinate));
        if (!failed) return;
        libraryOpen = false;
        message = failed.length === 1 ? `${failed[0]} could not be planned on roads, so it keeps the file's line`
            : failed.length ? `${failed.length} files could not be planned on roads, so they keep the file's line`
            : count > 1 ? `Trip imported · ${count} days` : 'Route imported';
    }

    function dragOver(event: DragEvent) {
        if (!event.dataTransfer?.types.includes('Files')) return;
        event.preventDefault();
        dropping = true;
    }

    function drop(event: DragEvent) {
        event.preventDefault();
        dropping = false;
        const files = [...event.dataTransfer?.files ?? []];
        if (files.length) void importFiles(files);
    }

    function commit(next: Trip, description: string) {
        session.commit(next);
        afterEdit(description);
    }

    // An edit ends a comparison, whose rows would be of the plan before it.
    function afterEdit(description: string) {
        list = 'plan';
        if (!hasEndpoints) { expandedDay = null; hoverProgress = null; }
        message = description;
        statusUndo = null;
    }

    function edit(change: Partial<Trip>, description: string) {
        commit({ ...trip, ...change }, description);
    }

    /** Clears the selection and keeps the map mode: a search answer can arrive while the rider draws or picks. */
    function deselect() {
        hoveredId = null;
        selectedId = null;
        pending = null;
        pendingSource = null;
        spot = null;
    }

    function clearSelection() {
        deselect();
        mapMode = null;
    }

    /** Keeps the day the rider works on after the number of days changed. */
    function clampNight() {
        night = Math.max(1, Math.min(night, planView(trip, session.line).days.length));
        if (expandedDay !== null) expandedDay = night;
    }

    function afterHistory(description: string) {
        clearSelection();
        clampNight();
        message = description;
        statusUndo = null;
        nameVisits(trip);
    }

    /** Opens the day a point lies in and lights its row for a moment. */
    function reveal(id: string, ridingDay: number | null) {
        if (ridingDay !== null && multi) {
            night = ridingDay;
            expandedDay = ridingDay;
            list = 'plan';
            stopChoosing();
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
        if (session.undo()) afterHistory('Change undone');
    }

    function redo() {
        if (session.redo()) afterHistory('Change restored');
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
        stopChoosing();
        exitSearch();
        clearSelection();
        const day = days[riding - 1];
        map?.fitCoordinates(routeSlice(coordinates, Math.max(0, day.from - .04), Math.min(1, day.to + .04)));
    }

    /** Opens a night in the day list, with its towns. */
    function openNight(number: number) {
        expandedDay = null;
        list = 'plan';
        exitSearch();
        nights.choose(number);
    }

    function showAllDays() {
        expandedDay = null;
        stopChoosing();
        clearSelection();
        map?.fitRoute();
    }

    function showDayEnd(day: Day) {
        if (day.pinned) inspectPoint(day.pinned);
        else if (day.number === days.length) inspectPoint(orderedRoutePoints(trip).at(-1)!);
        else showNightArea(day.number);
    }

    function selectPlace(place: Place) {
        choosePlace(place);
        map?.showPlace(place.coordinate, 12);
    }

    /** Opens a mapped place without changing the search area. */
    function choosePlace(place: Place) {
        clearSelection();
        mapPlace = { ...place };
        selectedId = place.id;
    }

    function selectPoint(id: string) {
        if (id === 'pending') return;
        const place = visiblePlaces.find(p => p.id === id) ?? (mapPlace?.id === id ? mapPlace : undefined);
        if (place) { choosePlace(place); return; }
        clearSelection();
        selectedId = id;
        const number = id.startsWith('dayend-') ? Number(id.slice('dayend-'.length)) : trip.points.find(p => p.id === id)?.night;
        if (number && multi) {
            night = number;
            expandedDay = number;
            list = 'plan';
            stopChoosing();
            exitSearch();
        }
    }

    function inspectPoint(point: RoutePoint) {
        selectPoint(point.id);
        map?.showPlace(point.coordinate, 12);
    }

    function pickOnMap() {
        clearSelection();
        mapMode = { kind: 'night' };
    }

    function emptyClick(coordinate: Coordinate) {
        if (mapMode?.kind === 'transfer') {
            void arrive(coordinate);
        } else if (picking) {
            mapMode = null;
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
            mapMode = { kind: 'draw', legEndId };
            return;
        }
        commit(setLegMode(trip, legEndId, mode), mode === 'straight' ? 'Leg set to a straight line' : mode === 'transfer' ? 'Leg set to a transfer' : 'Leg set to routed');
    }

    async function planSignedRoute(route: CatalogRecord, plan: RoutePlan, explore?: unknown) {
        if (!await leave()) return;
        if (!await session.planSignedRoute(route, plan, coordinate => map?.placeName(coordinate))) {
            message = `${route.name ?? route.ref} could not be planned. ${session.libraryError}`;
            return;
        }
        exploreBack = explore;
        message = `${route.name ?? route.ref} planned`;
        map?.fitCoordinates(plan.points);
    }

    function closeToStart() {
        clearSelection();
        commit(closeLoop(trip), 'Loop closed · the route returns to the start');
    }

    function insert(legEndId: string, coordinate: Coordinate, dragged = false) {
        clearSelection();
        commit((dragged ? dragPointOut : insertPoint)(trip, legEndId, coordinate), 'Shaping point inserted');
    }

    function drawn(legEndId: string, line: Coordinate[]) {
        mapMode = null;
        commit(setDrawnLeg(trip, legEndId, line), 'Leg drawn');
    }

    // A day end dropped within 2 km of a town with somewhere to sleep moves to that town.
    function moveDayEnd(number: number, progress: number) {
        const km = progress * total;
        const kinds = nightKinds(trip, number);
        const town = readyStops(number).filter(stop => suits(stop, kinds) && Math.abs(stop.km - km) <= 2).sort((a, b) => Math.abs(a.km - km) - Math.abs(b.km - km))[0];
        commit(setSplit(trip, session.line, number, town && total ? proposal(town, kinds, km).km / total : progress), town ? `Day ${dayLabels[number]} ends in ${town.name}` : 'Day end moved');
    }

    // A kept night moves to the town and stays kept.
    function endDayIn(number: number, stop: Stop) {
        if (!total) return;
        const end = proposal(stop, nightKinds(trip, number)), place = end.place?.place;
        const next = days[number - 1].pinned
            ? pinNight(trip, session.line, number, place?.coordinate ?? coordinateAt(coordinates, end.km / total), place?.label ?? stop.name, undefined, place ? placeRef(place) : {})
            : setSplit(trip, session.line, number, end.km / total);
        commit(next, `Day ${dayLabels[number]} ends in ${stop.name}`);
        stopChoosing();
    }

    function reorder(id: string, offset: number) {
        const next = reorderPoint(trip, id, offset);
        if (next !== trip) commit(next, 'Stops reordered · changed legs follow roads');
        else if (trip.points.some(p => p.kind === 'night' && trainAfter(trip, p.night!))) message = trainNightText;
    }

    function startTransfer(number: number) {
        clearSelection();
        mapMode = { kind: 'transfer', night: number };
    }

    /** The train or bus after the night of the transfer pick arrives at `coordinate`, named after its place or town. */
    async function arrive(coordinate: Coordinate, name?: string) {
        if (mapMode?.kind !== 'transfer') return;
        const number = mapMode.night, before = trip;
        mapMode = null;
        const label = name ?? map?.placeName(coordinate) ?? await townName(coordinate) ?? coordinateName(coordinate);
        if (trip !== before) {
            message = 'The plan changed, so the train or bus was not added. Try again.';
            return;
        }
        const around = days[number - 1].pinned ? undefined : nights.around(number);
        // A suggested night stays where the day list shows it: at its campsite, else at its town.
        const suggested = around && proposals.get(number) === around ? withProposals(trip, session.line, new Map([[number, around]])) : trip;
        const stay = around && (around.place ? { coordinate: around.place.place.coordinate, label: around.place.place.label, place: placeRef(around.place.place) } : { label: around.stop.name });
        let next: Trip;
        try {
            next = addTransfer(suggested, session.line, number, coordinate, label, stay);
        } catch (error) {
            message = (error as Error).message;
            return;
        }
        if (next === suggested) return;
        const skipped = trip.routeOrder.filter(id => !next.routeOrder.includes(id)).length;
        commit(next, `Train or bus to ${label}${skipped ? ` · ${skipped} ${skipped === 1 ? 'point' : 'points'} skipped` : ''}`);
    }

    function keepCampsites() {
        const camps = campsites.map(({ night, place }) => ({ night, coordinate: place.coordinate, label: place.label, place: placeRef(place) }));
        const next = pinNights(trip, session.line, camps);
        if (next !== trip) commit(next, camps.length === 1 ? `Night ${dayLabels[camps[0].night]} at ${camps[0].label}` : 'Suggested campsites kept');
    }

    function stopChoosing() {
        choosingNight = null;
        stopId = null;
    }

    function stayAt(sleepDay: number, place: Place) {
        // A place already on the route as a stop becomes the night, not a second point beside it.
        const source = trip.points.find(p => p.kind !== 'night' && p.coordinate[0] === place.coordinate[0] && p.coordinate[1] === place.coordinate[1]);
        const next = pinNight(trip, session.line, sleepDay, place.coordinate, place.label, source?.id, placeRef(place));
        if (next === trip) { if (trainAfter(trip, sleepDay)) message = trainNightText; return; }
        commit(next, `Night ${dayLabels[sleepDay]} at ${place.label}`);
        stopChoosing();
        clearSelection();
        if (expandedDay !== null) reveal(`night-${sleepDay}`, sleepDay);
    }

    function showNightArea(number: number) {
        const window = stopWindows[number];
        if (window && total) map?.fitCoordinates(routeSlice(coordinates, Math.max(0, window[0] / total - .02), Math.min(1, window[1] / total + .02)));
    }

    function stayHere(sleepDay: number) {
        if (!previewCoordinate) return;
        const coordinate: Coordinate = [...previewCoordinate];
        const source = trip.points.find(p => p.id === pendingSource);
        const label = selectedPlace?.label ?? (source && !['via', 'pass'].includes(source.kind) ? source.label : 'Overnight spot');
        const current = trip;
        const place = selectedPlace ? placeRef(selectedPlace) : { placeKind: source?.placeKind, placeId: source?.placeId };
        const next = pinNight(current, session.line, sleepDay, coordinate, label, pendingSource ?? undefined, place);
        if (next === current) { if (trainAfter(current, sleepDay)) message = trainNightText; return; }
        commit(next, 'Overnight pinned');
        clearSelection();
        reveal(`night-${sleepDay}`, sleepDay);
    }

    const placeRef = (place: Place): PlaceRef => ({ placeKind: place.placeKind, placeId: place.id });

    function newPoint(coordinate: Coordinate, kind: EditableKind, label?: string): RoutePoint {
        const autoLabel = kind === 'waypoint' && label === undefined;
        return { id: crypto.randomUUID(), coordinate: [...coordinate], label: label ?? (autoLabel ? coordinateName(coordinate) : defaultLabels[kind]),
            autoLabel: autoLabel || undefined, kind };
    }

    const added: Partial<Record<PointKind, string>> = { via: 'Shaping point added', waypoint: 'Visit added', night: 'Overnight pinned', marker: 'Marker added', pass: 'Pass added' };

    function addPoint(coordinate: Coordinate, kind: EditableKind) {
        const point = newPoint(coordinate, kind);
        commit(addClickedPoint(trip, session.line, point), added[kind]!);
        clearSelection();
        selectedId = point.id;
        if (kind !== 'via') reveal(point.id, dayOf(point.coordinate));
        void session.nameVisit(point);
    }

    function addVisit(place: Place) {
        const point = { ...newPoint(place.coordinate, 'waypoint', place.label), ...placeRef(place) };
        commit(addPointNear(trip, session.line, point), 'Visit added');
        clearSelection();
        selectedId = point.id;
        reveal(point.id, dayOf(point.coordinate));
    }

    function movePoint(id: string, coordinate: Coordinate) {
        session.movePoint(id, coordinate);
        afterEdit('Point moved');
        const point = trip.points.find(p => p.id === id);
        if (point) void session.nameVisit(point);
    }

    function removePoint() {
        if (!selectedPoint) return;
        const train = selectedPoint.kind === 'night' && trainAfter(trip, selectedPoint.night!);
        commit(removeRoutePoint(trip, selectedPoint.id, coordinate => map?.placeName(coordinate)), train ? 'Night and its train or bus removed' : 'Point removed');
        pointing = undefined;
        clearSelection();
    }

    function chooseEndpoint(kind: 'start' | 'finish') {
        const coordinate = spot?.coordinate ?? selectedPlace?.coordinate;
        if (!coordinate) return;
        const first = !hasEndpoints;
        commit(setEndpoint(trip, kind, coordinate, selectedPlace?.label), kind === 'start' ? 'Start set' : 'Finish set');
        showRoute = true;
        clearSelection();
        exitSearch();
        list = 'plan';
        if (first && hasEndpoints) firstOptions = true;
    }

    async function newPlan() {
        if (!await leave() || !await session.newPlan()) return;
        message = 'New plan';
        libraryOpen = false;
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
        const next = replacePoint(trip, session.line, point.id, { ...point, id, kind, night: undefined, label, autoLabel: autoLabel || undefined,
            ...kind === 'via' && { placeId: undefined, placeKind: undefined },
            anchor: kind === 'detour' ? nearestOnLine(coordinates, point.coordinate).at : undefined });
        commit(next, 'Point type updated');
        selectedId = id;
        void session.nameVisit(next.points.find(p => p.id === id)!);
    }

    function changeTrip(change: Partial<Trip>, description: string) {
        if (change.mode === 'trip' && trip.mode === 'route') commit(routeToTrip(trip, session.line), description);
        else edit(change, description);
        if (!('mode' in change)) return;
        night = 1;
        expandedDay = null;
        list = 'plan';
        exitSearch();
        clearSelection();
    }

    function changePlan(next: Trip, description: string) {
        commit(next, description);
        clampNight();
    }

    function locate() {
        if (!navigator.geolocation) { message = 'This browser cannot read your location.'; return; }
        navigator.geolocation.getCurrentPosition(position => {
            here = [position.coords.longitude, position.coords.latitude];
            message = 'Location set for search';
        }, () => { message = 'Location unavailable. Allow location access or use a named place.'; }, { timeout: 10000 });
    }

    // The build copies the file, so the sample needs no search service.
    const sampleRoute = new URL('../../../../../fixtures/sources/route-import/komoot-schwarzwald.gpx', import.meta.url);
    async function loadSearchSample() {
        try {
            const response = await fetch(sampleRoute);
            if (!response.ok) throw new Error('The example route is unavailable. Retry shortly.');
            const sample = importedTrip(trip, readTracks([{ name: 'Black Forest', text: await response.text() }]));
            const points = sample.points.map(p => p.kind === 'start' || p.kind === 'finish' ? { ...p, label: `Black Forest ${p.kind}` } : p);
            exploreBack = undefined;
            commit({ ...sample, points, mode: 'trip' }, 'Black Forest test route loaded · three provisional days');
            exitSearch(); clearSelection(); pointing = undefined; fitPlan = true;
        } catch (error) { message = (error as Error).message; }
    }

    async function applySearch() {
        const answer = $state.snapshot(searchState.answer);
        if (!answer?.changes || applyingQuery) return;
        const before = trip;
        applyingQuery = true; queryApplyError = '';
        try {
            const next = await applyQueryChanges(before, session.line, answer.changes, plan => session.calculate(plan, new AbortController().signal));
            if (JSON.stringify(answer.changes) !== JSON.stringify(searchState.answer?.changes) || before !== trip) throw new Error('The plan changed. Review the search again.');
            if (answer.changes.some(change => change.op === 'route')) exploreBack = undefined;
            session.commit(next.trip, next.line);
            afterEdit(answer.description ?? 'Query applied');
            exitSearch(); clearSelection();
        } catch (error) { queryApplyError = (error as Error).message; }
        finally { applyingQuery = false; }
    }

    function nameRest(index: number, name: string) {
        const names = (trip.restAfter ?? []).map((_, i) => i === index ? name.trim() : trip.restNames?.[i] ?? '');
        edit({ restNames: names }, 'Rest day named');
    }

    async function compare() {
        if (!await session.listPlans()) {
            message = session.libraryError;
            return;
        }
        clearSelection();
        exitSearch();
        comparing = true;
    }

    /** Back in the planner, the menu button that opened the compare screen takes the focus. */
    async function endCompare() {
        comparing = false;
        await focusMore();
    }

    async function focusMore() {
        await tick();
        document.querySelector<HTMLElement>('.trip-bar .menu > button')?.focus();
    }

    function keyboard(event: KeyboardEvent) {
        if (!active || comparing || asking || (event.target as HTMLElement)?.closest('input, select, textarea')) return;
        if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'z') {
            event.preventDefault();
            if (event.shiftKey) redo();
            else undo();
        }
        // An open menu takes Escape first.
        if (event.key === 'Escape' && !event.defaultPrevented) {
            session.previewOption(null);
            // A map selection or mode closes before the part, which `Lanes` clears unless the default is prevented.
            if (selectedId || mapMode) event.preventDefault();
            else if (!part && focusedDay) showAllDays();
            clearSelection();
        }
    }

    // TrailView opens at a point where the rider drops the binoculars or picks on the map, and grows from that point of the viewport.
    // The map keeps its view inside the release bounds, so every point on it is in the TrailView area.
    let trailView = $state.raw<{ at: Coordinate; origin: [number, number]; near: Near } | null>(null);
    let trailViewControl = $state<TrailViewControl>();

    function openTrailView(at: Coordinate, origin: [number, number]) {
        if (mapMode?.kind === 'look') mapMode = null;
        const nights = multi ? trip.points.filter(p => p.kind === 'night').map(p => ({
            coordinate: p.coordinate, color: dayColor(p.night!), category: sleepCategory(p.placeKind), label: String(dayLabels[p.night!] ?? p.night),
        })) : [];
        trailView = { at, origin, near: { metresPerPx: map?.metresPerPixel(at[1]) ?? 0, route: showRoute ? coordinates : [], overlay: { colours: routeColours(view.line, days), nights, profile: profileSamples(view.line) } } };
    }

    /** The binoculars without a drag turn the pick on the map on and off. */
    function binoculars() {
        if (mapMode?.kind === 'look') mapMode = null;
        else {
            clearSelection();
            mapMode = { kind: 'look' };
        }
    }

</script>

<svelte:window onkeydown={keyboard} bind:innerHeight={viewportHeight} bind:innerWidth={viewportWidth} />

<div class="planner-shell" style:--side-width={`${Math.min(sideWidth, maxSide)}px`}>
    {#if comparing}
    <CompareScreen routes={session.plans.filter(plan => endpointsChosen(plan.trip))} first={session.stored ? session.plan.id : undefined} changed={session.changed}
        lines={session.foundLines} {theme} onBack={endCompare} onOpen={async id => { comparing = false; await openPlan(id); await endCompare(); }}
        onFindLines={(trips, retry) => session.findLines(trips, retry)} />
    {:else}
    {#key planId}
    <TripBar
        {trip} name={session.plan.name} stored={session.stored} changed={session.changed} canSave={session.canSave} ready={session.ready && !session.busy}
        canUndo={session.canUndo} canRedo={session.canRedo} onNew={newPlan} onChange={changeTrip} onUndo={undo} onRedo={redo} onLibrary={openLibrary}
        onSave={save} onRename={renameRoute} more={protoType === 'T3' ? [
            { label: 'New route', disabled: !trip.points.length, run: () => newOfMode('route') },
            { label: 'New multi-day trip', disabled: !trip.points.length, run: () => newOfMode('trip') },
            { label: multi ? 'Change to one-day route' : 'Change to multi-day trip', rule: true, disabled: !trip.points.length,
                run: () => changeTrip({ mode: multi ? 'route' : 'trip' }, multi ? 'One-day route' : 'Multi-day trip') },
            { label: 'Open route…', rule: true, run: openLibrary },
            { label: 'Save a copy…', rule: true, disabled: !trip.points.length, run: saveCopy },
            { label: 'Discard changes', disabled: !session.changed, run: discard },
            { label: 'Compare routes…', rule: true, run: compare },
        ] : protoType ? [
            { label: 'Open route…', run: openLibrary },
            { label: 'Save a copy…', rule: true, disabled: !trip.points.length, run: saveCopy },
            { label: 'Discard changes', disabled: !session.changed, run: discard },
            { label: 'Compare routes…', rule: true, run: compare },
        ] : [
            { label: 'Save a copy…', disabled: !trip.points.length, run: saveCopy },
            { label: 'Discard changes', disabled: !session.changed, run: discard },
            { label: 'Compare routes…', rule: true, run: compare },
        ]}
    />
    {/key}
    <main aria-busy={!session.ready}>
        {#if libraryOpen}
            <LibraryPanel plans={session.plans} unreadable={session.unreadable} onDeleteUnreadable={() => { if (window.confirm('Delete the saved plans that cannot be read? This cannot be undone.')) void session.removeUnreadable(); }} activeId={planId} busy={session.busy} error={session.libraryError}
                onClose={() => libraryOpen = false} onOpen={plan => openPlan(plan.id)} onRename={(plan, name) => session.rename(plan, name)} onDuplicate={duplicate}
                onDelete={deletePlan} onImport={importFiles} gpxNames={session.gpxLines?.map(line => line.name) ?? null}
                onGpx={roads => roads === null ? session.gpxLines = null : importGpx(roads)} onDownload={plan => session.download(plan)} />
        {/if}
        <aside class="planner-pane" aria-label="Trip planning" inert={!session.ready || session.busy}>
            <Query bind:this={searchBox} bind:text={query} bind:searchState={searchState} context={searchContext} selection={calloutCoordinate ? { anchor: calloutCoordinate } : undefined} revision={searchRevision} viewRevision={searchViewRevision} onResults={coordinates => { deselect(); map?.fitSearchResults(coordinates); }} onSearch={() => { searching = true; queryApplyError = ''; }} onClear={clearSearch} onLocation={locate} onPointing={where => pointing = where} onSample={loadSearchSample} />
            {#if protoBar === 'C' && !searching}<div class="proto-prefs"><RidePrefs {trip} onChange={changeTrip} inline noType={!!protoType} /></div>{/if}
            {#if exploreBack && hasEndpoints && !searching}
                <button type="button" class="back-to-routes" onclick={() => router.go(ROUTES.explore, { explore: exploreBack })}><Icon name="back" size={15} />Find another route</button>
            {/if}
            {#if searching}
                <div class="pane-scroll">
                    <QueryResults attribution={config.attribution} state={searchState} {selectedId} {hoveredId} onHover={(id) => hoveredId = id} onSelect={selectPlace} applying={applyingQuery} applyError={queryApplyError} onApply={applySearch} onMore={() => searchBox?.more()} onRetry={() => searchBox?.retry()} onStretch={line => { pointing = {along:{ref:'km',from:{value:nearestProgress(coordinates,line[0])*total,unit:'km'},to:{value:nearestProgress(coordinates,line.at(-1)!)*total,unit:'km'}}}; map?.fitCoordinates(line); }} />
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
                            {#if session.canUndo}<button type="button" class="planner-action" onclick={undo}>Undo last change</button>{/if}
                            <button type="button" class="planner-action" onclick={() => session.retry()}>Retry routing</button>
                            <p>Move a point or choose another place in {config.name}.</p>
                        {/if}
                    </div>
                {/if}
                {#if !focusedDay && visualRoute}
                    <div class="trip-summary"><RouteStats distance={view.summary.distance} ascent={view.line?.unknownElevationKm === 0 ? view.summary.ascent : null} descent={view.line?.unknownElevationKm === 0 ? view.summary.descent : null} walking={shown.trip.bike === 'hiking'} hours={view.line ? view.summary.hours : null} /></div>
                {/if}
                {#if !focusedDay && gaps.length}
                    <p class="closure-note"><Icon name="pin" size={15} /><span>{gapNote(gaps)}</span></p>
                {/if}
                {#if !focusedDay && closures.length}
                    <p class="closure-note"><Icon name="calendar" size={15} /><span>{closureNote(closures)}</span><button type="button" class="planner-action quiet" onclick={() => showNext(closures)}>{stepLabel(closures)}</button></p>
                {/if}
                {#if !focusedDay && alpine.length}
                    <p class="closure-note"><Icon name="mountain" size={15} /><span>{alpineNote(alpine)}</span><button type="button" class="planner-action quiet" onclick={() => showNext(alpine)}>{stepLabel(alpine)}</button></p>
                {/if}
                {#if list !== 'plan'}
                    <header class="list-head">
                        <button type="button" class="planner-action quiet" onclick={() => list = 'plan'}><Icon name="back" size={15} />{multi ? 'Days' : 'Route'}</button>
                        <h2>Route options</h2>
                    </header>
                    <p class="list-lead">Compare other ways from start to finish, or keep your route.</p>
                {:else if visualRoute && !focusedDay}
                    <div class="list-links" bind:this={listLinks}>
                        <button type="button" data-list="ways" onclick={() => list = 'ways'}><Icon name="route" size={15} />Route options{optionsReason ? ` · ${optionsReason}` : ''}</button>
                    </div>
                {/if}
                <div class="pane-scroll" inert={multi && !currentRoute && routingStatus === 'Calculating route…'}>
                    {#if (protoType === 'T1' || protoType === 'T2') && list === 'plan' && !focusedDay}
                    <TypeProto variant={protoType} {trip} line={session.line} onChange={changeTrip} onPlan={changePlan} />
                {/if}
                    {#if list === 'ways'}
                        <CompareList label="Route options" rows={optionRows} status={options.state === 'ready' ? null : options.state === 'error' ? options.message : optionStatus[options.state]}
                            retry={options.state === 'error' ? () => session.findAlternatives() : undefined} applyLabel="Use this route" previewId={session.compared?.id ?? null}
                            note={optionRows.length < 2 ? 'No useful alternative to your route was found.' : 'Differences compare with your route.'} onPreview={id => session.previewOption(id)} onApply={useOptionRow} />
                    {:else if multi && visualRoute && (currentRoute || routingStatus === 'Calculating route…')}
                        <Itinerary
                            trip={planned} line={visualRoute} {itinerary} {days} {theme} {expandedDay} {nights} {conflicts} {selectedId} {revealId} {hoveredId} onHover={(id) => hoveredId = id}
                            onToggle={showDay} onOverview={showAllDays}
                            onInspect={inspectPoint}
                            onShowEnd={showDayEnd}
                            onShowConflict={([before, after]) => map?.fitCoordinates([before.coordinate, after.coordinate])}
                            onAddRest={(after) => commit(addRestDay(trip, after), `Rest day added after night ${dayLabels[after]}`)}
                            onTransfer={startTransfer}
                            onRemoveRest={(index) => commit(removeRestDay(trip, index), 'Rest day removed')}
                            onNameRest={nameRest}
                        />
                    {:else}
                        <RouteList {stops} loop={!!trip.loop} onLoop={multi ? undefined : closeToStart} {hoveredId} onHover={(id) => hoveredId = id} measured={!!currentRoute} onInspect={inspectPoint}
                            onReorder={reorder} />
                    {/if}
                </div>
            {/if}
        </aside>
        <Resize value={Math.min(sideWidth, maxSide)} min={320} max={maxSide} axis="x" label="Sidebar width" onResize={(value) => sideWidth = value} />
        <section inert={!session.ready || session.busy} class="geography" aria-label="Map and elevation" aria-busy={hasEndpoints && !currentRoute}>
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="map-area" bind:clientHeight={mapHeight} style:--map-height={`${mapHeight}px`} ondragover={dragOver} ondrop={drop}
                ondragleave={event => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) dropping = false; }}>
                <PlannerMap
                    bind:this={map} {part} {segments} gaps={gaps.map(gap => gap.coordinates)} {coordinates} points={mapPoints} {selectedId} {hoveredId} onPointHover={(id) => hoveredId = id} callout={calloutCoordinate} {drawing}
                    {theme} {hillshade} {contours} {mapOverlays} dataLayer={{ layers: dataLayers, shown: dataLayer, date: shownDate, notes: layerNotes }} bottomInset={dataLayer ? dateBarHeight + 34 : 0} accessMode={trip.bike === 'hiking' ? 'walking' : 'cycling'} {showRoute} compare={comparison && { line: comparison.option.line.coordinates, letter: comparison.letter }} {hoverProgress} highlightedCoordinates={highlighted} pickMode={picking || mapMode?.kind === 'transfer'} routingPackage={currentRoute?.package}
                    onPick={mapMode?.kind === 'look' ? openTrailView : null}
                    highlightedPlaceIds={searching ? results.map(result => result.place.id) : []}
                    placeKinds={placeChoice.kinds} highlightedKind={placeChoice.highlight}
                    onBounds={(bounds, preserveSearch) => { viewBounds = bounds; if (!preserveSearch) searchViewRevision++; }} onEmptyClick={emptyClick} onPointSelect={selectPoint} onPointMove={movePoint} onPointPreview={(id, coordinate) => session.previewMove(id, coordinate)} onDayEndDrag={moveDayEnd}
                    onLegClick={legClick} onInsert={(legEndId, coordinate) => insert(legEndId, coordinate, true)} onDrawn={drawn} onPlaceClick={place => mapMode?.kind === 'transfer' ? void arrive(place.coordinate, place.label) : choosePlace(place)}
                    onVisibleRange={(range) => visibleRange = range}
                >
                    {#snippet popup()}
                        {#if calloutKind}
                            {#key selectedId}
                                <MapCallout
                                    kind={calloutKind} trip={withProposals(trip, session.line, proposals)} line={session.line} {days} {dayLabels} {night} {legMode}
                                    onEndpoint={chooseEndpoint} point={selectedPoint} place={selectedPoint ? pointPlace : selectedPlace} coordinate={previewCoordinate} {detailsError}
                                    onClose={clearSelection}
                                    onAddHere={addHere}
                                    onLegMode={setLeg}
                                    onInsert={() => insert(spot!.legEndId!, spot!.coordinate)}
                                    onLoop={closeToStart}
                                    onLoopStart={() => { const { legEndId, coordinate } = spot!; clearSelection(); commit(startLoopHere(trip, legEndId!, coordinate, map?.placeName(coordinate)), 'Loop start moved'); }}
                                    onPick={pickOnMap}
                                    onChooseNight={() => { const number = night; clearSelection(); if (expandedDay === null) nights.choose(number); else choosingNight = number; }}
                                    onStay={stayHere}
                                    onAddVisit={addVisit}
                                    onRename={rename}
                                    onKind={changeKind}
                                    onRemove={removePoint}
                                >
                                    {#if dataLayer && calloutKind === 'add'}
                                        <LayerInspect compact chart={pinChart} date={shownDate} icon={dataLayer.icon} error={dataLayer.error} />
                                    {/if}
                                </MapCallout>
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
                    <MapSettings {theme} {dataLayers} bind:shownLayer={shownLayerId} bind:mapOverlays bind:hillshade bind:contours />
                    <PlaceKinds />
                    {#if hasTrailView(config)}
                        <div class="control-group">
                            <TrailViewControl bind:this={trailViewControl} active={mapMode?.kind === 'look'} onActivate={binoculars}
                                onDrop={(x, y) => { const at = map?.coordinateAtScreen(x, y); if (at) openTrailView(at, [x, y]); return !!at; }} />
                        </div>
                    {/if}
                </div>
                {#if dataLayer}
                    <div class="layer-date" bind:clientHeight={dateBarHeight}>
                        <div class="layer-legend" role="status">
                            {#if dataLayer.error}{dataLayer.error}
                            {:else if dataLayer.source}<strong>{dataLayer.label}</strong><LayerLegend legend={dataLayer.legend(theme)} label={`${dataLayer.label} colours`} />
                            {:else}Loading…{/if}
                        </div>
                        {#if layerYear}<LayerDateBar date={shownDate} year={layerYear} variable={dataLayer.variable} onDate={(date) => layerDate = date} />{/if}
                    </div>
                {/if}
                {#if dropping}
                    <div class="drop-frame"><strong>Drop to open as a plan</strong><span>One file is a route. Several files are a trip.</span></div>
                {/if}
                {#if mapMode}
                    <div class="mode-chip" role="status">
                        {mapMode.kind === 'night' ? `Click the route or the map to end day ${dayLabels[night] ?? night} here · Esc`
                            : mapMode.kind === 'transfer' ? `Click the map where the train or bus arrives. Day ${dayLabels[mapMode.night + 1]} starts there · Esc`
                            : mapMode.kind === 'draw' ? 'Drawing the leg · Esc cancels' : 'Click the map to open TrailView · Enter opens the map centre · Esc'}
                        <button type="button" onclick={() => mapMode = null}>Cancel</button>
                    </div>
                {/if}
            </div>
            <Resize value={drawerHeight} min={DRAWER_CLOSED} max={maxProfile} axis="y" label="Elevation height" onResize={resizeProfile} />
            {#if comparison}
                <CompareProfile option={comparison.option} current={comparison.current} letter={comparison.letter} height={drawerHeight} open={profileOpen} onToggle={() => profileOpen = !profileOpen} />
            {:else if !visualRoute}
                <section class="empty-profile" aria-label="Elevation profile" style:height={`${drawerHeight}px`}>
                    <DrawerTitle title="Elevation & surface" open={emptyOpen} onToggle={() => emptyOpen = !emptyOpen} />
                    {#if emptyOpen}<div><Icon name="route" size={24} /><p>{hasEndpoints ? 'The profile appears when your route is ready.' : 'See the climbs and surfaces along your route.'}</p><small>{hasEndpoints ? routingStatus : stops.length ? `Choose a ${nextEndpoint} to see the profile.` : 'Choose a start and finish to get started.'}</small></div>{/if}
                </section>
            {:else}
            <Profile
                lineData={visualRoute} walking={shown.trip.bike === 'hiking'} singleRoute={!multi} height={drawerHeight} open={profileOpen} onToggle={() => profileOpen = !profileOpen} {total} {days} {dayLabels} {theme}
                activeNight={focusedDay?.ridingNumber ?? 0} band={nightBand} window={profileWindow} {trip} {rests} onTrip={changePlan}
                focus={focusedDay ? { from: focusedDay.from, to: focusedDay.to, label: `Day ${focusedDay.number}` } : null}
                onNight={openNight} onDayEndDrag={moveDayEnd} onHover={(progress) => hoverProgress = progress}
                {part} onPart={next => { if (active) part = next; }}
            >
                {#snippet strip(from, to, rests, onHover)}
                    {#if dataLayer?.strip}<LayerStrip layer={dataLayer} line={visualRoute} samples={routeSamples} {total} date={shownDate} {theme} {from} {to} {rests} {onHover} />{/if}
                {/snippet}
            </Profile>
            {/if}
            <div class="status-line" role="status">
                <span class:save-error={!!session.saveError}>{session.saveError || `${message} · ${routingMessage}`}</span>
                {#if session.saveError}<button type="button" class="planner-action" onclick={() => session.download()}>Download plan</button>
                {:else if statusUndo}<span>·</span><button type="button" class="planner-action" onclick={statusUndo}>Undo</button>{/if}
                <span class="lab-note">Regional map, search and routing</span>
            </div>
        </section>
    </main>
    {/if}
    {#if asking}
        <SaveWindow mode={asking.mode} name={session.plan.name} stored={session.stored} {trip} onAnswer={answer => { asking?.answer(answer); asking = null; }} />
    {/if}
    {#if trailView}
        <TrailViewWindow at={trailView.at} near={trailView.near} origin={trailView.origin} onClose={() => { trailView = null; trailViewControl?.focus(); }} />
    {/if}
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
    .empty-profile { flex: none; display: flex; flex-direction: column; align-items: flex-start; padding: 8px 24px; overflow: hidden; background: var(--panel); }
    .empty-profile div { flex: 1; align-self: stretch; display: flex; flex-direction: column; align-items: center; justify-content: center; text-align: center; }
    .empty-profile p { margin: 12px 0 4px; }
    .empty-profile small { color: var(--ink-soft); }

    button {
        padding: 0;
        border: 0;
        background: none;
        color: inherit;
    }
    .planner-shell {
        --query-place: #3b654c;
        --query-area: #365f84;
        --query-time: #965125;
        display: flex;
        flex-direction: column;
        height: 100%;
        min-width: 760px;
        min-height: 580px;
    }
    :global([data-theme="dark"]) .planner-shell {
        --query-place: #aed0b5;
        --query-area: #a9cce9;
        --query-time: #edb58b;
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
    .planner-shell :global(.planner-action.primary) { border-color: var(--amber); background: var(--amber); color: var(--on-amber); }
    .planner-shell :global(.planner-action.primary:hover:not(:disabled)) { border-color: transparent; background: color-mix(in srgb, var(--amber) 86%, var(--ink)); }
    main {
        position: relative;
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
    .proto-prefs { border-bottom: 1px solid var(--line); }
    .back-to-routes { display: flex; align-items: center; gap: 6px; min-height: 32px; margin: -4px 12px 8px; padding: 0 6px; border-radius: 6px; color: var(--ink-soft); font-size: 13px; text-align: left; }
    .back-to-routes:hover { background: var(--parchment-2); color: var(--ink); }
    .closure-note { display: flex; align-items: center; gap: 8px; margin: 0 16px 12px; font-size: 13px; color: var(--ink-soft); }
    .closure-note :global(svg) { flex: none; }
    .closure-note span { flex: 1; }
    /* Clear of the map controls column, which reaches the bottom on a short map. The map reaches the clicks beside the legend. */
    .layer-date { position: absolute; left: 16px; right: 68px; bottom: 34px; z-index: 2; display: grid; gap: 8px; pointer-events: none; }
    .layer-date > :global(*) { pointer-events: auto; }
    .layer-legend { justify-self: start; display: flex; flex-wrap: wrap; align-items: center; gap: 4px 12px; padding: 7px 12px; border-radius: 8px; background: var(--panel); box-shadow: var(--planner-shadow); font-size: 12px; color: var(--ink-soft); }
    .layer-legend strong { font-weight: 600; color: var(--ink); }
    .list-head { display: flex; align-items: center; gap: 12px; padding: 12px 16px 4px; border-top: 1px solid var(--line); }
    .list-head h2 { margin: 0; font: 600 15px var(--sans); }
    .list-head :global(.planner-action) { gap: 4px; padding-left: 6px; }
    .list-lead { margin: 4px 16px 0; font-size: 13px; color: var(--ink-soft); }
    .list-links { display: flex; flex-wrap: wrap; gap: 4px 16px; padding: 6px 16px 8px; border-top: 1px solid var(--line); }
    .list-links button { display: inline-flex; align-items: center; gap: 6px; min-height: 28px; font-size: 13px; color: var(--ink-soft); }
    .list-links button:hover { color: var(--ink); }
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
    .drop-frame {
        position: absolute;
        inset: 16px;
        z-index: 5;
        display: grid;
        place-content: center;
        gap: 4px;
        border: 3px dashed var(--rust);
        border-radius: 14px;
        background: color-mix(in srgb, var(--panel) 85%, transparent);
        color: var(--ink);
        text-align: center;
        pointer-events: none;
    }
    .drop-frame strong { font-size: 18px; }
    .drop-frame span { color: var(--ink-soft); }
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
    /* Global, so that the button of a control component takes the same look; the global button rule gives the cursor. */
    .control-group :global(button) {
        display: grid;
        place-items: center;
        width: 36px;
        height: 36px;
        padding: 0;
        border: 0;
        background: none;
        color: var(--ink);
    }
    .control-group :global(button:hover) {
        color: var(--link);
    }
    .control-group button + button {
        border-top: 1px solid var(--line);
    }
    .control-group :global(.chosen) {
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
    @media (max-width: 760px) {
        .planner-shell { min-width: 0; height: auto; min-height: 100%; }
        main { grid-template-columns: minmax(0, 1fr); }
        main > :global([aria-label="Sidebar width"]) { display: none; }
        .planner-pane { max-height: 50dvh; }
        .geography { height: 75dvh; }
        .status-line { height: auto; min-height: 38px; flex-wrap: wrap; padding-block: 8px; }
        .lab-note { display: none; }
    }
</style>
