<script module lang="ts">
    import { terrainRetry, terrainSource } from '../../lib/planner/map-terrain';
    import { TERRAIN_URL } from '../../lib/planner/map-data';
    const terrain = terrainSource(TERRAIN_URL);
</script>

<script lang="ts">
    import { onMount, untrack, type Snippet } from "svelte";
    import * as maplibregl from "maplibre-gl";
    import mapWorkerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url";
    import type { GeoJSONSource } from "maplibre-gl";
    import type { Feature, FeatureCollection, LineString, Point } from "geojson";
    import { Protocol } from "pmtiles";
    import mlcontour from "maplibre-contour";
    import "maplibre-gl/dist/maplibre-gl.css";
    import { mapStyle, poiFilter } from "../../lib/planner/map-style";
    import { mapIcon } from "../../lib/planner/map-icons";
    import { MAP_BOUNDS } from "../../lib/planner/map-data";
    import { categoryIds, placeCategories, type PlaceCategory } from "../../lib/planner/poi-kinds";
    import { poiPlace } from "../../lib/planner/place-index";
    import { coordinateAt, nearestProgress, type Place } from "../../lib/planner/editor";
    import type { Coordinate, MapPoint, MapSegment } from "../../lib/planner/map-types";
    import { RouteOverlays, type AccessMode, type OverlayOptions, type OverlaySelection } from "../../lib/planner/route-overlays";
    import MapOverlayDetails from './MapOverlayDetails.svelte';

    let {
        segments = [], coordinates = [], highlightedCoordinates = [], points = [], selectedId = null, hoveredId = null, callout = null,
        drawing = null, highlightedPlaceIds = [], theme = "light", hillshade = true, contours = true, pickMode = false,
        showRoute = true, hoverProgress = null, center = [8.8, 48.65], zoom = 7,
        shownCategories = categoryIds, highlightedPlaces = [], landmarks = [], mapOverlays = { network: 'none', access: false }, accessMode = 'cycling',
        onEmptyClick, onPointSelect, onPointHover, onPointMove, onPointPreview, onDayEndDrag, onLegClick, onInsert, onDrawn, onPlaceClick, onVisibleRange, onBounds, popup,
    }: {
        segments?: MapSegment[]; coordinates?: Coordinate[]; highlightedCoordinates?: Coordinate[]; points?: MapPoint[];
        selectedId?: string | null; callout?: Coordinate | null; drawing?: string | null; highlightedPlaceIds?: string[];
        hoveredId?: string | null;
        /** While picking, any map click places the overnight; the line takes no edits. */
        pickMode?: boolean; showRoute?: boolean; theme?: "light" | "dark"; hillshade?: boolean; contours?: boolean;
        /** Basemap place categories to draw. */
        shownCategories?: PlaceCategory[];
        /** Places drawn with a ring at every zoom. */
        highlightedPlaces?: Place[];
        /** Places to ride over, drawn from zoom 10. */
        landmarks?: Place[];
        mapOverlays?: OverlayOptions;
        /** Travel mode, independent of the network chosen for display. */
        accessMode?: AccessMode;
        /** Route progress the elevation profile points at. */
        hoverProgress?: number | null;
        center?: Coordinate; zoom?: number;
        onEmptyClick?: (coordinate: Coordinate) => void;
        onPointSelect?: (id: string) => void;
        onPointHover?: (id: string | null) => void;
        onPointPreview?: (id: string, coordinate: Coordinate) => void;
        onPointMove?: (id: string, coordinate: Coordinate) => void;
        onDayEndDrag?: (night: number, progress: number) => void;
        onLegClick?: (legEndId: string, coordinate: Coordinate) => void;
        onInsert?: (legEndId: string, coordinate: Coordinate) => void;
        onDrawn?: (legEndId: string, coordinates: Coordinate[]) => void;
        onPlaceClick?: (place: Place) => void;
        onBounds?: (bounds: [number, number, number, number], preserveSearch: boolean) => void;
        onVisibleRange?: (range: [number, number]) => void; popup?: Snippet;
    } = $props();

    type LineHit = { legEndId: string; coordinate: Coordinate };
    const lineReach = 10;
    const sketchStep = 3;
    // The right inset keeps the route and callouts clear of the map controls.
    const controlsWidth = 68;
    const fitPadding = { top: 60, right: 90, bottom: 40, left: 40 };

    let container: HTMLDivElement;
    let popupContent: HTMLDivElement;
    let map = $state.raw<maplibregl.Map>();
    let ready = $state(false);
    let failure = $state("");
    let errorDetail = $state("");
    let markerList: maplibregl.Marker[] = [];
    let pinButtons = $state.raw(new Map<string, HTMLButtonElement>());
    let draggingPin = $state(false);
    let builtPins = "";
    let calloutPopup: maplibregl.Popup | undefined;
    let dem: InstanceType<typeof mlcontour.DemSource>;
    let appliedTheme: "light" | "dark";
    let fittedInitialRoute = false;
    let dragging = $state(false);
    let hover = $state<LineHit | null>(null);
    let overPoi = $state(false);
    let overOverlay = $state(false);
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

    export function centerOn(coordinate: Coordinate) {
        wholeRoute = false;
        map?.panTo(coordinate, { duration: motionDuration() }, { preserveSearch: true });
    }

    function inspectOverlay(event: maplibregl.MapMouseEvent) {
        const selected = overlayLayer?.hit(event);
        if (!selected || drawing || pickMode) return;
        event.preventDefault();
        cancelGesture();
        consumedPress = true;
        overlaySelection = selected;
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

    export function fitCoordinates(region: Coordinate[]) {
        if (!map || !region.length) return;
        wholeRoute = false;
        fitBounds(region, 13, motionDuration());
    }

    export function fitSearchResults(region: Coordinate[]) {
        if (!map || !region.length) return;
        fittedInitialRoute = true;
        wholeRoute = false;
        fitBounds(region, 14, motionDuration(), true);
    }

    export function zoomBy(delta: number) {
        if (!map) return;
        wholeRoute = false;
        map.zoomTo(map.getZoom() + delta, { duration: motionDuration() });
    }

    function fitBounds(region: Coordinate[], maxZoom: number, duration: number, preserveSearch = false) {
        const bounds = new maplibregl.LngLatBounds();
        region.forEach((coordinate) => bounds.extend(coordinate));
        map!.fitBounds(bounds, { padding: fitPadding, maxZoom, duration }, { preserveSearch });
    }

    function motionDuration() {
        return window.matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 400;
    }

    /** Pans the least distance that brings the open callout inside the map, clear of the controls column and the scale strip. */
    function keepCalloutInside() {
        if (!map || !calloutPopup) return;
        const frame = container.getBoundingClientRect();
        const box = calloutPopup.getElement().getBoundingClientRect();
        const margin = 16;
        const right = frame.right - controlsWidth;
        const bottom = frame.bottom - 48;
        const dx = box.left < frame.left + margin ? box.left - frame.left - margin : Math.max(0, box.right - right);
        const dy = box.top < frame.top + margin ? box.top - frame.top - margin : Math.max(0, box.bottom - bottom);
        if (dx || dy) map.panBy([dx, dy], { duration: motionDuration() }, { preserveSearch: true });
    }

    function lineFeature(input: Coordinate[], properties: Record<string, string> = {}): Feature<LineString> {
        // Svelte may proxy nested point arrays; worker messages require plain coordinate values.
        const plain = input.map(([longitude, latitude]) => [longitude, latitude]);
        return { type: "Feature", properties, geometry: { type: "LineString", coordinates: plain } };
    }

    function lineData(lines: Feature<LineString>[]): FeatureCollection<LineString> {
        return { type: "FeatureCollection", features: lines };
    }

    function tripData() {
        return lineData(segments.map(({ coordinates, color, legEndId, leg }) => lineFeature(coordinates, { color, legEndId, leg })));
    }

    function placeData(list: Place[]): FeatureCollection<Point> {
        return {
            type: "FeatureCollection",
            features: list.map((place) => ({
                type: "Feature", properties: { pid: place.id, category: place.category, name: place.label },
                geometry: { type: "Point", coordinates: [place.coordinate[0], place.coordinate[1]] },
            })),
        };
    }

    function highlightData() {
        return lineData(highlightedCoordinates.length < 2 ? [] : [lineFeature(highlightedCoordinates)]);
    }

    function setSketch(line: Coordinate[]) {
        (map?.getSource("planner-sketch") as GeoJSONSource | undefined)?.setData(lineData(line.length < 2 ? [] : [lineFeature(line)]));
    }

    function fitInitialRoute() {
        if (fittedInitialRoute || !map || coordinates.length < 2) return;
        fittedInitialRoute = true;
        fitRoute();
    }

    function installRoute() {
        if (!map || map.getSource("trip")) return;
        const dark = theme === "dark";
        const casing = dark ? "#201f17" : "#ffffff";
        const round = { "line-cap": "round", "line-join": "round" } as const;
        map.addSource("trip-highlight", { type: "geojson", data: highlightData() });
        map.addLayer({ id: "trip-highlight", type: "line", source: "trip-highlight", layout: round, paint: { "line-color": dark ? "#5a4622" : "#fbe6b8", "line-width": 14 } });
        map.addSource("trip", { type: "geojson", data: tripData() });
        map.addLayer({ id: "trip-casing", type: "line", source: "trip", filter: ["==", ["get", "leg"], "routed"], layout: round, paint: { "line-color": casing, "line-width": 8 } });
        map.addLayer({ id: "trip-casing-drawn", type: "line", source: "trip", filter: ["==", ["get", "leg"], "drawn"], layout: { "line-join": "round" }, paint: { "line-color": dark ? "#bdb47e" : "#5c5a2e", "line-width": 8, "line-dasharray": [1, 0.8] } });
        map.addLayer({ id: "trip-line", type: "line", source: "trip", layout: round, paint: { "line-color": ["get", "color"], "line-width": 4 } });
        map.addSource("planner-sketch", { type: "geojson", data: lineData([]) });
        map.addLayer({ id: "planner-sketch", type: "line", source: "planner-sketch", layout: round, paint: { "line-color": dark ? "#f175c5" : "#cc2a93", "line-width": 3, "line-dasharray": [1.5, 1.5] } });
        const panel = dark ? "#201f17" : "#ffffff";
        const text: maplibregl.SymbolLayerSpecification["layout"] = { "text-font": ["Noto Sans Regular"], "text-size": 11, "text-anchor": "top", "text-offset": [0, 1], "text-optional": true };
        const textPaint = { "text-color": dark ? "#f2efe3" : "#1c1b14", "text-halo-color": panel, "text-halo-width": 1.2 };
        map.addSource("planner-landmarks", { type: "geojson", data: placeData(landmarks) });
        map.addLayer({
            id: "planner-landmarks", type: "symbol", source: "planner-landmarks", minzoom: 10,
            layout: { "icon-image": `landmark-${theme}`, "icon-allow-overlap": true, "text-field": ["step", ["zoom"], "", 12, ["get", "name"]], ...text },
            paint: textPaint,
        });
        map.addSource("planner-highlights", { type: "geojson", data: placeData(highlightedPlaces) });
        map.addLayer({ id: "planner-highlight-rings", type: "circle", source: "planner-highlights", paint: { "circle-radius": 12, "circle-color": panel, "circle-stroke-color": dark ? "#f2a93a" : "#f4a81d", "circle-stroke-width": 2.5 } });
        map.addLayer({
            id: "planner-highlight-icons", type: "symbol", source: "planner-highlights",
            layout: { "icon-image": ["concat", "poi-", ["get", "category"], `-${theme}`], "icon-allow-overlap": true, "text-field": ["step", ["zoom"], "", 13, ["get", "name"]], ...text },
            paint: textPaint,
        });
        syncRouteVisibility();
    }

    function syncRouteVisibility() {
        if (!map?.getLayer("trip-line")) return;
        for (const id of ["trip-line", "trip-casing", "trip-casing-drawn", "trip-highlight"]) map.setLayoutProperty(id, "visibility", showRoute ? "visible" : "none");
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

    /** The nearest point on a leg within reach of a screen position. */
    function lineHit(point: maplibregl.Point): LineHit | null {
        if (!map || !showRoute) return null;
        let best: { legEndId: string; x: number; y: number; pixels: number } | null = null;
        for (const segment of segments) {
            const projected = segment.coordinates.map((coordinate) => map!.project(coordinate));
            for (let i = 1; i < projected.length; i++) {
                const a = projected[i - 1];
                const b = projected[i];
                const dx = b.x - a.x;
                const dy = b.y - a.y;
                const t = Math.max(0, Math.min(1, ((point.x - a.x) * dx + (point.y - a.y) * dy) / (dx * dx + dy * dy || 1)));
                const pixels = Math.hypot(point.x - a.x - dx * t, point.y - a.y - dy * t);
                if (!best || pixels < best.pixels) best = { legEndId: segment.legEndId, x: a.x + dx * t, y: a.y + dy * t, pixels };
            }
        }
        if (!best || best.pixels > lineReach) return null;
        const at = map.unproject([best.x, best.y]);
        return { legEndId: best.legEndId, coordinate: [at.lng, at.lat] };
    }

    const placeLayers = ["planner-pois", "planner-poi-icons", "planner-highlight-rings", "planner-highlight-icons", "planner-landmarks"];

    /** The basemap place, highlighted place or landmark under a screen position. */
    function placeAt(point: maplibregl.Point): Place | null {
        if (!map) return null;
        const box: [maplibregl.PointLike, maplibregl.PointLike] = [[point.x - 4, point.y - 4], [point.x + 4, point.y + 4]];
        const feature = map.queryRenderedFeatures(box, { layers: placeLayers.filter((id) => map!.getLayer(id)) })[0];
        if (!feature || feature.geometry.type !== "Point") return null;
        const { pid, kind } = feature.properties;
        if (pid) return [...highlightedPlaces, ...landmarks].find((place) => place.id === pid) ?? null;
        const [longitude, latitude] = feature.geometry.coordinates;
        return poiPlace(feature.id, String(kind), feature.properties["name:en"] ?? feature.properties.name, [longitude, latitude]);
    }

    function legEnds(legEndId: string): [Coordinate, Coordinate] {
        const legSegments = segments.filter((segment) => segment.legEndId === legEndId);
        return [legSegments[0].coordinates[0], legSegments.at(-1)!.coordinates.at(-1)!];
    }

    function pressMap(event: maplibregl.MapMouseEvent) {
        consumedPress = false;
        if (event.originalEvent.button !== 0) return;
        const coordinate: Coordinate = [event.lngLat.lng, event.lngLat.lat];
        if (drawing) {
            sketch = [coordinate];
            sketchEnd = event.point;
        } else if (hover) {
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
            overPoi = onMap && !!placeAt(event.point);
            hover = onMap && !overPoi && !dragging && !draggingPin && !drawing && !pickMode ? lineHit(event.point) : null;
            overOverlay = overlayLayer?.hover(onMap && !overPoi && !hover && !dragging && !draggingPin && !drawing && !pickMode ? event : undefined) ?? false;
        }
    }

    function releaseMap(event: maplibregl.MapMouseEvent) {
        if (event.originalEvent.target !== map?.getCanvas()) { cancelGesture(); return; }
        const coordinate: Coordinate = [event.lngLat.lng, event.lngLat.lat];
        if (sketch && drawing) {
            const drawn = sketch;
            sketch = null;
            consumedPress = true;
            setSketch([]);
            if (drawn.length > 1) onDrawn?.(drawing, drawn);
        } else if (press) {
            // A press without a drag stays a click, which opens the leg callout.
            const { hit, moved } = press;
            press = null;
            if (!moved) return;
            consumedPress = true;
            hover = null;
            setSketch([]);
            onInsert?.(hit.legEndId, coordinate);
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
        maplibregl.setWorkerUrl(mapWorkerUrl);
        const protocol = new Protocol();
        maplibregl.addProtocol("pmtiles", protocol.tile);
        const terrainLease = terrain.acquire(maplibregl);
        dem = terrainLease.dem;
        const contourUrl = dem.contourProtocolUrl({ thresholds: { 10: [200, 1000], 11: [100, 500], 13: [50, 250], 14: [20, 100] }, contourLayer: "contours", elevationKey: "ele", levelKey: "level" });
        insertDot = new maplibregl.Marker({ element: Object.assign(document.createElement("div"), { className: "planner-insert-dot" }) });
        hoverDot = new maplibregl.Marker({ element: Object.assign(document.createElement("div"), { className: "planner-hover-dot" }) });
        try {
            map = new maplibregl.Map({ container, center, zoom, maxBounds: MAP_BOUNDS, style: mapStyle(theme, dem.sharedDemProtocolUrl, contourUrl), attributionControl: false, maxPitch: 0, renderWorldCopies: false });
            overlayLayer = new RouteOverlays(map, (message, retry = false) => { overlayStatus = message; overlayRetry = retry; });
            fitInitialRoute();
            map.addControl(new maplibregl.AttributionControl({ compact: true }), "bottom-right");
            map.addControl(new maplibregl.ScaleControl({ maxWidth: 90, unit: "metric" }), "bottom-left");
            map.dragRotate.disable();
            map.touchZoomRotate.disableRotation();
            map.on("style.load", () => { ready = true; installRoute(); syncTerrainAndOverlays(); });
            map.once("load", () => { basemapComplete = true; syncTerrainAndOverlays(); });
            map.setMissingStyleImageResolver((id) => {
                const icon = mapIcon(id);
                if (icon && !map!.hasImage(id)) map!.addImage(id, icon.image, { pixelRatio: icon.pixelRatio });
            });
            map.once("load", reportView);
            const terrainError = terrainRetry(map);
            map.on("error", (event) => {
                if (terrainError(event)) {
                    console.warn("Planner terrain:", event.error);
                    return;
                }
                failure ="Some map data could not load. Check your connection, then retry.";
                errorDetail = event.error.message;
                console.error("Planner map:", event.error);
                // MapLibre does not repaint after a failed request, so the first load would wait for a camera move.
                // A failed tile also marks its source loaded, so an earlier repaint would fire load too soon.
                if (!basemapComplete && map!.areTilesLoaded()) map!.triggerRepaint();
            });
            map.on("click", (event) => {
                const target = event.originalEvent.target;
                if (consumedPress || drawing || (target instanceof Node && popupContent?.contains(target))) return;
                const place = placeAt(event.point);
                const hit = place || pickMode ? null : lineHit(event.point);
                const overlay = place || pickMode ? null : overlayLayer?.hit(event);
                overlaySelection = null;
                if (place) onPlaceClick?.(place);
                else if (overlay?.kind === 'access') overlaySelection = overlay;
                else if (hit) onLegClick?.(hit.legEndId, hit.coordinate);
                else onEmptyClick?.([event.lngLat.lng, event.lngLat.lat]);
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
            map.on("mouseout", () => { if (!press) hover = null; overPoi = false; overOverlay = false; overlayLayer?.hover(); });
            map.on("movestart", (event) => { if (event.originalEvent) wholeRoute = false; overOverlay = false; overlayLayer?.hover(); });
            map.on("moveend", reportView);
        } catch (error) {
            failure = "The map could not start. This view needs a browser with WebGL enabled.";
            errorDetail = error instanceof Error ? error.message : String(error);
        }
        map?.on("dragstart",()=>dragging=true);
        map?.on("dragend",()=>dragging=false);
        let refit: ReturnType<typeof setTimeout> | undefined;
        const observer = new ResizeObserver(() => {
            map?.resize();
            if (!wholeRoute) return;
            clearTimeout(refit);
            refit = setTimeout(() => { if (wholeRoute) fitRoute(); }, 150);
        });
        observer.observe(container);
        const popupObserver = new ResizeObserver(() => { requestAnimationFrame(keepCalloutInside); });
        popupObserver.observe(popupContent);
        return () => {
            popupObserver.disconnect();
            observer.disconnect();
            clearTimeout(refit);
            markerList.forEach((marker) => marker.remove());
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
        map.setStyle(mapStyle(theme, dem.sharedDemProtocolUrl, dem.contourProtocolUrl({ thresholds: { 10: [200, 1000], 11: [100, 500], 13: [50, 250], 14: [20, 100] }, contourLayer: "contours", elevationKey: "ele", levelKey: "level" })));
    });
    $effect(() => {
        coordinates;
        if (map && ready) {
            fitInitialRoute();
            (map.getSource("trip") as GeoJSONSource | undefined)?.setData(tripData());
            reportView();
        }
    });
    $effect(() => { hillshade; contours; if (ready) syncTerrain(); });
    $effect(() => { const options = { ...mapOverlays }; const mode = accessMode; if (ready) { overlaySelection = null; overOverlay = false; overlayLayer?.set(options, mode); } });
    $effect(() => { showRoute; if (ready) syncRouteVisibility(); });
    $effect(() => {
        highlightedCoordinates;
        if (map && ready) (map.getSource("trip-highlight") as GeoJSONSource | undefined)?.setData(highlightData());
    });
    $effect(() => {
        const filter = poiFilter(shownCategories);
        if (!map || !ready) return;
        for (const id of ["planner-pois", "planner-poi-icons"]) if (map.getLayer(id)) map.setFilter(id, filter);
    });
    $effect(() => {
        const data = placeData(highlightedPlaces);
        if (map && ready) (map.getSource("planner-highlights") as GeoJSONSource | undefined)?.setData(data);
    });
    $effect(() => {
        const data = placeData(landmarks);
        if (map && ready) (map.getSource("planner-landmarks") as GeoJSONSource | undefined)?.setData(data);
    });
    $effect(() => {
        if (pickMode) hover = null;
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
        if (hover) insertDot.setLngLat(hover.coordinate).addTo(map);
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
        if (map) map.getCanvas().style.cursor = dragging || draggingPin ? "grabbing" : drawing || pickMode ? "crosshair" : hover || overPoi || overOverlay ? "pointer" : "grab";
    });

    function markerIcon(path: string) {
        const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
        svg.setAttribute("viewBox", "0 0 24 24");
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

    function routeProgress(marker: maplibregl.Marker) {
        const at = marker.getLngLat();
        return nearestProgress(coordinates, [at.lng, at.lat]);
    }

    // Pins are rebuilt only when the points change, so a focused pin keeps its focus through a selection.
    $effect(() => {
        if (!map) return;
        if (draggingPin) return;
        const shown = points.filter((point) => showRoute || point.kind === "place" || point.kind === "marker");
        const key = JSON.stringify(shown);
        if (key === builtPins) return;
        builtPins = key;
        markerList.forEach((marker) => marker.remove());
        const buttons = new Map<string, HTMLButtonElement>();
        let nightNumber = 0;
        markerList = shown.map((point) => {
            if (point.kind === "night") nightNumber++;
            const dayEnd = point.kind === "dayend";
            const draggable = dayEnd ? !!onDayEndDrag : !!onPointMove && !point.fixed && point.kind !== "place";
            const button = document.createElement("button");
            button.className = `planner-map-pin ${point.kind} ${point.appearance ?? ""}${draggable ? " draggable" : ""}`;
            if (point.color) button.style.setProperty("--pin-color", point.color);
            button.setAttribute("aria-label", point.label);
            button.title = dayEnd ? `${point.appearance === "moved" ? "Day end you moved" : "Day end suggested"} · drag along the route` : point.label + (draggable ? " · drag to move" : "");
            buttons.set(point.id, button);
            if (point.kind === "place" && point.category) {
                button.append(markerIcon(placeCategories[point.category].icon));
            } else if (point.kind === "waypoint" || point.kind === "detour") {
                button.append(markerIcon(pointIcons[point.kind]));
            } else {
                button.textContent = point.markerLabel ?? (point.kind === "start" ? "A" : point.kind === "finish" ? "B" : point.kind === "night" ? String(nightNumber) : "");
            }
            button.addEventListener("click", (event) => { event.stopPropagation(); onPointSelect?.(point.id); });
            button.addEventListener('mouseenter', () => onPointHover?.(point.id));
            button.addEventListener('mouseleave', () => onPointHover?.(null));
            button.addEventListener('focus', () => onPointHover?.(point.id));
            button.addEventListener('blur', () => onPointHover?.(null));
            const marker = new maplibregl.Marker({ element: button, draggable }).setLngLat(point.coordinate).addTo(map!);
            if (dayEnd) {
                marker.on("drag", () => marker.setLngLat(coordinateAt(coordinates, routeProgress(marker))));
                marker.on("dragend", () => onDayEndDrag?.(point.night!, routeProgress(marker)));
            } else {
                marker.on("dragstart", () => { draggingPin = true; hover = null; });
                marker.on("drag", () => { const p = marker.getLngLat(); onPointPreview?.(point.id, [p.lng, p.lat]); });
                marker.on("dragend", () => { const p = marker.getLngLat(); onPointMove?.(point.id, [p.lng, p.lat]); draggingPin = false; });
            }
            return marker;
        });
        pinButtons = buttons;
    });
    $effect(() => {
        for (const [id, button] of pinButtons) {
            button.classList.toggle("selected", id === selectedId);
            button.classList.toggle('highlighted', id === hoveredId);
            button.classList.toggle("matched", highlightedPlaceIds.includes(id));
            button.setAttribute("aria-pressed", String(id === selectedId));
        }
    });
    $effect(() => {
        if (!map || !popupContent) return;
        if (!callout || !popup) {
            calloutPopup?.remove();
            calloutPopup = undefined;
            return;
        }
        calloutPopup ??= new maplibregl.Popup({
            closeButton: false, closeOnClick: false, offset: 20, maxWidth: "340px",
            padding: { top: 16, right: controlsWidth, bottom: 48, left: 16 },
        }).setDOMContent(popupContent);
        calloutPopup.setLngLat(callout).addTo(map);
        const settle = () => requestAnimationFrame(keepCalloutInside);
        if (map.isMoving()) map.once("moveend", settle);
        else settle();
    });
</script>

<svelte:window onmouseup={(event) => { if (event.target !== map?.getCanvas()) cancelGesture(); }}
    onblur={cancelGesture} onkeydown={(event) => { if (event.key === 'Escape') { cancelGesture(); overlaySelection = null; } }} />

<div class="map-frame" data-map-theme={theme}>
    <div class="map-canvas" bind:this={container} aria-label="Route map"></div>
    <div class="popup-storage"><div bind:this={popupContent}>{#if popup}{@render popup()}{/if}</div></div>
    {#if overlaySelection}
        <MapOverlayDetails selection={overlaySelection} onclose={() => overlaySelection = null}
            onuse={() => { const coordinate = overlaySelection!.coordinate; overlaySelection = null; onEmptyClick?.(coordinate); }} />
    {/if}
    {#if overlayStatus && !failure && !overlaySelection}
        <div class="overlay-status" role="status">{overlayStatus}{#if overlayRetry}<button onclick={() => overlayLayer?.refresh()}>Retry</button>{/if}</div>
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
    .map-canvas { width: 100%; height: 100%; min-height: 240px; }
    .popup-storage { display: none; }
    .overlay-status { position: absolute; left: 12px; bottom: 34px; max-width: calc(100% - 80px); padding: 7px 10px; border-radius: 6px; color: var(--ink); background: var(--panel); font-size: 12px; }
    .overlay-status button { margin-left: 8px; border: 0; background: none; color: var(--link); font: inherit; text-decoration: underline; cursor: pointer; }
    .map-status { position: absolute; bottom: 32px; left: 12px; right: 12px; padding: 10px 12px; background: var(--panel, white); color: var(--ink, #1c1b14); border: 1px solid var(--line-strong, #bcb9aa); font-size: 13px; }
    .map-status button { color: inherit; background: transparent; border: 0; text-decoration: underline; cursor: pointer; font: inherit; }
    :global(.planner-map-pin) { width: 28px; height: 28px; display: grid; place-items: center; padding: 0; border: 2px solid var(--panel, #fff); border-radius: 50%; background: #a4501e; color: #fff; font: 700 13px var(--sans, sans-serif); cursor: pointer; box-shadow: var(--planner-shadow, 0 6px 18px rgba(28, 27, 20, .12)); }
    :global(.planner-map-pin.draggable) { cursor: grab; }
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
    :global(.planner-map-pin.matched) { border: 3px solid var(--amber, #f4a81d); }
    :global(.planner-map-pin.selected) { outline: 3px solid var(--amber, #f4a81d); outline-offset: 3px; }
    :global(.planner-map-pin.highlighted) { outline: 3px solid var(--ink, #1c1b14); outline-offset: 3px; }
    :global(.planner-map-pin:hover) { filter: brightness(1.08); }
    :global(.planner-map-pin:focus-visible) { outline: 3px solid var(--amber, #f4a81d); outline-offset: 3px; }
    :global(.planner-hover-dot) { width: 12px; height: 12px; border: 3px solid var(--panel, #fff); border-radius: 50%; background: var(--ink, #1c1b14); pointer-events: none; }
    :global(.planner-insert-dot) { width: 14px; height: 14px; border: 2.5px solid var(--route, #cc2a93); border-radius: 50%; background: var(--panel, #fff); pointer-events: none; }
    .map-frame :global(.maplibregl-popup-content) { padding: 0; border-radius: 8px; color: var(--ink, #1c1b14); background: var(--panel, white); font-family: var(--sans, sans-serif); box-shadow: var(--planner-shadow, 0 6px 18px rgba(28, 27, 20, .12)); }
    .map-frame :global(.maplibregl-ctrl-scale) { padding: 0 6px; border: 1px solid var(--line-strong, #bcb9aa); border-top: 0; background: var(--panel, white); font: 11px/18px var(--sans, sans-serif); color: var(--ink, #1c1b14); }
    .map-frame :global(.maplibregl-ctrl-attrib) { background: var(--panel, white); font: 11px var(--sans, sans-serif); color: var(--ink-soft, #5c5a2e); }
    .map-frame :global(.maplibregl-ctrl-attrib a) { color: var(--link, var(--ink-soft, #5c5a2e)); }
    .map-frame :global(.maplibregl-ctrl-attrib.maplibregl-compact-show .maplibregl-ctrl-attrib-button) { background-color: var(--parchment-2, #e7ecdf); }
    .map-frame[data-map-theme="dark"] :global(.maplibregl-ctrl-attrib-button) { background-image: url("data:image/svg+xml;charset=utf-8,%3Csvg xmlns='http://www.w3.org/2000/svg' width='24' height='24' fill-rule='evenodd' viewBox='0 0 20 20'%3E%3Cpath d='M4 10a6 6 0 1 0 12 0 6 6 0 1 0-12 0m5-3a1 1 0 1 0 2 0 1 1 0 1 0-2 0m0 3a1 1 0 1 1 2 0v3a1 1 0 1 1-2 0' fill='%23f2efe3'/%3E%3C/svg%3E"); }
    .map-frame :global(.maplibregl-popup-anchor-bottom .maplibregl-popup-tip) { border-top-color: var(--panel, white); }
    .map-frame :global(.maplibregl-popup-anchor-top .maplibregl-popup-tip) { border-bottom-color: var(--panel, white); }
</style>
