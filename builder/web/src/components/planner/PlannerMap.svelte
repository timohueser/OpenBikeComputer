<script lang="ts">
    import { onMount, tick, untrack, type Snippet } from "svelte";
    import * as maplibregl from "maplibre-gl";
    import type { GeoJSONSource } from "maplibre-gl";
    import type { Feature, FeatureCollection, LineString, Polygon } from "geojson";
    import { ClosedAttribution, styleConfig, terrain, useMapProtocols } from "../../lib/planner/map-setup";
    import { config } from "../../lib/planner/map-data";
    import { regionPlace } from "../../lib/planner/config";
    import { terrainRetry } from "../../lib/planner/map-terrain";
    import "maplibre-gl/dist/maplibre-gl.css";
    import { mapStyle, type Relief } from "../../lib/planner/map-style";
    import { mapIcon } from "../../lib/planner/map-icons";
    import { placeCategories, type PlaceCategory } from "../../lib/planner/poi-kinds";
    import { poiPlace } from "../../lib/planner/place-index";
    import { FIT_LEVEL, LAST_LEVEL, opens, type TilePlace } from "../../lib/planner/place-groups";
    import type { Place } from "../../lib/planner/editor";
    import { coordinateAt, kilometres, nearestOnScreen, nearestProgress, routeSlice } from "../../lib/planner/geo";
    import type { Coordinate, MapPoint, MapSegment, MapTown } from "../../lib/planner/map-types";
    import { grownHull, span } from "../../lib/planner/town-area";
    import { RouteOverlays, type AccessMode, type OverlayOptions, type OverlaySelection, type RouteHint } from "../../lib/planner/route-overlays";
    import type { DataLayer } from "../../lib/planner/layers/data-layer";
    import { nightView } from "../../lib/planner/view-change";
    import { mapCallout, type MapCallout } from "../../lib/planner/map-callout";
    import { dayLook, pinLook, type Week } from "../../lib/planner/week";
    import MapOverlayDetails from './MapOverlayDetails.svelte';
    import PlaceMarks from './PlaceMarks.svelte';
    import PlaceList from './PlaceList.svelte';

    let {
        segments = [], gaps = [], coordinates = [], highlightedLines = [], points = [], selectedId = null, hoveredId = null, callout = null,
        drawing = null, highlightedPlaceIds = [], theme = "light", hillshade = true, contours = true, pickMode = false,
        showRoute = true, hoverProgress = null, center = [8.8, 48.65], zoom = 7,
        placeKinds = [], highlightedKind = null, mapOverlays = { network: 'none', access: false }, accessMode = 'cycling', routingPackage, dataLayer, bottomInset = 0,
        lines = [], onPick = null, part = null, week = null, lit = null, towns = [], litStop = null,
        onStopHover, onStopOpen, onEmptyClick, onPointSelect, onPointHover, onPointMove, onPointPreview, onDayEndDrag, onLegClick, onInsert, onDrawn, onPlaceClick, onVisibleRange, onBounds, onRiderMove, onMoveStart, onRouteHover, onDayClick, popup,
    }: {
        /** Lines that take no edits, such as the days of two saved routes, each in its colour and width; a later line
         * draws on top. */
        lines?: { coordinates: Coordinate[]; color: string; width: number }[];
        /**
         * While set, a click on the map or a pin goes to it alone, with the coordinate and its viewport point: no place,
         * line, overlay or route takes the click. Enter on the focused map gives it the map's centre.
         */
        onPick?: ((coordinate: Coordinate, point: [number, number]) => void) | null;
        segments?: MapSegment[];
        /** Dashed connectors from the route to points that it does not reach. */
        gaps?: Coordinate[][];
        coordinates?: Coordinate[]; highlightedLines?: Coordinate[][]; points?: MapPoint[];
        selectedId?: string | null; callout?: Coordinate | null; drawing?: string | null; highlightedPlaceIds?: string[];
        hoveredId?: string | null;
        /** While picking, any map click places the overnight; the line takes no edits. */
        pickMode?: boolean; showRoute?: boolean; theme?: "light" | "dark"; hillshade?: boolean; contours?: boolean;
        /** The kinds of places that the map draws, and the one of them that it highlights. */
        placeKinds?: PlaceCategory[]; highlightedKind?: PlaceCategory | null;
        mapOverlays?: OverlayOptions;
        /** The region's data layers, the shown one if any, the layer date, and the shown layer's labels beside the overnight stops. */
        dataLayer?: { layers: DataLayer[]; shown?: DataLayer; date: string; notes?: { coordinate: Coordinate; text: string }[] };
        /** Height of a bar over the bottom of the map that fits and callouts keep clear of. */
        bottomInset?: number;
        /** Travel mode, independent of the network chosen for display. */
        accessMode?: AccessMode;
        /** Routing package of the shown route. Overlays from another package stay hidden. */
        routingPackage?: string;
        /** Route progress the elevation profile points at. */
        hoverProgress?: number | null;
        /** The part of the route the profile selects, as route progress; the rest of the line is pale. */
        part?: [number, number] | null;
        /** The week in focus of a long trip: the map draws its days in their colours and the other days quiet. */
        week?: Week | null;
        /** The riding day that the strip lights. */
        lit?: number | null;
        /** The towns of the open night, and the stop of that night that is lit: a town or the place of a lone pin. */
        towns?: MapTown[]; litStop?: string | null;
        /** The stop of the night under the pointer: a town by its area or its name, a lone place by its pin. */
        onStopHover?: (id: string | null) => void;
        /** A click on the area or the name of a town. */
        onStopOpen?: (id: string) => void;
        center?: Coordinate; zoom?: number;
        /** A click on no point or line; null outside the region. */
        onEmptyClick?: (coordinate: Coordinate | null) => void;
        onPointSelect?: (id: string) => void;
        onPointHover?: (id: string | null) => void;
        /** A pin drag; null ends the preview. */
        onPointPreview?: (id: string, coordinate: Coordinate | null) => void;
        onPointMove?: (id: string, coordinate: Coordinate) => void;
        onDayEndDrag?: (night: number, progress: number) => void;
        onLegClick?: (legEndId: string, coordinate: Coordinate) => void;
        onInsert?: (legEndId: string, coordinate: Coordinate) => void;
        onDrawn?: (legEndId: string, coordinates: Coordinate[]) => void;
        onPlaceClick?: (place: Place) => void;
        onBounds?: (bounds: [number, number, number, number], preserveSearch: boolean) => void;
        onVisibleRange?: (range: [number, number]) => void; popup?: Snippet;
        /** The rider starts to move the map, by pointer, wheel or keyboard. */
        onRiderMove?: () => void;
        /** The map starts to move, by the rider or by the page. */
        onMoveStart?: () => void;
        /** The place on the route under the pointer, as route progress, or null off the route. */
        onRouteHover?: (progress: number | null) => void;
        /** A click on the line of a day outside the week. */
        onDayClick?: (day: number) => void;
    } = $props();

    type LineHit = { legEndId: string; day: number; coordinate: Coordinate };
    const lineReach = 10;
    const sketchStep = 3;
    // The right inset keeps the route and callouts clear of the map controls.
    const controlsWidth = 68;
    const fitPadding = { top: 60, right: 90, bottom: 40, left: 40 };
    const calloutOffset = 20;
    // How far the area of a town reaches beyond its places, in px: the open town holds a pin for each place.
    const townGrowth = 15, openTownGrowth = 34;

    let container: HTMLDivElement;
    let frameWidth = $state(0);
    let frameHeight = $state(0);
    // The scale and attribution strip, or a bar over the bottom of the map.
    const bottomClear = $derived(Math.max(48, bottomInset + 12));
    let popupContent: HTMLDivElement;
    let map = $state.raw<maplibregl.Map>();
    let ready = $state(false);
    // True until the map has drawn its tiles and stopped moving.
    let loading = $state(true);
    let failure = $state("");
    let errorDetail = $state("");
    type Pin = { marker: maplibregl.Marker; button: HTMLButtonElement; key: string; point: MapPoint; draggable: boolean; title: string };
    const pins = new Map<string, Pin>();
    let madePins = $state.raw<Pin[]>([]);
    let draggingPin = $state(false);
    let calloutPopup: MapCallout | undefined;
    let relief: Relief;
    let appliedTheme: "light" | "dark";
    let fittedInitialRoute = false;
    let dragging = $state(false);
    // Raw: a deep proxy of the hit would make each read of its coordinate in the route search a proxy call.
    let hover = $state.raw<LineHit | null>(null);
    let overOverlay = $state(false);
    let overStop = $state<string | null>(null);
    const townNames = new Map<string, maplibregl.Marker>();
    let insertDot: maplibregl.Marker;
    let hoverDot: maplibregl.Marker;
    let press: { hit: LineHit; start: maplibregl.Point; moved: boolean } | null = null;
    let sketch: Coordinate[] | null = null;
    let sketchEnd: maplibregl.Point;
    // A finished drawing or insert must not also count as a click on the map.
    let consumedPress = false;
    let wholeRoute = false;
    let overlayLayer: RouteOverlays | undefined;
    // Terrain and overlays wait for the first complete basemap, so their downloads never delay it.
    let basemapComplete = false;
    let overlayStatus = $state('');
    let overlayRetry = $state(false);
    let overlaySelection = $state<OverlaySelection | null>(null);
    let routeHint = $state<RouteHint | null>(null);
    // The places of a mark that the callout lists, at the level of the click.
    let placeList = $state.raw<{ coordinate: Coordinate; places: TilePlace[]; opener: HTMLButtonElement; level: number } | null>(null);
    // The callout holds the list of a mark, else the overlay details at their click, else the popup of the page at `callout`.
    const calloutAt = $derived(placeList?.coordinate ?? overlaySelection?.coordinate ?? (popup ? callout : null));
    // A popup of the page at another place takes the callout from the list and the overlay details. The place is compared
    // by value: the page can pass an equal coordinate in a new array.
    const calloutPlace = $derived(callout?.join());
    $effect(() => { void calloutPlace; overlaySelection = null; placeList = null; });
    const picking = $derived(pickMode || !!onPick);

    function inspectOverlay(event: maplibregl.MapMouseEvent) {
        const selected = overlayLayer?.hit(event);
        if (!selected || drawing || picking) return;
        event.preventDefault();
        cancelGesture();
        consumedPress = true;
        overlayLayer?.hover();
        overlaySelection = selected;
        placeList = null;
    }

    /** The name of the basemap place nearest to `coordinate` within 5 km, such as a village or a hamlet, from the loaded tiles. */
    export function placeName(coordinate: Coordinate): string | undefined {
        let best: { name: string; km: number } | undefined;
        for (const feature of map?.querySourceFeatures("basemap", { sourceLayer: "places", filter: ["==", ["get", "kind"], "locality"] }) ?? []) {
            const name = feature.properties.name, at = feature.geometry.type === "Point" ? feature.geometry.coordinates as Coordinate : undefined;
            const km = at && kilometres(at, coordinate);
            if (name && km !== undefined && km < 5 && (!best || km < best.km)) best = { name, km };
        }
        return best?.name;
    }

    export function fitRoute() {
        if (!map || !coordinates.length) return;
        fitBounds(coordinates, 14, 0);
        wholeRoute = true;
    }

    export function showPlace(coordinate: Coordinate, detail: number) {
        if (!map) return;
        wholeRoute = false;
        // A tall map shows the place below the centre, so its callout opens above it; a short one keeps it centred.
        const height = map.getContainer().clientHeight;
        const below = height >= 560 ? Math.min(150, height / 5) : 0;
        map.flyTo({ center: coordinate, zoom: detail, offset: [-25, below], duration: motionDuration() }, { preserveSearch: true });
    }

    export function fitCoordinates(region: Coordinate[], preserveSearch = false) {
        if (!map || !region.length) return;
        wholeRoute = false;
        fitBounds(region, 13, motionDuration(), preserveSearch);
    }

    /** Fits the places of a stop that opens, with room for the area of a town. */
    export function showStop(places: Coordinate[]) {
        if (!map || !places.length) return;
        wholeRoute = false;
        fitBounds(places, 13, motionDuration(), false, openTownGrowth);
    }

    /** Shows the end of a day and the places of the stops of its night, as `nightView` says. The fit leaves room for the
     * areas of the towns and their names, and goes no closer than zoom 13. */
    export function showNight(spot: Coordinate, towns: Coordinate[][]) {
        if (!map) return;
        const shown = nightView(paddedView(), spot, towns);
        if (shown.action === 'stay') return;
        wholeRoute = false;
        const [west, south, east, north] = shown.box;
        fitBounds([[west, south], [east, north]], 13, motionDuration(), false, 48);
    }

    /** Fits the line of a week, unless the padded view shows all of it. */
    export function showWeek(line: Coordinate[]) {
        if (!map || !line.length) return;
        // One pixel of slack: after "Show whole route" the route touches the padded view.
        const [west, south, east, north] = paddedView(1);
        if (!line.every(([lng, lat]) => lng >= west && lng <= east && lat >= south && lat <= north)) fitCoordinates(line);
    }

    /** The view inside the fit padding less `slack` pixels, clear of the controls and of a bar over the bottom, as
     * [west, south, east, north]. */
    function paddedView(slack = 0): [number, number, number, number] {
        const { clientWidth: width, clientHeight: height } = map!.getContainer(), { top, right, bottom, left } = padding();
        const northWest = map!.unproject([left - slack, top - slack]), southEast = map!.unproject([width - right + slack, height - bottom + slack]);
        return [northWest.lng, southEast.lat, southEast.lng, northWest.lat];
    }

    /** The coordinate under a viewport point, or null outside the map or the region. */
    export function coordinateAtScreen(x: number, y: number): Coordinate | null {
        const box = container.getBoundingClientRect();
        if (!map || x < box.left || x > box.right || y < box.top || y > box.bottom) return null;
        return place(map.unproject([x - box.left, y - box.top]));
    }

    /** Metres on the ground per CSS pixel at a latitude: the world is 512 px wide at zoom 0. */
    export function metresPerPixel(latitude: number): number {
        return map ? 40_075_016.686 * Math.cos(latitude * Math.PI / 180) / (512 * 2 ** map.getZoom()) : 0;
    }

    export function zoomBy(delta: number) {
        if (!map) return;
        wholeRoute = false;
        map.zoomTo(map.getZoom() + delta, { duration: motionDuration() });
    }

    let grown = false;
    // The map opens inside the region; once it has a size, it may show one region size around it, so that "Show whole
    // route" fits a trip that fills the region. In a hidden page MapLibre fits the region to 400 × 300 instead.
    function growBounds() {
        if (grown || !map || !container.clientWidth || !container.clientHeight) return;
        grown = true;
        const [west, south, east, north] = config.bounds, width = east - west, height = north - south;
        map.setMaxBounds([[Math.max(-180, west - width), Math.max(-85, south - height)], [Math.min(180, east + width), Math.min(85, north + height)]]);
    }
    // Every path that adds or moves something takes its place from here: nothing goes into the margin around the region.
    const place = (at: { lng: number; lat: number }) => regionPlace(config.bounds, at);
    const boundsOf = (region: Coordinate[]) => region.reduce((bounds, coordinate) => bounds.extend(coordinate), new maplibregl.LngLatBounds());
    const padding = () => ({ ...fitPadding, bottom: fitPadding.bottom + bottomInset });

    function fitBounds(region: Coordinate[], maxZoom: number, duration: number, preserveSearch = false, room = 0) {
        const { top, right, bottom, left } = padding();
        map!.fitBounds(boundsOf(region), { padding: { top: top + room, right: right + room, bottom: bottom + room, left: left + room }, maxZoom, duration }, { preserveSearch });
    }

    function motionDuration() {
        return window.matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 400;
    }

    function lineFeature(input: Coordinate[], properties: Record<string, string | number> = {}): Feature<LineString> {
        // Svelte may proxy nested point arrays; worker messages require plain coordinate values.
        const plain = input.map(([longitude, latitude]) => [longitude, latitude]);
        return { type: "Feature", properties, geometry: { type: "LineString", coordinates: plain } };
    }

    function lineData(lines: Feature<LineString>[]): FeatureCollection<LineString> {
        return { type: "FeatureCollection", features: lines };
    }

    function tripData() {
        return lineData([...segments.map(({ coordinates, color, legEndId, leg, day }) => lineFeature(coordinates, { color, legEndId, leg, day })),
            ...gaps.map(gap => lineFeature(gap, { leg: "gap" }))]);
    }

    function linesData() {
        return lineData(lines.map(({ coordinates, color, width }, order) => ({ ...lineFeature(coordinates), properties: { color, width, order } })));
    }

    function highlightData() {
        return lineData(highlightedLines.filter(line => line.length >= 2).map(line => lineFeature(line)));
    }

    function restData() {
        const rest = part ? [routeSlice(coordinates, 0, part[0]), routeSlice(coordinates, part[1], 1)] : [];
        return lineData(rest.filter(line => line.length >= 2).map(line => lineFeature(line)));
    }

    function setSketch(line: Coordinate[]) {
        (map?.getSource("planner-sketch") as GeoJSONSource | undefined)?.setData(lineData(line.length < 2 ? [] : [lineFeature(line)]));
    }

    function fitInitialRoute() {
        if (fittedInitialRoute || !map || coordinates.length < 2) return;
        fittedInitialRoute = true;
        fitRoute();
    }

    /** The areas of the towns, under the route. A lit area is warm with a solid edge; the others have a dashed edge. */
    function installTowns() {
        if (!map || map.getSource("towns")) return;
        const dark = theme === "dark", ink = dark ? "#f2efe3" : "#1c1b14";
        const warm: maplibregl.ExpressionSpecification = ["get", "lit"], round = { "line-cap": "round", "line-join": "round" } as const;
        map.addSource("towns", { type: "geojson", data: lineData([]) });
        map.addLayer({ id: "towns-fill", type: "fill", source: "towns", paint: { "fill-color": ["case", warm, dark ? "#5a4622" : "#fff2d1", dark ? "#201f17" : "#ffffff"], "fill-opacity": ["case", warm, .82, .45] } });
        // MapLibre measures a dash in line widths, and a round cap adds half a width to each end.
        map.addLayer({ id: "towns-edge", type: "line", source: "towns", filter: ["!", warm], layout: round, paint: { "line-color": ink, "line-width": 2, "line-dasharray": [1.75, 3.25] } });
        map.addLayer({ id: "towns-edge-lit", type: "line", source: "towns", filter: warm, layout: round, paint: { "line-color": ink, "line-width": 2.5 } });
    }

    /** Draws each town as the hull of its places on the screen, grown by a fixed number of pixels, so the areas are drawn
     * again at each zoom. The name stands beside the middle of the area, on the side with more room. */
    function drawTowns() {
        const source = map?.getSource("towns") as GeoJSONSource | undefined;
        if (!map || !source) return;
        for (const [id, marker] of townNames) if (!towns.some(town => town.id === id)) { marker.remove(); townNames.delete(id); }
        const room = container.clientWidth - controlsWidth, features: Feature<Polygon>[] = [];
        for (const { id, name, places, open = false } of towns) {
            const ring = grownHull(places.map(place => map!.project(place)), open ? openTownGrowth : townGrowth);
            const ys = ring.map(point => point.y), middle = (Math.min(...ys) + Math.max(...ys)) / 2, [west, east] = span(ring, middle), left = west > room - east;
            const outline = ring.map(({ x, y }) => map!.unproject([x, y]).toArray());
            features.push({ type: "Feature", properties: { id, open, lit: open || id === litStop }, geometry: { type: "Polygon", coordinates: [[...outline, outline[0]]] } });
            let marker = townNames.get(id);
            if (!marker) {
                // The list holds each stop for the keyboard and for a screen reader.
                const element = Object.assign(document.createElement("div"), { className: "planner-map-town", ariaHidden: "true" });
                element.dataset.stop = id;
                townNames.set(id, marker = new maplibregl.Marker({ element, anchor: "left" }).setLngLat([0, 0]).addTo(map));
            }
            const element = marker.getElement();
            element.textContent = name;
            // The list of the open town holds its places.
            if (!open) element.append(Object.assign(document.createElement("em"), { textContent: String(places.length) }));
            element.classList.toggle("left", left);
            element.classList.toggle("lit", !open && id === litStop);
            element.classList.toggle("open", open);
            marker.setLngLat(map.unproject([left ? west - 5 : east + 5, middle]));
        }
        source.setData({ type: "FeatureCollection", features });
    }

    /** The town whose area is under a point of the map. The area takes the pointer from the line of the route under it. */
    function townAt(point: maplibregl.Point): string | null {
        return map?.getLayer("towns-fill") ? map.queryRenderedFeatures(point, { layers: ["towns-fill"], filter: ["!", ["get", "open"]] })[0]?.properties.id ?? null : null;
    }

    /** The stop of a night under the pointer: a town by its area or its name, a lone place by its pin. */
    function stopAt(event: maplibregl.MapMouseEvent): string | null {
        const target = event.originalEvent.target;
        if (drawing || picking || !(target instanceof Element)) return null;
        return target === map!.getCanvas() ? townAt(event.point) : target.closest<HTMLElement>("[data-stop]")?.dataset.stop ?? null;
    }

    // The look of each riding day that is set as a feature state of `trip`; a day with none is in its colour.
    const looks = new Map<number, ReturnType<typeof dayLook>>();

    function installRoute() {
        if (!map || map.getSource("trip")) return;
        const dark = theme === "dark";
        const casing = dark ? "#201f17" : "#ffffff";
        const round = { "line-cap": "round", "line-join": "round" } as const;
        // A feature state changes the paint of the loaded tiles; a new paint expression would load the whole line again.
        const look = (colour: unknown, quiet: unknown, lit = colour) => ["match", ["feature-state", "look"], "quiet", quiet, "lit", lit, colour] as maplibregl.ExpressionSpecification;
        const casingWidth = look(8, 5.5, 9);
        map.addSource("trip-highlight", { type: "geojson", data: highlightData() });
        map.addLayer({ id: "trip-highlight", type: "line", source: "trip-highlight", layout: round, paint: { "line-color": dark ? "#5a4622" : "#fbe6b8", "line-width": 14 } });
        map.addSource("trip", { type: "geojson", data: tripData(), promoteId: "day" });
        looks.clear();
        map.addLayer({ id: "trip-casing", type: "line", source: "trip", filter: ["==", ["get", "leg"], "routed"], layout: round, paint: { "line-color": casing, "line-width": casingWidth } });
        map.addLayer({ id: "trip-casing-drawn", type: "line", source: "trip", filter: ["==", ["get", "leg"], "drawn"], layout: { "line-join": "round" }, paint: { "line-color": look(dark ? "#bdb47e" : "#5c5a2e", casing), "line-width": casingWidth, "line-dasharray": [1, 0.8] } });
        map.addLayer({ id: "trip-gap", type: "line", source: "trip", filter: ["==", ["get", "leg"], "gap"], paint: { "line-color": dark ? "#bdb47e" : "#5c5a2e", "line-width": 3, "line-dasharray": [1, 0.8] } });
        map.addLayer({ id: "trip-line", type: "line", source: "trip", filter: ["!=", ["get", "leg"], "gap"], layout: round, paint: { "line-color": look(["get", "color"], dark ? "#6f6a55" : "#a9a593"), "line-width": look(4, 2.5, 6) } });
        map.addSource("trip-rest", { type: "geojson", data: restData() });
        map.addLayer({ id: "trip-rest", type: "line", source: "trip-rest", layout: round, paint: { "line-color": casing, "line-width": 5, "line-opacity": .65 } });
        map.addSource("lines", { type: "geojson", data: linesData() });
        const sorted: maplibregl.LineLayerSpecification["layout"] = { ...round, "line-sort-key": ["get", "order"] };
        map.addLayer({ id: "lines-casing", type: "line", source: "lines", layout: sorted, paint: { "line-color": casing, "line-width": ["+", ["get", "width"], 3] } });
        map.addLayer({ id: "lines", type: "line", source: "lines", layout: sorted, paint: { "line-color": ["get", "color"], "line-width": ["get", "width"] } });
        map.addSource("planner-sketch", { type: "geojson", data: lineData([]) });
        map.addLayer({ id: "planner-sketch", type: "line", source: "planner-sketch", layout: round, paint: { "line-color": dark ? "#f175c5" : "#cc2a93", "line-width": 3, "line-dasharray": [1.5, 1.5] } });
        syncRouteVisibility();
    }

    function syncRouteVisibility() {
        if (!map?.getLayer("trip-line")) return;
        for (const id of ["trip-line", "trip-casing", "trip-casing-drawn", "trip-gap", "trip-rest", "trip-highlight"]) {
            map.setLayoutProperty(id, "visibility", showRoute ? "visible" : "none");
        }
    }

    function syncTerrainAndOverlays() {
        if (basemapComplete) overlayLayer?.install(theme);
        syncTerrain();
    }

    // A hidden layer leaves its source unused, so MapLibre requests none of its tiles.
    function syncTerrain() {
        if (!map?.getLayer("relief")) return;
        for (const id of ["contour-lines", "contour-labels"]) map.setLayoutProperty(id, "visibility", basemapComplete && contours ? "visible" : "none");
        map.setLayoutProperty("relief", "visibility", basemapComplete && hillshade ? "visible" : "none");
    }

    function reportView(event?: { type: string; preserveSearch?: boolean }) {
        if (!map) return;
        const bounds = map.getBounds();
        untrack(() => onBounds?.([bounds.getWest(), bounds.getSouth(), bounds.getEast(), bounds.getNorth()], !!event?.preserveSearch));
        if (!onVisibleRange || !coordinates.length) return;
        const visible = coordinates.flatMap((coordinate, index) => bounds.contains(coordinate) ? [index] : []);
        if (visible.length) onVisibleRange([visible[0], visible[visible.length - 1]]);
    }

    // The line of a day outside the week takes no edits: its shaping points are hidden, and it crosses the whole map, so
    // an insert drag would catch many pans. A press on it pans the map and a click opens its day.
    const quiet = (day: number) => dayLook(week, null, day) === "quiet";

    /** The nearest point on a leg within reach of a screen position. */
    function lineHit(point: maplibregl.Point): LineHit | null {
        if (!map || !showRoute || townAt(point)) return null;
        const best = nearestOnScreen(segments, coordinate => map!.project(coordinate), at => map!.unproject([at.x, at.y]).toArray(), point, lineReach, segment => quiet(segment.day));
        if (!best) return null;
        const at = map.unproject([best.x, best.y]);
        return { legEndId: best.line.legEndId, day: best.line.day, coordinate: [at.lng, at.lat] };
    }

    const level = () => Math.min(LAST_LEVEL, Math.floor(map!.getZoom()));
    const asPlace = (place: TilePlace) => poiPlace(place.id, place.kind, place.name, [place.lon, place.lat])!;

    /** A click on the places of a mark opens the place, lists the places in the callout, or fits the map to them. */
    function openMark(coordinate: Coordinate, places: TilePlace[], opener: HTMLButtonElement) {
        overlaySelection = null;
        // The popup of the page can stay at the same place, so its effect does not clear the list.
        placeList = null;
        const action = opens(places, level());
        if (action === "place") return onPlaceClick?.(asPlace(places[0]));
        const region = places.map((place): Coordinate => [place.lon, place.lat]);
        // A fit that reaches no finer level would ask for the same fit at the next click, so the list opens instead.
        const zoom = action === "zoom" ? map!.cameraForBounds(boundsOf(region), { padding: padding(), maxZoom: FIT_LEVEL })?.zoom ?? 0 : 0;
        if (Math.floor(zoom) > level()) {
            wholeRoute = false;
            fitBounds(region, FIT_LEVEL, motionDuration());
        } else placeList = { coordinate, places, opener, level: level() };
    }

    /** Closes the list and gives the focus back to its opener, or to the map when the opener is gone. */
    // Escape reaches the window listener before the list's own: the second call must not move the focus again.
    function closeList() {
        if (!placeList) return;
        const opener = placeList.opener;
        placeList = null;
        // A popup of the page that comes back focuses itself in its first frame; the opener takes the focus after it.
        tick().then(() => requestAnimationFrame(() => (opener?.isConnected ? opener : map?.getCanvas())?.focus({ preventScroll: true })));
    }

    function legEnds(legEndId: string): [Coordinate, Coordinate] {
        const legSegments = segments.filter((segment) => segment.legEndId === legEndId);
        return [legSegments[0].coordinates[0], legSegments.at(-1)!.coordinates.at(-1)!];
    }

    function pressMap(event: maplibregl.MapMouseEvent) {
        consumedPress = false;
        if (event.originalEvent.button !== 0 || event.originalEvent.target !== map?.getCanvas()) return;
        const coordinate: Coordinate = [event.lngLat.lng, event.lngLat.lat];
        if (drawing) {
            sketch = [coordinate];
            sketchEnd = event.point;
        } else if (!picking && (hover = lineHit(event.point)) && !quiet(hover.day)) {
            // Stops the map from panning: this press may drag a new point out of the line.
            event.preventDefault();
            press = { hit: hover, start: event.point, moved: false };
        }
    }

    function trackPointer(event: maplibregl.MapMouseEvent) {
        if (!map) return;
        const coordinate: Coordinate = [event.lngLat.lng, event.lngLat.lat];
        if (sketch) {
            if (event.point.dist(sketchEnd) < sketchStep) return;
            sketch.push(coordinate);
            sketchEnd = event.point;
            setSketch(sketch);
        } else if (press) {
            press.moved ||= event.point.dist(press.start) > sketchStep;
            if (!press.moved) return;
            const [from, to] = legEnds(press.hit.legEndId);
            insertDot.setLngLat(coordinate);
            setSketch([from, coordinate, to]);
        } else {
            const onMap = event.originalEvent.target === map.getCanvas();
            hover = onMap && !dragging && !draggingPin && !drawing && !picking ? lineHit(event.point) : null;
            overStop = dragging || draggingPin ? null : stopAt(event);
            overOverlay = overlayLayer?.hover(onMap && !hover && !overStop && !dragging && !draggingPin && !drawing && !picking ? event : undefined) ?? false;
        }
    }

    function releaseMap(event: maplibregl.MapMouseEvent) {
        if (event.originalEvent.target !== map?.getCanvas()) { cancelGesture(); return; }
        if (sketch && drawing) {
            const drawn = sketch;
            sketch = null;
            consumedPress = true;
            setSketch([]);
            if (drawn.length > 1 && drawn.every(([lng, lat]) => place({ lng, lat }))) onDrawn?.(drawing, drawn);
        } else if (press) {
            // A press without a drag stays a click, which opens the leg callout.
            const { hit, moved } = press;
            press = null;
            if (!moved) return;
            consumedPress = true;
            hover = null;
            setSketch([]);
            const at = place(event.lngLat);
            if (at) onInsert?.(hit.legEndId, at);
        }
    }

    function cancelGesture() {
        if (!press && !sketch) return;
        press = null;
        sketch = null;
        hover = null;
        consumedPress = true;
        setSketch([]);
    }

    onMount(() => {
        appliedTheme = theme;
        useMapProtocols();
        const terrainLease = terrain.acquire(maplibregl);
        // The switches of the map show or hide the relief and the contour lines (`syncTerrainAndOverlays`).
        relief = terrainLease.relief();
        insertDot = new maplibregl.Marker({ element: Object.assign(document.createElement("div"), { className: "planner-insert-dot" }) });
        hoverDot = new maplibregl.Marker({ element: Object.assign(document.createElement("div"), { className: "planner-hover-dot" }) });
        try {
            // `zoomLevelsToOverscale` must stay `undefined`: its default splits the tiles of each vector source past the deepest
            // zoom of the source, and keeps only the source layers that a style layer draws. The route details read a source
            // layer that no style layer draws.
            map = new maplibregl.Map({ container, center, zoom, maxBounds: config.bounds, style: mapStyle(theme, styleConfig, relief), attributionControl: false, maxPitch: 0, renderWorldCopies: false, zoomLevelsToOverscale: undefined });
            // The script of the tile pictures moves the map.
            if (import.meta.env.DEV) Object.assign(window, { plannerMap: map });
            // The callout fits clear of the controls column and the bottom strip.
            calloutPopup = mapCallout(map, popupContent, { offset: calloutOffset,
                free: () => ({ left: 16, top: 16, right: container.clientWidth - controlsWidth, bottom: container.clientHeight - bottomClear }) });
            overlayLayer = new RouteOverlays(map, config.overlays, (message, retry = false) => { overlayStatus = message; overlayRetry = retry; }, hint => routeHint = hint);
            growBounds();
            fitInitialRoute();
            map.addControl(new ClosedAttribution(), "bottom-right");
            map.addControl(new maplibregl.ScaleControl({ maxWidth: 90, unit: "metric" }), "bottom-left");
            map.dragRotate.disable();
            map.touchZoomRotate.disableRotation();
            map.on("style.load", () => { ready = true; installTowns(); installRoute(); syncTerrainAndOverlays(); syncDataLayer(); });
            map.once("load", () => { basemapComplete = true; syncTerrainAndOverlays(); });
            map.setMissingStyleImageResolver((id) => {
                const icon = mapIcon(id);
                if (icon && !map!.hasImage(id)) map!.addImage(id, icon.image, { pixelRatio: icon.pixelRatio });
            });
            map.once("load", reportView);
            const terrainError = terrainRetry(map);
            map.on("error", (event) => {
                if ('name' in event.error && event.error.name === "AbortError") return;
                if (terrainError(event)) {
                    console.warn("Planner terrain:", event.error);
                    return;
                }
                // Data layers are decoration too, and each shows its own error. Their sources are named by the layer id.
                const source = (event as { sourceId?: string }).sourceId;
                if (source && dataLayer?.layers.some(layer => source === layer.id || source.startsWith(`${layer.id}-`))) {
                    console.warn("Planner data layer:", event.error);
                    return;
                }
                failure = "Some map data could not load. Check your connection, then retry.";
                errorDetail = event.error.message;
                console.error("Planner map:", event.error);
                // MapLibre does not repaint after a failed request, so the first load would wait for a camera move.
                // A failed tile also marks its source loaded, so an earlier repaint would fire load too soon.
                if (!basemapComplete && map!.areTilesLoaded()) map!.triggerRepaint();
            });
            map.on("click", (event) => {
                const target = event.originalEvent.target;
                if (consumedPress || drawing || (target instanceof Node && popupContent?.contains(target))) return;
                const at = place(event.lngLat);
                if (onPick) { if (at) onPick(at, [event.originalEvent.clientX, event.originalEvent.clientY]); return; }
                const stop = stopAt(event);
                if (stop) { onStopOpen?.(stop); return; }
                const hit = pickMode ? null : lineHit(event.point);
                const overlay = pickMode ? null : overlayLayer?.hit(event);
                overlaySelection = null;
                placeList = null;
                if (overlay?.kind === 'access') overlaySelection = overlay;
                else if (hit && quiet(hit.day)) onDayClick?.(hit.day);
                else if (hit) onLegClick?.(hit.legEndId, hit.coordinate);
                else onEmptyClick?.(at);
            });
            // MapLibre makes its canvas focusable, and the arrow keys pan it.
            map.getCanvas().addEventListener("keydown", (event) => {
                const at = map && place(map.getCenter());
                if (event.key !== "Enter" || !onPick || !at) return;
                const box = container.getBoundingClientRect();
                onPick(at, [box.left + box.width / 2, box.top + box.height / 2]);
            });
            map.on('contextmenu', inspectOverlay);
            map.on('touchstart', () => consumedPress = false);
            map.on('touchend', event => {
                // A long press must not also add a point through a synthetic click.
                if (consumedPress) event.originalEvent.preventDefault();
            });
            map.on("mousedown", pressMap);
            map.on("mousemove", trackPointer);
            map.on("mouseup", releaseMap);
            map.on("mouseout", () => {
                if (!press) hover = null; overOverlay = false; overStop = null; overlayLayer?.hover();
            });
            map.on("movestart", (event) => { onMoveStart?.(); if (event.originalEvent) { wholeRoute = false; onRiderMove?.(); } overOverlay = false; overlayLayer?.hover(); });
            map.on("moveend", reportView);
            map.on("zoom", drawTowns);
            map.on("moveend", drawTowns);
            map.on("idle", () => loading = false);
            map.on("dataloading", () => loading = true);
            map.on("movestart", () => loading = true);
            map.on("zoom", () => {
                if (!placeList || placeList.level === level()) return;
                if (popupContent.contains(document.activeElement)) closeList();
                else placeList = null;
            });
        } catch (error) {
            failure = "The map could not start. This view needs a browser with WebGL enabled.";
            errorDetail = error instanceof Error ? error.message : String(error);
        }
        map?.on("dragstart",()=>dragging=true);
        map?.on("dragend",()=>dragging=false);
        let refit: ReturnType<typeof setTimeout> | undefined;
        const observer = new ResizeObserver(() => {
            map?.resize();
            growBounds();
            if (!wholeRoute) return;
            clearTimeout(refit);
            refit = setTimeout(() => { if (wholeRoute) fitRoute(); }, 150);
        });
        observer.observe(container);
        const preventDrag = (event: DragEvent) => event.preventDefault();
        container.addEventListener('dragstart', preventDrag);
        return () => {
            container.removeEventListener('dragstart', preventDrag);
            observer.disconnect();
            clearTimeout(refit);
            pins.forEach(({ marker }) => marker.remove());
            townNames.forEach(marker => marker.remove());
            insertDot.remove();
            hoverDot.remove();
            calloutPopup?.remove();
            overlayLayer?.destroy();
            map?.remove();
            terrainLease.release();
        };
    });

    $effect(() => {
        if (!map || !ready || appliedTheme === theme) return;
        appliedTheme = theme;
        ready = false;
        map.setStyle(mapStyle(theme, styleConfig, relief));
    });
    $effect(() => {
        coordinates;
        if (map && ready) {
            fitInitialRoute();
            (map.getSource("trip") as GeoJSONSource | undefined)?.setData(tripData());
            reportView();
        }
    });
    const lastDay = $derived(segments.at(-1)?.day ?? 0);
    // A week step changes the looks of a few days and sends no line to the map.
    $effect(() => {
        const shown = Array.from({ length: lastDay }, (_, i) => dayLook(week, lit, i + 1));
        if (!map || !ready) return;
        if (shown.every(look => look === "colour")) {
            if (looks.size) map.removeFeatureState({ source: "trip" });
            looks.clear();
            return;
        }
        shown.forEach((look, i) => {
            if ((looks.get(i + 1) ?? "colour") === look) return;
            looks.set(i + 1, look);
            map!.setFeatureState({ source: "trip", id: i + 1 }, { look });
        });
    });
    $effect(() => { void [towns, litStop]; if (ready) untrack(drawTowns); });
    $effect(() => { const stop = overStop; untrack(() => onStopHover?.(stop)); });
    $effect(() => { hillshade; contours; if (ready) syncTerrain(); });
    $effect(() => { const options = { ...mapOverlays }; const mode = accessMode; if (ready) { overlaySelection = null; overOverlay = false; overlayLayer?.set(options, mode); } });
    $effect(() => { void showRoute; if (ready) syncRouteVisibility(); });
    $effect(() => { void lines; if (ready) (map?.getSource("lines") as GeoJSONSource | undefined)?.setData(linesData()); });
    function syncDataLayer() {
        if (!map || !dataLayer) return;
        const { layers, shown, date } = dataLayer;
        // The hidden layers first, so a hidden layer never undoes what the shown one set on shared base layers.
        for (const layer of layers) if (layer !== shown) layer.sync(map, { shown: false, date, theme });
        shown?.sync(map, { shown: true, date, theme });
    }
    $effect(() => { void [dataLayer?.shown, dataLayer?.date, dataLayer?.shown?.variable?.value]; if (ready) untrack(syncDataLayer); });
    const notes: maplibregl.Marker[] = [];
    $effect(() => {
        const list = showRoute ? dataLayer?.notes ?? [] : [];
        if (!map) return;
        for (const marker of notes.splice(0)) marker.remove();
        for (const { coordinate, text } of list) {
            const element = Object.assign(document.createElement('div'), { className: 'planner-map-note', textContent: text });
            notes.push(new maplibregl.Marker({ element, anchor: 'left', offset: [16, 0] }).setLngLat(coordinate).addTo(map));
        }
    });
    $effect(() => { const routing = routingPackage; if (ready) overlayLayer?.verify(routing); });
    $effect(() => {
        const data = restData();
        if (map && ready) (map.getSource("trip-rest") as GeoJSONSource | undefined)?.setData(data);
    });
    $effect(() => {
        highlightedLines;
        if (map && ready) (map.getSource("trip-highlight") as GeoJSONSource | undefined)?.setData(highlightData());
    });
    // A hit belongs to the line it was found on: a new line clears the place until the pointer moves again.
    $effect(() => {
        const hit = hover;
        untrack(() => onRouteHover?.(hit && coordinates.length > 1 ? nearestProgress(coordinates, hit.coordinate) : null));
    });
    $effect(() => {
        void coordinates;
        untrack(() => onRouteHover?.(null));
    });
    $effect(() => {
        if (picking) hover = null;
    });
    $effect(() => {
        if (!map) return;
        if (hoverProgress === null || !coordinates.length) { hoverDot.remove(); return; }
        hoverDot.setLngLat(coordinateAt(coordinates, hoverProgress));
        // `addTo` removes and inserts the element again, so it runs only when the dot is absent.
        if (!hoverDot.getElement().isConnected) hoverDot.addTo(map);
    });
    $effect(() => {
        if (!map) return;
        if (hover && !quiet(hover.day)) insertDot.setLngLat(hover.coordinate).addTo(map);
        else insertDot.remove();
    });
    $effect(() => {
        if (!map) return;
        if (drawing) {
            map.dragPan.disable();
            hover = null;
            return;
        }
        map.dragPan.enable();
        sketch = null;
        setSketch([]);
    });
    $effect(() => {
        if (map) map.getCanvas().style.cursor = dragging || draggingPin ? "grabbing" : drawing || picking ? "crosshair" : hover || overOverlay || overStop ? "pointer" : "grab";
    });

    function markerIcon(path: string) {
        const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
        svg.setAttribute("viewBox", "0 0 24 24");
        svg.setAttribute("draggable", "false");
        svg.setAttribute("aria-hidden", "true");
        svg.setAttribute("fill", "none");
        svg.setAttribute("stroke", "currentColor");
        svg.setAttribute("stroke-width", "1.7");
        svg.setAttribute("stroke-linecap", "round");
        svg.setAttribute("stroke-linejoin", "round");
        const element = document.createElementNS("http://www.w3.org/2000/svg", "path");
        element.setAttribute("d", path);
        svg.append(element);
        return svg;
    }

    const pointIcons = {
        waypoint: "M6 21V3m0 1c4-3 8 3 12 0v9c-4 3-8-3-12 0",
        detour: "M8 5 3 10l5 5M3 10h11a5 5 0 0 1 0 10h-2",
    };
    // The start plays and the finish waves a chequered flag, as in the iPhone planner. A loop has only its start.
    const endpointIcons = {
        start: '<path d="M9 6.5v11l9-5.5z" fill="currentColor" />',
        finish: '<path d="M6 21V4h13v9H6" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round" />'
            + '<path d="M6 4h4.3v4.5H6zm8.7 0H19v4.5h-4.3zm-4.4 4.5h4.4V13h-4.4z" fill="currentColor" />',
    };

    function routeProgress(marker: maplibregl.Marker) {
        const at = marker.getLngLat();
        return nearestProgress(coordinates, [at.lng, at.lat]);
    }

    // Keep unchanged marker elements mounted through searches and day selection.
    $effect(() => {
        if (!map || draggingPin) return;
        const shown = points.filter((point) => showRoute || point.kind === "place" || point.kind === "marker");
        const ids = new Set(shown.map(point => point.id));
        for (const [id, pin] of pins) if (!ids.has(id)) { pin.marker.remove(); pins.delete(id); }
        let nightNumber = 0;
        for (const point of shown) {
            if (point.kind === "night") nightNumber++;
            const key = JSON.stringify([point, point.kind === 'night' ? nightNumber : 0]);
            if (pins.get(point.id)?.key === key) continue;
            pins.get(point.id)?.marker.remove();
            const dayEnd = point.kind === "dayend";
            const draggable = dayEnd ? !!onDayEndDrag : !!onPointMove && !point.fixed && point.kind !== "place";
            const button = document.createElement("button");
            button.type = 'button';
            button.draggable = false;
            button.className = `planner-map-pin ${point.kind} ${point.appearance ?? ""}${point.stop ? " lone" : ""}${point.named ? " named" : ""}${draggable ? " draggable" : ""}`;
            if (point.color) button.style.setProperty("--pin-color", point.color);
            button.setAttribute("aria-label", point.kind === "start" ? `Start: ${point.label}` : point.kind === "finish" ? `Finish: ${point.label}` : point.label);
            const title = dayEnd ? `${point.appearance === "moved" ? "Day end you moved" : "Day end suggested"} · drag along the route` : point.label + (draggable ? " · drag to move" : "");
            if ((point.kind === "place" || point.kind === "night") && point.category) {
                button.append(markerIcon(placeCategories[point.category].icon));
            } else if (point.kind === "waypoint" || point.kind === "detour") {
                button.append(markerIcon(pointIcons[point.kind]));
            } else if (point.kind === "start" || point.kind === "finish") {
                button.innerHTML = `<svg viewBox="0 0 24 24" aria-hidden="true" draggable="false">${endpointIcons[point.kind]}</svg>`;
            } else {
                button.textContent = point.markerLabel ?? (point.kind === "night" ? String(nightNumber) : "");
            }
            if (point.stop) button.dataset.stop = point.stop;
            if (point.stop || point.named) button.append(Object.assign(document.createElement("span"), { textContent: point.label }));
            button.addEventListener("click", (event) => {
                event.stopPropagation();
                overlaySelection = null;
                placeList = null;
                if (!onPick) return onPointSelect?.(point.id);
                const box = button.getBoundingClientRect();
                onPick(point.coordinate, [box.left + box.width / 2, box.top + box.height / 2]);
            });
            button.addEventListener('mouseenter', () => onPointHover?.(point.id));
            button.addEventListener('mouseleave', () => onPointHover?.(null));
            button.addEventListener('focus', () => onPointHover?.(point.id));
            button.addEventListener('blur', () => onPointHover?.(null));
            const marker = new maplibregl.Marker({ element: button, draggable }).setLngLat(point.coordinate).addTo(map);
            // Day ends stay last in the DOM, so a place added later at the same spot does not cover the drag handle.
            if (!dayEnd) map.getCanvasContainer().querySelector(".planner-map-pin.dayend")?.before(button);
            marker.on("dragstart", () => { draggingPin = true; hover = null; });
            if (dayEnd) {
                marker.on("drag", () => marker.setLngLat(coordinateAt(coordinates, routeProgress(marker))));
                marker.on("dragend", () => { onDayEndDrag?.(point.night!, routeProgress(marker)); draggingPin = false; });
            } else {
                // Outside the region the preview ends, and a drop there puts the pin back.
                marker.on("drag", () => onPointPreview?.(point.id, place(marker.getLngLat())));
                marker.on("dragend", () => {
                    const at = place(marker.getLngLat());
                    if (at) onPointMove?.(point.id, at);
                    else { marker.setLngLat(point.coordinate); onPointPreview?.(point.id, null); }
                    draggingPin = false;
                });
            }
            pins.set(point.id, { marker, button, key, point, draggable, title });
        }
        madePins = [...pins.values()];
    });
    $effect(() => {
        for (const { point, marker, button, draggable, title } of madePins) {
            // The selected pin stays shown, so its callout never points at a hidden pin.
            const { id } = point, look = id === selectedId ? "shown" : pinLook(week, point);
            button.classList.toggle("selected", id === selectedId);
            button.classList.toggle('highlighted', id === hoveredId || (!!point.stop && point.stop === litStop));
            button.classList.toggle("matched", highlightedPlaceIds.includes(id));
            button.setAttribute("aria-pressed", String(id === selectedId));
            button.classList.toggle("dot", look === "dot");
            button.classList.toggle("off", look === "hidden");
            marker.setDraggable(draggable && look === "shown");
            button.title = look === "dot" ? point.label : title;
        }
    });
    $effect(() => {
        void bottomClear;
        untrack(() => {
            if (wholeRoute) fitRoute();
            calloutPopup?.refit(true);
        });
    });
    $effect(() => {
        const at = calloutAt;
        if (map) untrack(() => calloutPopup?.show(at));
    });

    // In the capture phase, so that Escape closes the route details or the place list before the page goes back a level.
    function escape(event: KeyboardEvent) {
        if (event.key !== 'Escape') return;
        cancelGesture();
        if (!overlaySelection && !placeList) return;
        event.preventDefault();
        overlaySelection = null;
        if (placeList) closeList();
    }
