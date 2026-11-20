<script lang="ts">
    import { onMount, tick, untrack } from 'svelte';
    import PlannerMap from './PlannerMap.svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import Profile from './PlannerProfile.svelte';
    import DrawerTitle from './DrawerTitle.svelte';
    import Query from './PlannerQuery.svelte';
    import Resize from './PanelResize.svelte';
    import TripBar from './TripBar.svelte';
    import RoutePreferences from './RoutePreferences.svelte';
    import SaveWindow, { type SaveAnswer, type SaveMode } from './SaveWindow.svelte';
    import Itinerary from './Itinerary.svelte';
    import TripStrip from './TripStrip.svelte';
    import TripRow from './TripRow.svelte';
    import RouteList from './RouteList.svelte';
    import CompareScreen from './CompareScreen.svelte';
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
    import type { BikeType } from '../../lib/planner/riding-profiles';
    import type { RoutePlan } from '../../lib/planner/signed-route-plan';
    import type { CatalogRecord } from '../../lib/planner/signed-routes';
    import LibraryPanel from './PlanLibrary.svelte';
    import { savedWhen, type Plan } from '../../lib/planner/library';
    import type { Library } from '../../lib/library/store';
    import { PlanSession } from '../../lib/planner/plan-session.svelte';
    import { router } from '../../lib/router.svelte';
    import { ROUTES } from '../../lib/routes';
    import {
        addClickedPoint, addPointNear, addRestDay, addTransfer, allStay, closeLoop, releaseNight, trainAfter, multiDay, hasEndpoints as endpointsChosen,
        insertPoint, maxRidingDays, partDays, planTitle, nightOrderConflicts, dragPointOut,
        orderedRoutePoints, pinNight, pinNights, planView, daySegments, removeRestDay, reorderPoint, replacePoint, startLoopHere,
        setDrawnLeg, setLegMode, setSplit, setEndpoint, removeRoutePoint, endpointLabel,
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
    import { nightKinds, nightWindows, proposal, proposeNights, suitable, suits, withProposals, type Proposal, type Stop } from '../../lib/planner/stops';
    import { searchPlan } from '../../lib/planner/search/plan';
    import { containing, weekOf } from '../../lib/planner/week';
    import { placeDetails, withDetails, type SearchPlace, type SearchState, type SearchContext, type Where } from '../../lib/planner/search/types';
    import { placeSource } from '../../lib/planner/place-index';
    import { asPlace, category, sleepCategory, stayKind, stayOptions, trainNightText } from '../../lib/planner/search/presentation';
    import { applyQueryChanges } from '../../lib/planner/search/actions';
    import type { MapPoint, MapSegment } from '../../lib/planner/map-types';
    import { appearance } from '../../lib/theme.svelte';
    import { routingKey, type RoutingLine, type Surface } from '../../lib/planner/routing';
    import { surfaceLines } from '../../lib/planner/surface-data';
    import { DRAWER_CLOSED, DRAWER_MIN, resizeDrawer } from '../../lib/planner/drawer';
    import { closureStretches } from '../../lib/planner/route-closures';
    import { alpineStretches } from '../../lib/planner/route-difficulty';
    import { legGaps, routeGaps } from '../../lib/planner/route-gaps';
    import { noteKey, routeNotes as notesOf, type RouteNote } from '../../lib/planner/route-notes';
    import RouteNotes from './RouteNotes.svelte';
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
    // The compare screen of saved routes replaces the trip bar and the planner below it.
    let comparing = $state(false);

    let searching = $state(false);
    // Riding numbers stay stable when the itinerary includes rest days.
    let night = $state(1);
    let expandedDay = $state<number | null>(null);
    // The night whose towns are listed: one opened in the ladder, or a pinned night the rider changes.
    let choosingNight = $state<number | null>(null);
    // A night that opened, until the map shows it.
    let nightToShow = $state<number | null>(null);
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
    // A place selected in a town card opens its callout without the focus, so the arrow keys stay in the card.
    let quietCallout = $state(false);
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
    let here = $state<{ coordinate: Coordinate; at: number } | undefined>();
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
    // The place on the route under the map pointer, for the profile.
    let mapHover = $state<number | null>(null);
    // The surface that the rider points at in the surface legend, for the map. A legend that leaves the page lights nothing.
    let litSurface = $state<Surface>();
    $effect(() => { if (!visualRoute || !profileOpen) litSurface = undefined; });
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
    const multi = $derived(multiDay(trip));
    // The view follows the type on each path that changes it: the day count, Undo, Redo, a search, an import.
    let viewMulti: boolean | undefined;
    $effect.pre(() => {
        if (viewMulti !== undefined && multi !== viewMulti) untrack(() => {
            night = 1;
            expandedDay = null;
            stopChoosing();
            exitSearch();
            clearSelection();
        });
        viewMulti = multi;
    });
    // The days of the trip without its suggested nights. The towns are searched around their ends, and the suggestions
    // then move them. A drag preview changes the line but not the trip, so they follow the line of the trip.
    const evenDays = $derived(planView(trip, session.line).days);
    // The calendar days of the trip without its suggested nights: the week never reads the suggestions.
    const plain = $derived(planView(trip, session.line).itinerary);
    // The week in focus of a long trip, by its first calendar day. The week rests 300 ms after its last move, or after the
    // end of a drag on the strip; the night search reads only the rested week.
    let weekStart = $state(1);
    let restedStart = $state(1);
    let stripDragging = $state(false);
    const week = $derived(weekOf(plain, weekStart));
    // The map fits the week when it rests after a move of the strip or of ‹ ›. Any map move that starts before the rest,
    // by the rider or by the page, cancels the fit, and so does an opened day or night: the fit never undoes what the
    // rider chose. The fit clears the flag before it moves the map.
    let fitWeek = false;
    // The riding day that the pointer lights on the strip.
    let litDay = $state<number | null>(null);
    $effect(() => {
        const start = weekStart;
        if (stripDragging) return;
        const timer = setTimeout(() => {
            restedStart = start;
            if (!fitWeek) return;
            fitWeek = false;
            if (weekShown?.ridingDays.length) map?.showWeek(routeSlice(coordinates, weekShown.from, weekShown.to));
        }, 300);
        return () => clearTimeout(timer);
    });
    function moveWeek(start: number) {
        weekStart = start;
        fitWeek = true;
    }
    /** Moves the week the least that puts calendar day `day` inside it. */
    function showInWeek(day: number | undefined) {
        fitWeek = false;
        if (week && day !== undefined) weekStart = containing(plain.length, week.start, day);
    }
    // The page of the night the rider opened replaces the day list. It stays while the route is recalculated.
    const nightPage = $derived(multi && !searching && expandedDay === null && choosingNight !== null && choosingNight < evenDays.length ? choosingNight : null);
    // An open day lists the towns of its night until that night is pinned; a night page lists its own night.
    const pickerNight = $derived(!multi || !currentRoute || searching ? null
        : expandedDay !== null ? (night < evenDays.length && (!evenDays[night - 1]?.pinned || choosingNight === night) ? night : null)
        : nightPage);
    // A night that the plan no longer has must not open again when a later change brings its day back. A route that is
    // still calculated keeps the night, so the page stays open while it is recalculated.
    $effect(() => {
        if (currentRoute && choosingNight !== null && choosingNight >= evenDays.length) untrack(stopChoosing);
    });
    // Towns are searched for every night still to choose, of the rested week on a long trip, so the ladder can suggest a
    // place for each.
    const stopWindows = $derived(multi ? nightWindows(trip, session.line, pickerNight, weekOf(plain, restedStart)?.nights) : {});
    const routeKey = $derived(routingKey(trip));
    $effect(() => {
        const stretches = Object.values(stopWindows), route = routeKey, context = searchContext;
        if (!session.dragging) untrack(() => stopFinder.want(route, context, stretches));
    });
    const nightStops = (night: number) => stopWindows[night] ? stopFinder.found[stretchKey(routeKey, stopWindows[night])] : undefined;
    const readyStops = (night: number): Stop[] => { const found = nightStops(night); return found?.state === 'ready' ? found.stops : []; };
    // The map shows a night once for each opening, so a change of its kinds does not move the map. A night with a window
    // waits while its towns load; a move of the rider or the close of the night drops the wait, so the map never jumps late.
    $effect(() => {
        const number = nightToShow;
        if (number === null || (number === pickerNight && stopWindows[number] && (nightStops(number)?.state ?? 'loading') === 'loading')) return;
        nightToShow = null;
        if (number === pickerNight) untrack(() => { const area = nightArea(number); if (area) map?.showNight(...area); });
    });
    const proposals = $derived(multi ? proposeNights(trip, session.line, readyStops) : new Map<number, Proposal>());
    const campsites = $derived([...proposals].flatMap(([night, { place }]) => place && stayKind(place.place.placeKind) === 'camp' ? [{ night, place: place.place }] : []));
    const planned = $derived(withProposals(shown.trip, shown.line, proposals));
    const view = $derived(planView(planned, shown.line));
    // Ascent and descent stay unknown while a part of the line has no elevation.
    const totals = $derived.by(() => {
        if (!visualRoute) return null;
        const known = visualRoute.unknownElevationKm === 0;
        return { ...view.summary, ascent: known ? view.summary.ascent : null, descent: known ? view.summary.descent : null, walking: shown.trip.bike === 'hiking' };
    });
    const coordinates = $derived(view.coordinates);
    const lengths = $derived(cumulative(coordinates));
    const total = $derived(view.total);
    // The route list follows the points of the trip, also while their new line is calculated.
    const stops = $derived(session.stale ? planView(trip, undefined).stops : view.stops);
    const visualStops = $derived(view.stops);
    // A suggested night is not a day end the rider moved.
    const days = $derived(view.days.map(day => day.split && trip.splits?.[day.number] === undefined ? { ...day, split: false } : day));
    const itinerary = $derived(view.itinerary);
    // The stretch of the week on the shown line, with the suggested nights.
    const weekShown = $derived(week && { ...week, from: itinerary[week.start - 1]?.from ?? 0, to: itinerary[week.end - 1]?.to ?? 1 });
    const focusedDay = $derived(multi && !searching ? itinerary.find(d => !d.rest && d.ridingNumber === expandedDay) ?? null : null);
    const itineraryShown = $derived(multi && !!visualRoute && (!!currentRoute || routingStatus === 'Calculating route…'));
    const dayLabels = $derived(Object.fromEntries(itinerary.filter(d => !d.rest).map(d => [d.ridingNumber, d.number])));
    const rests = $derived(itinerary.filter(d => d.rest).map(d => ({ day: d.number, after: d.ridingNumber, name: planned.restNames?.[d.restIndex!], town: d.pinned?.label ?? proposals.get(d.ridingNumber)?.stop.name ?? '' })));
    const queryPlan = $derived(searchPlan(trip, view));
    // The rider moves, so a position older than five minutes is left out and the search asks for a new one.
    // A getter, because time passing changes no dependency of the context.
    const searchContext = $derived<SearchContext>({ view: viewBounds, get here() { return here && Date.now() - here.at < 300_000 ? here.coordinate : undefined; },
        pointing, startDate: trip.startDate, plan: queryPlan });
    const conflicts = $derived(multi ? nightOrderConflicts(shown.trip, shown.line) : []);
    const gaps = $derived(visualRoute ? routeGaps(visualStops, coordinates) : []);
    const gapLegs = $derived(visualRoute ? legGaps(visualStops, coordinates, visualRoute.gapLegs) : []);
    const routeNotes = $derived(notesOf(gaps, gapLegs, closureStretches(visualRoute), alpineStretches(visualRoute)));
    // Plan edits rebuild the notes without a new route, so the shown note is kept by key; a route without it marks nothing.
    let shownKey = $state<string | null>(null);
    const shownNote = $derived(routeNotes.find(note => noteKey(note) === shownKey));
    function showNote(note: RouteNote) {
        shownKey = note === shownNote ? null : noteKey(note);
        if (shownKey) map?.fitCoordinates(note.coordinates.flat());
    }
    // The ladder and an open day light up the stretch whose towns they list.
    const nightBand = $derived.by(() => {
        const window = pickerNight !== null ? stopWindows[pickerNight] : undefined;
        return window && total ? { from: window[0] / total, to: window[1] / total, blocked: false } : null;
    });
    const highlighted = $derived(litSurface ? surfaceLines(visualRoute, litSurface)
        : nightBand ? [routeSlice(coordinates, nightBand.from, nightBand.to)] : shownNote?.coordinates ?? []);
    const openStop = $derived(pickerNight !== null ? readyStops(pickerNight).find(stop => stop.id === stopId && suits(stop, nightKinds(trip, pickerNight))) : undefined);
    const routeHeights = $derived(currentRoute ? profileSamples(currentRoute) : []);
    const nights: NightChoices = {
        get choosing() { return pickerNight; },
        get stopId() { return openStop?.id ?? null; },
        get shownId() { return selectedId; },
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
        choose: number => { choosingNight = nightToShow = number; stopId = null; clearSelection(); if (number !== null) night = number; },
        open: id => { stopId = id; clearSelection(); const stop = pickerNight !== null ? readyStops(pickerNight).find(s => s.id === id) : undefined; if (stop) map?.fitCoordinates(suitable(stop, nights.kinds(pickerNight!)).map(p => p.place.coordinate)); },
        step: (night, direction) => {
            const stop = nights.neighbours(night)[direction < 0 ? 0 : 1];
            if (stop) endDayIn(night, stop);
        },
        endHere: (night, stop) => endDayIn(night, stop),
        stay: (night, place) => stayAt(night, place),
        selectPlace: place => {
            if (!place) return clearSelection();
            selectPlace(place);
            quietCallout = true;
        },
        pick: () => pickOnMap(),
        retry: night => { const window = stopWindows[night]; if (window) stopFinder.retry(stretchKey(routeKey, window)); },
    };
    // One stretch per leg and day, so the map can tell legs apart and colour days.
    const segments = $derived<MapSegment[]>(daySegments(view).map(segment => ({ ...segment, color: dayColor(segment.day, theme) })));
    const results = $derived((searchState.answer?.results ?? []).map(result => ({ place: asPlace(result) })));
    const visiblePlaces = $derived(searching ? results.map(result => result.place) : pickerNight === null ? []
        : (openStop ? [openStop] : readyStops(pickerNight)).flatMap(stop => suitable(stop, nights.kinds(pickerNight)).map(p => p.place)));
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
    // The riding day of each route point, by its place on the line.
    const pointDays = $derived.by(() => {
        const length = visualStops.at(-1)?.distance || 1;
        return new Map(visualStops.map(stop => [stop.point.id, days.find(day => stop.distance / length <= day.to)?.number]));
    });
    const mapPoints = $derived.by(() => {
        const pins: MapPoint[] = trip.points.map(p => ({
            ...p,
            day: pointDays.get(p.id),
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
        stopChoosing();
        visibleRange = null;
        hoverProgress = null;
        part = null;
        restedStart = weekStart = 1;
        fitWeek = false;
        fitPlan = true;
        nameVisits(session.trip);
    }

    function nameVisits(trip: Trip) {
        trip.points.filter(point => point.autoLabel).forEach(point => { void session.nameVisit(point); void nameEndpoint(point); });
    }

    /** Names a start or finish that still has its placeholder after the nearest town in the map tiles, else from the search.
     * A found name stays: the two lookups can disagree, so a second lookup could rename the point by itself. */
    async function nameEndpoint(point: RoutePoint) {
        if (!point.autoLabel || (point.kind !== 'start' && point.kind !== 'finish') || point.label !== endpointLabel(point.kind)) return;
        const label = map?.placeName(point.coordinate) ?? await townName(point.coordinate);
        if (label) session.autoName(point.id, point.coordinate, label);
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

    async function restoreDraft() {
        if (await session.restore()) afterHistory('Unsaved changes restored');
    }

    function dismissDraft() {
        void session.dismiss();
        statusUndo = null;
    }

    async function openPlan(id: string) {
        if (!await leave()) return;
        if (!await session.open(id)) {
            if (session.libraryError) message = session.libraryError;
            return;
        }
        libraryOpen = false;
        message = 'Route opened';
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

    function afterEdit(description: string) {
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
        quietCallout = false;
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
            showInWeek(dayLabels[ridingDay]);
            night = ridingDay;
            expandedDay = ridingDay;
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
        if (multi && expandedDay !== null) showDay(expandedDay);
        else { clearSelection(); map?.fitRoute(); }
    }

    /** Opens a riding day and fits its route on the map. */
    function showDay(riding: number) {
        showInWeek(dayLabels[riding]);
        night = riding;
        expandedDay = riding;
        stopChoosing();
        exitSearch();
        clearSelection();
        const day = days[riding - 1];
        map?.fitCoordinates(routeSlice(coordinates, Math.max(0, day.from - .04), Math.min(1, day.to + .04)));
    }

    /** Opens a night in the day list, with its towns. */
    function openNight(number: number) {
        fitWeek = false;
        // The night before the week stands at the left edge of the profile: it is one of the week's nights already.
        if (!week?.nights.includes(number)) showInWeek(dayLabels[number]);
        expandedDay = null;
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
            showInWeek(dayLabels[number]);
            night = number;
            expandedDay = number;
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

    // Outside the region (null), a click only clears the selection.
    function emptyClick(coordinate: Coordinate | null) {
        if (!coordinate) {
            if (!picking && mapMode?.kind !== 'transfer' && selectedId) clearSelection();
        } else if (mapMode?.kind === 'transfer') {
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
        const shapes = (plan: Trip) => plan.points.filter(p => p.kind === 'via').length;
        if (next !== trip) commit(next, shapes(next) < shapes(trip) ? 'Shapes cleared on the changed legs' : 'Stops reordered · changed legs follow roads');
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
            message = 'The route changed, so the train or bus was not added. Try again.';
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
        choosingNight = nightToShow = null;
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

    /** Where the rider sleeps on night `number`: the planned end of its day, at its pinned or suggested place if it has one.
     * Then the places of the towns listed for the night, nearest that end first. */
    function nightArea(number: number): [Coordinate, Coordinate[][]] | undefined {
        if (!total) return;
        const { pinned, to } = days[number - 1], place = pinned ? undefined : nights.around(number)?.place, km = to * total;
        const kinds = nights.kinds(number), towns = readyStops(number).filter(stop => suits(stop, kinds)).sort((a, b) => Math.abs(a.km - km) - Math.abs(b.km - km));
        return [pinned?.coordinate ?? place?.place.coordinate ?? coordinateAt(coordinates, to), towns.map(stop => suitable(stop, kinds).map(p => p.place.coordinate))];
    }

    /** Fits the map to the end of the day of night `number` and its three nearest towns, or, with no towns to show, to
     * the stretch of route searched for them. */
    function showNightArea(number: number) {
        const area = nightArea(number), window = stopWindows[number];
        if (!area) return;
        const towns = area[1].slice(0, 3).flat();
        map?.fitCoordinates(towns.length || !window ? [area[0], ...towns]
            : routeSlice(coordinates, Math.max(0, window[0] / total - .02), Math.min(1, window[1] / total + .02)));
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
        if (point) { void session.nameVisit(point); void nameEndpoint(point); }
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
        commit(setEndpoint(trip, kind, coordinate, selectedPlace?.label), kind === 'start' ? 'Start set' : 'Finish set');
        void nameEndpoint(trip.points.find(p => p.kind === kind)!);
        showRoute = true;
        clearSelection();
        exitSearch();
    }

    async function newPlan() {
        if (!await leave() || !await session.newPlan()) return;
        message = 'New route';
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

    function changePlan(next: Trip, description: string) {
        commit(next, description);
        clampNight();
    }

    function locate() {
        return new Promise<'denied' | 'unavailable' | undefined>(resolve => {
            if (!navigator.geolocation) { resolve('unavailable'); return; }
            navigator.geolocation.getCurrentPosition(({ coords }) => {
                here = { coordinate: [coords.longitude, coords.latitude], at: Date.now() };
                resolve(undefined);
            }, error => resolve(error.code === error.PERMISSION_DENIED ? 'denied' : 'unavailable'), { timeout: 10000 });
        });
    }

    async function applySearch() {
        const answer = $state.snapshot(searchState.answer);
        if (!answer?.changes || applyingQuery) return;
        const before = trip;
        applyingQuery = true; queryApplyError = '';
        try {
            const next = await applyQueryChanges(before, session.line, answer.changes, plan => session.calculate(plan, new AbortController().signal));
            if (JSON.stringify(answer.changes) !== JSON.stringify(searchState.answer?.changes) || before !== trip) throw new Error('The route changed. Review the search again.');
            if (answer.changes.some(change => change.op === 'route')) { exploreBack = undefined; restedStart = weekStart = 1; fitWeek = false; }
            session.commit(next.trip, next.line);
            showInWeek(answer.changes.find(change => change.day !== undefined)?.day);
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
        // Escape goes back one level wherever the focus is: a town, then a night page, then an open day. The map
        // prevents the default when Escape closes its route details or its place list: that is the level. A map selection
        // or mode closes before the selected part, which `Lanes` clears unless the default is prevented.
        if (event.key === 'Escape' && !event.defaultPrevented) {
            if (selectedId || mapMode) event.preventDefault();
            else if (!part) {
                if (openStop) stopId = null;
                else if (nightPage !== null) stopChoosing();
                else if (focusedDay) showAllDays();
            }
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
        canUndo={session.canUndo} canRedo={session.canRedo} onUndo={undo} onRedo={redo}
        onSave={save} onRename={renameRoute} more={[
            { label: 'New route', disabled: !trip.points.length, run: newPlan },
            { label: 'Open route…', run: openLibrary },
            { label: 'Save a copy…', rule: true, disabled: !trip.points.length, run: saveCopy },
            { label: 'Discard changes', disabled: !session.changed, run: discard },
            { label: 'Compare routes…', rule: true, run: compare },
        ]}
    />
    {/key}
    <main aria-busy={!session.ready}>
        {#if libraryOpen}
            <LibraryPanel plans={session.plans} unreadable={session.unreadable} onDeleteUnreadable={() => { if (window.confirm('Delete the saved routes that cannot be read? This cannot be undone.')) void session.removeUnreadable(); }} activeId={planId} busy={session.busy} error={session.libraryError}
                onClose={() => libraryOpen = false} onOpen={plan => openPlan(plan.id)} onRename={(plan, name) => session.rename(plan, name)} onDuplicate={duplicate}
                onDelete={deletePlan} onImport={importFiles} gpxNames={session.gpxLines?.map(line => line.name) ?? null}
                onGpx={roads => roads === null ? session.gpxLines = null : importGpx(roads)} onDownload={plan => session.download(plan)} />
        {/if}
        <aside class="planner-pane" aria-label="Trip planning" inert={!session.ready || session.busy}>
            <div class="query-slot" style:display={nightPage !== null ? 'none' : 'contents'}><Query bind:this={searchBox} bind:text={query} bind:searchState={searchState} context={searchContext} selection={calloutCoordinate ? { anchor: calloutCoordinate } : undefined} revision={searchRevision} viewRevision={searchViewRevision} onSearch={() => { searching = true; queryApplyError = ''; }} onClear={clearSearch} onLocation={locate} onPointing={where => pointing = where} /></div>
            {#if !searching && nightPage === null}{#key planId}<RoutePreferences {trip} onChange={edit} />{/key}{/if}
            {#if exploreBack && hasEndpoints && !searching && nightPage === null}
                <button type="button" class="back-to-routes" onclick={() => router.go(ROUTES.explore, { explore: exploreBack })}><Icon name="back" size={15} />Find another route</button>
            {/if}
            {#if searching}
                <div class="pane-scroll">
                    <QueryResults attribution={config.attribution} state={searchState} days={queryPlan.days.filter(day => !day.rest)} {theme} {selectedId} {hoveredId} onHover={(id) => hoveredId = id} onSelect={selectPlace} applying={applyingQuery} applyError={queryApplyError} onApply={applySearch} onMore={() => searchBox?.more()} onRetry={() => searchBox?.retry()} onStretch={line => { pointing = {along:{ref:'km',from:{value:nearestProgress(coordinates,line[0])*total,unit:'km'},to:{value:nearestProgress(coordinates,line.at(-1)!)*total,unit:'km'}}}; map?.fitCoordinates(line); }} />
                </div>
            {:else if !hasEndpoints}
                <div class="start-plan pane-scroll">
                    <Icon name="route" size={24} />
                    <h2>{stops.length ? `Choose your ${nextEndpoint}` : 'Where would you like to ride?'}</h2>
                    <p>{stops.length ? `Now set your ${nextEndpoint}.` : 'Click the map or search to set your start.'}</p>
                    {#each stops as { point } (point.id)}
                        <button type="button" class="chosen-endpoint" onclick={() => inspectPoint(point)}>
                            <Icon name="pin" size={18} /><span><small>{point.kind === 'start' ? 'Start' : 'Finish'}</small><strong>{point.label}</strong></span><Icon name="chevron" size={16} />
                        </button>
                    {/each}
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
                {#if !focusedDay && nightPage === null}
                    <RouteNotes notes={routeNotes} shown={shownNote} onShow={showNote} />
                    <!-- The rider's trip, not `planned`: a change must not keep the suggested nights as day ends. -->
                    <div class="trip-settings"><TripRow {trip} line={session.line} onChange={changePlan}
                        onDays={(next, description) => { changePlan(next, description); if (!multiDay(next)) statusUndo = undo; }}
                        onLimit={() => message = `${maxRidingDays} riding days is the most a trip can have`} />
                        {#if multi && trip.days > partDays}<p class="device-parts">Goes to the bike computer in 2 parts</p>{/if}</div>
                {/if}
                <div class="pane-scroll" inert={multi && !currentRoute && routingStatus === 'Calculating route…'}>
                    {#if itineraryShown}
                        <Itinerary
                            trip={planned} line={visualRoute} {itinerary} {days} {theme} {expandedDay} {nightPage} {nights} {conflicts} {selectedId} {revealId} {hoveredId} onHover={(id) => hoveredId = id}
                            week={weekShown} onWeek={moveWeek}
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
                    {theme} {hillshade} {contours} {mapOverlays} dataLayer={{ layers: dataLayers, shown: dataLayer, date: shownDate, notes: layerNotes }} bottomInset={dataLayer ? dateBarHeight + 34 : 0} accessMode={trip.bike === 'hiking' ? 'walking' : 'cycling'} {showRoute} {hoverProgress} highlightedLines={highlighted} pickMode={picking || mapMode?.kind === 'transfer'} routingPackage={currentRoute?.package}
                    onPick={mapMode?.kind === 'look' ? openTrailView : null}
                    highlightedPlaceIds={searching ? results.map(result => result.place.id) : []}
                    placeKinds={placeChoice.kinds} highlightedKind={placeChoice.highlight}
                    onBounds={(bounds, preserveSearch) => { viewBounds = bounds; if (!preserveSearch) searchViewRevision++; }} onEmptyClick={emptyClick} onPointSelect={selectPoint} onPointMove={movePoint} onPointPreview={(id, coordinate) => coordinate ? session.previewMove(id, coordinate) : session.cancelPreview()} onDayEndDrag={moveDayEnd}
                    onLegClick={legClick} onInsert={(legEndId, coordinate) => insert(legEndId, coordinate, true)} onDrawn={drawn} onPlaceClick={place => mapMode?.kind === 'transfer' ? void arrive(place.coordinate, place.label) : choosePlace(place)}
                    onVisibleRange={(range) => visibleRange = range}
                    onRiderMove={() => nightToShow = null} onMoveStart={() => fitWeek = false} onRouteHover={progress => mapHover = progress}
                    {week} lit={week ? litDay : null} onDayClick={showDay}
                >
                    {#snippet popup()}
                        {#if calloutKind}
                            {#key selectedId}
                                <MapCallout
                                    kind={calloutKind} trip={withProposals(trip, session.line, proposals)} line={session.line} {days} {dayLabels} {night} {legMode} takeFocus={!quietCallout}
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
                    <div class="drop-frame"><strong>Drop to open</strong><span>One file is a route. Several files are a trip.</span></div>
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
            {#if !visualRoute}
                <section class="empty-profile" aria-label="Elevation profile" style:height={`${drawerHeight}px`}>
                    <DrawerTitle title="Elevation & surface" open={emptyOpen} onToggle={() => emptyOpen = !emptyOpen} />
                    {#if emptyOpen}<div><Icon name="route" size={24} /><p>{hasEndpoints ? 'The profile appears when your route is ready.' : 'See the climbs and surfaces along your route.'}</p><small>{hasEndpoints ? routingStatus : stops.length ? `Choose a ${nextEndpoint} to see the profile.` : 'Choose a start and finish to get started.'}</small></div>{/if}
                </section>
            {:else}
            <Profile
                lineData={visualRoute} walking={shown.trip.bike === 'hiking'} singleRoute={!multi} height={drawerHeight} open={profileOpen} onToggle={() => profileOpen = !profileOpen} {total} {days} {dayLabels} {theme}
                activeNight={focusedDay?.ridingNumber ?? 0} band={nightBand} window={profileWindow} {rests} {totals}
                focus={focusedDay ? { from: focusedDay.from, to: focusedDay.to, label: `Day ${focusedDay.number}` } : null}
                onNight={openNight} onDayEndDrag={moveDayEnd} onHover={(progress) => hoverProgress = progress}
                {part} onPart={next => { if (active) part = next; }} marker={mapHover} bind:surface={litSurface} week={weekShown}
            >
                {#snippet tripStrip()}
                    {#if week}<TripStrip {itinerary} {week} {theme} startDate={trip.startDate} onMove={moveWeek} onHover={riding => litDay = riding} bind:dragging={stripDragging} />{/if}
                {/snippet}
                {#snippet strip(from, to, rests, onHover)}
                    {#if dataLayer?.strip}<LayerStrip layer={dataLayer} line={visualRoute} samples={routeSamples} {total} date={shownDate} {theme} {from} {to} {rests} {onHover} />{/if}
                {/snippet}
            </Profile>
            {/if}
            <div class="status-line" role="status">
                <span class:save-error={!!session.saveError}>{session.saveError || `${message} · ${routingMessage}`}</span>
                {#if session.saveError}<button type="button" class="planner-action" onclick={() => session.download()}>Download plan file</button>
                {:else if statusUndo}<span>·</span><button type="button" class="planner-action" onclick={statusUndo}>Undo</button>
                {:else if session.earlier}<span>· Earlier unsaved changes discarded ·</span><button type="button" class="planner-action" onclick={restoreDraft}>Undo</button>{/if}
                {#if session.offer}
                    <span class="reminder">
                        <span>Unsaved changes from {savedWhen(session.offer.updatedAt)}</span>
                        <button type="button" class="planner-action quiet" onclick={restoreDraft}>Restore</button>
                        <button type="button" class="planner-action quiet" aria-label="Dismiss" onclick={dismissDraft}><Icon name="close" size={14} /></button>
                    </span>
                {/if}
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
    .planner-shell :global(.planner-action),
    .planner-shell :global(.planner-chip) {
        display: inline-flex;
        align-items: center;
        min-height: 32px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        cursor: pointer;
    }
    .planner-shell :global(.planner-action:hover:not(:disabled)),
    .planner-shell :global(.planner-chip:hover) { background: var(--parchment-2); border-color: var(--ink-faint); }
    .planner-shell :global(.planner-action) {
        justify-content: center;
        padding: 5px 10px;
        font: inherit;
        font-weight: 600;
        text-decoration: none;
    }
    .planner-shell :global(.planner-action.quiet) {
        color: var(--ink-soft);
    }
    .planner-shell :global(.planner-chip) {
        gap: 7px;
        /* Room for a label that wraps; a one-line chip stays at the minimum height. */
        padding: 4px 9px 4px 10px;
        font: 600 13px var(--sans);
    }
    .planner-shell :global(.planner-chip > svg:first-child) { color: var(--chip-ink, var(--ink-soft)); }
    .planner-shell :global(.planner-chip[aria-expanded="true"]) { border-color: var(--ink); }
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
    .trip-settings { padding: 12px 16px; border-top: 1px solid var(--line); }
    .device-parts { margin: 8px 0 0; font-size: 13px; color: var(--ink-soft); }
    .back-to-routes { display: flex; align-items: center; gap: 6px; min-height: 32px; margin: -4px 12px 8px; padding: 0 6px; border-radius: 6px; color: var(--ink-soft); font-size: 13px; text-align: left; }
    .back-to-routes:hover { background: var(--parchment-2); color: var(--ink); }
    /* Clear of the map controls column, which reaches the bottom on a short map. The map reaches the clicks beside the legend. */
    .layer-date { position: absolute; left: 16px; right: 68px; bottom: 34px; z-index: 2; display: grid; gap: 8px; pointer-events: none; }
    .layer-date > :global(*) { pointer-events: auto; }
    .layer-legend { justify-self: start; display: flex; flex-wrap: wrap; align-items: center; gap: 4px 12px; padding: 7px 12px; border-radius: 8px; background: var(--panel); box-shadow: var(--planner-shadow); font-size: 12px; color: var(--ink-soft); }
    .layer-legend strong { font-weight: 600; color: var(--ink); }
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
    .reminder {
        display: flex;
        align-items: center;
        gap: 6px;
    }
    .lab-note {
        margin-left: auto;
    }
    @media (max-width: 760px) {
        .planner-shell { min-width: 0; height: auto; min-height: 100%; }
        main { grid-template-columns: minmax(0, 1fr); }
        main > :global([aria-label="Sidebar width"]) { display: none; }
        /* An open stay list or date popover of the trip bar can be taller than the pane: the pane scrolls. */
        .planner-pane { max-height: 50dvh; overflow-y: auto; }
        .geography { height: 75dvh; }
        .status-line { height: auto; min-height: 38px; flex-wrap: wrap; padding-block: 8px; }
        .lab-note { display: none; }
    }
</style>