</script>

<svelte:window onmouseup={(event) => { if (event.target !== map?.getCanvas()) cancelGesture(); }}
    onblur={cancelGesture} onkeydowncapture={escape} />

<div class="map-frame" data-map-theme={theme} bind:clientWidth={frameWidth} bind:clientHeight={frameHeight}>
    <div class="map-canvas" bind:this={container} aria-label="Route map" aria-busy={loading && !failure}></div>
    {#if map}
        <PlaceMarks {map} {ready} {theme} url={config.places} kinds={placeKinds} highlight={highlightedKind} still={!!drawing || !!onPick} onOpen={openMark} />
    {/if}
    <div class="popup-storage"><div bind:this={popupContent}>
        {#if placeList}
            <PlaceList places={placeList.places} onClose={closeList} onPick={(place) => { placeList = null; onPlaceClick?.(asPlace(place)); }} />
        {:else if overlaySelection}
            <MapOverlayDetails selection={overlaySelection} {theme} onclose={() => overlaySelection = null} />
        {:else if popup}{@render popup()}{/if}
    </div></div>
    {#if routeHint && !overlaySelection}
        <!-- The note starts from the pointer, or from the edge of the free area when the pointer is beyond it. -->
        {@const right = frameWidth - controlsWidth}
        {@const x = Math.min(routeHint.point.x, right)}
        {@const y = Math.min(routeHint.point.y, frameHeight - bottomClear)}
        {@const left = x > right / 2}
        <!-- The note opens on the side of the pointer with more room, and is never wider than that room. It keeps clear of the controls and the bottom strip. -->
        <div class="planner-map-note route-hint" style:left={`${x}px`} style:top={`${y}px`} style:max-width={`${(left ? x : right - x) - 22}px`}
            style:translate={`${left ? 'calc(-100% - 14px)' : '14px'} ${y > frameHeight - bottomClear - 44 ? 'calc(-100% - 12px)' : '18px'}`}>
            {#if routeHint.name}<b>{routeHint.name}</b>{/if}<span>Right-click for details</span>
        </div>
    {/if}
    {#if overlayStatus && !failure}
        <div class="overlay-status" role="status">{overlayStatus}{#if overlayRetry}<button onclick={() => overlayLayer?.retry()}>Retry</button>{/if}</div>
    {/if}
    {#if !ready && !failure}<div class="map-status" role="status">Loading map…</div>{/if}
    {#if failure}
        <div class="map-status" role="status">
            {failure} <button onclick={() => { failure = ""; errorDetail = ""; map?.setStyle(map!.getStyle()); }}>Retry map</button>
            {#if errorDetail}<details><summary>Error details</summary>{errorDetail}</details>{/if}
        </div>
    {/if}
</div>

<style>
    .map-frame { position: relative; min-height: 240px; height: 100%; isolation: isolate; background: var(--parchment, #f4f2eb); }
    .map-canvas { user-select: none; -webkit-user-select: none; width: 100%; height: 100%; min-height: 240px; }
    .popup-storage { display: none; }
    .overlay-status { position: absolute; left: 12px; bottom: 34px; max-width: calc(100% - 80px); padding: 7px 10px; border-radius: 6px; color: var(--ink); background: var(--panel); font-size: 12px; }
    .overlay-status button { margin-left: 8px; border: 0; background: none; color: var(--link); font: inherit; text-decoration: underline; cursor: pointer; }
    .map-status { position: absolute; bottom: 32px; left: 12px; right: 12px; padding: 10px 12px; background: var(--panel, white); color: var(--ink, #1c1b14); border: 1px solid var(--line-strong, #bcb9aa); font-size: 13px; }
    .map-status button { color: inherit; background: transparent; border: 0; text-decoration: underline; cursor: pointer; font: inherit; }
    :global(.planner-map-pin) { width: 28px; height: 28px; display: grid; place-items: center; padding: 0; border: 2px solid var(--panel, #fff); border-radius: 50%; background: #a4501e; color: #fff; font: 700 13px var(--sans, sans-serif); cursor: pointer; box-shadow: var(--planner-shadow, 0 6px 18px rgba(28, 27, 20, .12)); }
    :global(.planner-map-pin.draggable) { cursor: grab; }
    :global(.planner-map-pin:is(.start, .finish)) { color: var(--ink, #1c1b14); background: var(--panel, #fff); border: 1.5px solid var(--ink-soft, #5c5a2e); }
    :global(.planner-map-pin.finish) { border-radius: 7px; }
    :global(.planner-map-pin:is(.start, .finish) svg) { width: 16px; height: 16px; }
    :global(.planner-map-pin.via) { width: 20px; height: 20px; background: var(--route, #cc2a93); }
    :global(.planner-map-pin.pass) { width: 22px; height: 22px; background: var(--panel, #fff); border: 3px solid var(--route, #cc2a93); }
    :global(.planner-map-pin.marker) { width: 24px; height: 24px; background: #e7ecdf; border-color: var(--ink-soft, #5c5a2e); color: #1c1b14; }
    :global(.planner-map-pin.marker:empty)::after { content: ""; width: 6px; height: 6px; border-radius: 50%; background: currentColor; }
    :global(.planner-map-pin.place) { width: 32px; height: 32px; color: var(--ink-soft, #5c5a2e); background: var(--panel, #fff); border-color: var(--line-strong, rgba(92, 90, 46, .32)); }
    :global([data-theme="dark"] .planner-map-pin.place) { color: var(--cream, #fff2d1); }
    :global(.planner-map-pin svg) { width: 21px; height: 21px; }
    :global(.planner-map-pin.waypoint) { background: var(--ink-soft, #5c5a2e); }
    :global(.planner-map-pin.detour) { background: var(--panel, #fff); color: var(--ink-soft, #5c5a2e); border-color: var(--ink-soft, #5c5a2e); }
    :global(.planner-map-pin.detour svg) { width: 18px; height: 18px; }
    :global(.planner-map-pin.suggested) { color: var(--ink-soft, #5c5a2e); background: #e7ecdf; border: 2px dashed var(--ink-soft, #5c5a2e); box-shadow: none; }
    :global(.planner-map-pin.night:not(.suggested)) { color: var(--panel, #fff); background: var(--pin-color, var(--ink, #1c1b14)); }
    :global(.planner-map-pin.dayend) { width: 24px; height: 24px; font: 600 11px var(--sans, sans-serif); color: var(--pin-color); background: var(--panel, #fff); border: 2px dashed var(--pin-color); }
    :global(.planner-map-pin.dayend.moved) { border-style: dotted; }
    :global(.planner-map-pin.dayend.moved)::after { content: ""; position: absolute; bottom: 2px; width: 3px; height: 3px; border-radius: 50%; background: currentColor; }
    :global(.planner-map-pin.dayend.draggable) { cursor: ew-resize; }
    /* Outside the week a night mark is a small dot, and a shaping point is hidden. */
    :global(.planner-map-pin.dot:is(.night, .dayend)) { width: 10px; height: 10px; border: 2px solid color-mix(in srgb, var(--ink-faint) 70%, var(--panel)); background: var(--panel); box-shadow: none; font-size: 0; cursor: pointer; }
    :global(.planner-map-pin.dot > *), :global(.planner-map-pin.dot)::after { display: none; }
    :global(.planner-map-pin.off) { display: none; }
    :global(.planner-map-pin.matched) { border: 3px solid var(--amber, #f4a81d); }
    :global(.planner-map-pin.selected) { outline: 3px solid var(--amber, #f4a81d); outline-offset: 3px; }
    :global(.planner-map-pin.highlighted) { outline: 3px solid var(--ink, #1c1b14); outline-offset: 3px; }
    /* A place of the open stop of a night under the pointer has a soft ring: less than the ink of the selected one. */
    :global(.planner-map-pin.option.highlighted:not(.named)) { outline: 0; box-shadow: 0 0 0 5px color-mix(in srgb, var(--ink) 20%, transparent); }
    /* A lone place of a night is a small pin with its name in the style of a map label. Lit, both are ink. */
    :global(.planner-map-pin.lone) { width: 18px; height: 18px; border: 1.5px solid var(--ink-soft); box-shadow: none; }
    :global(.planner-map-pin.lone svg) { width: 11px; height: 11px; }
    :global(.planner-map-pin:is(.lone, .named) span) { position: absolute; left: 100%; margin-left: 1px; padding: 1px 5px; border-radius: 6px; color: var(--ink); font: 500 12.5px var(--sans); white-space: nowrap; paint-order: stroke; -webkit-text-stroke: 3.5px color-mix(in srgb, var(--panel) 92%, transparent); }
    :global(.planner-map-pin:is(.lone.highlighted, .named)) { border-color: var(--ink); background: var(--ink); color: var(--panel); }
    :global(.planner-map-pin.lone.highlighted:not(:focus-visible)) { outline: 0; }
    :global(.planner-map-pin:is(.lone.highlighted, .named) span), :global(.planner-map-town.lit) { background: var(--ink); color: var(--panel); -webkit-text-stroke: 0; box-shadow: 0 2px 8px rgba(28, 27, 20, .25); }
    /* The name of a town beside its area. The padding is that of the pill of the lit name, so the name stays in place. */
    :global(.planner-map-town) { padding: 2px 9px; border-radius: 6px; color: var(--ink); font: 700 14px var(--sans); white-space: nowrap; paint-order: stroke; -webkit-text-stroke: 4px color-mix(in srgb, var(--panel) 92%, transparent); cursor: pointer; }
    :global(.planner-map-town.left) { translate: -100%; }
    :global(.planner-map-town.open) { pointer-events: none; }
    :global(.planner-map-town em) { margin-left: 5px; color: var(--ink-soft); font: 500 12.5px var(--sans); }
    :global(.planner-map-town.lit em) { color: color-mix(in srgb, var(--panel) 75%, transparent); }
    :global(.planner-map-pin:hover) { filter: brightness(1.08); }
    :global(.planner-map-pin:focus-visible) { outline: 3px solid var(--amber, #f4a81d); outline-offset: 3px; }
    :global(.planner-map-note) { padding: 2px 8px; border-radius: 6px; color: var(--ink, #1c1b14); background: var(--panel, #fff); font: 600 12px/20px var(--sans, sans-serif); font-variant-numeric: tabular-nums; white-space: nowrap; box-shadow: var(--planner-shadow, 0 6px 18px rgba(28, 27, 20, .12)); pointer-events: none; }
    .route-hint { position: absolute; display: flex; gap: 8px; box-sizing: border-box; }
    .route-hint :is(b, span) { min-width: 0; overflow: hidden; text-overflow: ellipsis; }
    /* A long route name gives way before the gesture does. */
    .route-hint b { flex-shrink: 1000; font-weight: inherit; }
    .route-hint span { color: var(--ink-soft, #5c5a2e); font-weight: 400; }
    :global(.planner-hover-dot) { width: 12px; height: 12px; border: 3px solid var(--panel, #fff); border-radius: 50%; background: var(--ink, #1c1b14); pointer-events: none; }
    :global(.planner-insert-dot) { width: 14px; height: 14px; border: 2.5px solid var(--route, #cc2a93); border-radius: 50%; background: var(--panel, #fff); pointer-events: none; }
    .map-frame :global(.maplibregl-ctrl-scale) { padding: 0 6px; border: 1px solid var(--line-strong, #bcb9aa); border-top: 0; background: var(--panel, white); font: 11px/18px var(--sans, sans-serif); color: var(--ink, #1c1b14); }
    .map-frame :global(.maplibregl-ctrl-attrib) { background: var(--panel, white); font: 11px var(--sans, sans-serif); color: var(--ink-soft, #5c5a2e); }
    .map-frame :global(.maplibregl-ctrl-attrib a) { color: var(--link, var(--ink-soft, #5c5a2e)); }
    .map-frame :global(.maplibregl-ctrl-attrib.maplibregl-compact-show .maplibregl-ctrl-attrib-button) { background-color: var(--parchment-2, #e7ecdf); }
    .map-frame[data-map-theme="dark"] :global(.maplibregl-ctrl-attrib-button) { background-image: url("data:image/svg+xml;charset=utf-8,%3Csvg xmlns='http://www.w3.org/2000/svg' width='24' height='24' fill-rule='evenodd' viewBox='0 0 20 20'%3E%3Cpath d='M4 10a6 6 0 1 0 12 0 6 6 0 1 0-12 0m5-3a1 1 0 1 0 2 0 1 1 0 1 0-2 0m0 3a1 1 0 1 1 2 0v3a1 1 0 1 1-2 0' fill='%23f2efe3'/%3E%3C/svg%3E"); }
</style>
