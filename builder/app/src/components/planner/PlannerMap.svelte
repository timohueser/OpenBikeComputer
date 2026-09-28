<script lang="ts">
    import { onMount, type Snippet } from "svelte";
    import * as maplibregl from "maplibre-gl";
    import mapWorkerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url";
    import type { GeoJSONSource } from "maplibre-gl";
    import type { Feature, FeatureCollection, LineString } from "geojson";
    import { Protocol } from "pmtiles";
    import mlcontour from "maplibre-contour";
    import "maplibre-gl/dist/maplibre-gl.css";
    import { mapStyle } from "../../lib/planner/map-style";
    import { TERRAIN_URL } from "../../lib/planner/map-data";
    import { poiKinds } from "../../lib/planner/poi-kinds";
    import { coordinateAt, nearestProgress } from "../../lib/planner/editor";
    import type { Coordinate, MapPoi, MapPoint, MapSegment } from "../../lib/planner/map-types";

    let {
        segments = [], coordinates = [], highlightedCoordinates = [], points = [], selectedId = null, callout = null,
        drawing = null, highlightedPlaceIds = [], theme = "light", hillshade = true, contours = true, pickMode = false,
        showRoute = true, center = [8.0, 46.7], zoom = 11,
        onEmptyClick, onPointSelect, onPointMove, onDayEndDrag, onLegClick, onInsert, onDrawn, onPoiClick, onVisibleRange, popup,
    }: {
        segments?: MapSegment[]; coordinates?: Coordinate[]; highlightedCoordinates?: Coordinate[]; points?: MapPoint[];
        selectedId?: string | null; callout?: Coordinate | null; drawing?: string | null; highlightedPlaceIds?: string[];
        pickMode?: boolean; showRoute?: boolean; theme?: "light" | "dark"; hillshade?: boolean; contours?: boolean;
        center?: Coordinate; zoom?: number;
        onEmptyClick?: (coordinate: Coordinate) => void;
        onPointSelect?: (id: string) => void;
        onPointMove?: (id: string, coordinate: Coordinate) => void;
        onDayEndDrag?: (night: number, progress: number) => void;
        onLegClick?: (legEndId: string, coordinate: Coordinate) => void;
        onInsert?: (legEndId: string, coordinate: Coordinate) => void;
        onDrawn?: (legEndId: string, coordinates: Coordinate[]) => void;
        onPoiClick?: (poi: MapPoi) => void;
        onVisibleRange?: (range: [number, number]) => void; popup?: Snippet;
    } = $props();

    type LineHit = { legEndId: string; coordinate: Coordinate };
    const lineReach = 10;
    const sketchStep = 3;
    // The right inset keeps the route clear of the map controls.
    const fitPadding = { top: 60, right: 90, bottom: 40, left: 40 };

    let container: HTMLDivElement;
    let popupContent: HTMLDivElement;
    let map = $state.raw<maplibregl.Map>();
    let ready = $state(false);
    let failure = $state("");
    let errorDetail = $state("");
    let markerList: maplibregl.Marker[] = [];
    let calloutPopup: maplibregl.Popup | undefined;
    let dem: InstanceType<typeof mlcontour.DemSource>;
    let appliedTheme: "light" | "dark";
    let fittedInitialRoute = false;
    let dragging = $state(false);
    let hover = $state<LineHit | null>(null);
    let overPoi = $state(false);
    let insertDot: maplibregl.Marker;
    let press: { hit: LineHit; start: maplibregl.Point; moved: boolean } | null = null;
    let sketch: Coordinate[] | null = null;
    let sketchEnd: maplibregl.Point;
    // A finished drawing or insert must not also count as a click on the map.
    let consumedPress = false;
    let wholeRoute = false;

    export function fitRoute() {
        if (!map || !coordinates.length) return;
        fitBounds(coordinates, 14, 0);
        wholeRoute = true;
    }

    export function showPlace(coordinate: Coordinate, detail: number) {
        if (!map) return;
        wholeRoute = false;
        // The callout opens above the place, so the place sits below the centre and left of the map controls.
        const below = Math.min(150, map.getContainer().clientHeight / 5);
        map.flyTo({ center: coordinate, zoom: detail, offset: [-25, below], duration: motionDuration() });
    }

    export function fitCoordinates(region: Coordinate[]) {
        if (!map || !region.length) return;
        wholeRoute = false;
        fitBounds(region, 13, motionDuration());
    }

    export function zoomBy(delta: number) {
        if (!map) return;
        wholeRoute = false;
        map.zoomTo(map.getZoom() + delta, { duration: motionDuration() });
    }

    function fitBounds(region: Coordinate[], maxZoom: number, duration: number) {
        const bounds = new maplibregl.LngLatBounds();
        region.forEach((coordinate) => bounds.extend(coordinate));
        map!.fitBounds(bounds, { padding: fitPadding, maxZoom, duration });
    }

    function motionDuration() {
        return window.matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 400;
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
        map.addLayer({ id: "trip-highlight", type: "line", source: "trip-highlight", layout: round, paint: { "line-color": dark ? "#4a3a1c" : "#fbe6b8", "line-width": 14 } });
        map.addSource("trip", { type: "geojson", data: tripData() });
        map.addLayer({ id: "trip-casing", type: "line", source: "trip", filter: ["==", ["get", "leg"], "routed"], layout: round, paint: { "line-color": casing, "line-width": 8 } });
        map.addLayer({ id: "trip-casing-drawn", type: "line", source: "trip", filter: ["==", ["get", "leg"], "drawn"], layout: { "line-join": "round" }, paint: { "line-color": dark ? "#bdb47e" : "#5c5a2e", "line-width": 8, "line-dasharray": [1, 0.8] } });
        map.addLayer({ id: "trip-line", type: "line", source: "trip", layout: round, paint: { "line-color": ["get", "color"], "line-width": 4 } });
        map.addSource("planner-sketch", { type: "geojson", data: lineData([]) });
        map.addLayer({ id: "planner-sketch", type: "line", source: "planner-sketch", layout: round, paint: { "line-color": dark ? "#f175c5" : "#cc2a93", "line-width": 3, "line-dasharray": [1.5, 1.5] } });
        syncRouteVisibility();
        syncTerrain();
    }

    function syncRouteVisibility() {
        if (!map?.getLayer("trip-line")) return;
        for (const id of ["trip-line", "trip-casing", "trip-casing-drawn", "trip-highlight"]) map.setLayoutProperty(id, "visibility", showRoute ? "visible" : "none");
    }

    function syncTerrain() {
        if (!map?.getLayer("relief")) return;
        for (const id of ["contour-lines", "contour-labels"]) map.setLayoutProperty(id, "visibility", contours ? "visible" : "none");
        map.setLayoutProperty("relief", "visibility", hillshade ? "visible" : "none");
    }

    function reportView() {
        if (!map || !onVisibleRange || !coordinates.length) return;
        const bounds = map.getBounds();
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

    function poiAt(point: maplibregl.Point): MapPoi | null {
        if (!map?.getLayer("planner-pois")) return null;
        const box: [maplibregl.PointLike, maplibregl.PointLike] = [[point.x - 4, point.y - 4], [point.x + 4, point.y + 4]];
        const feature = map.queryRenderedFeatures(box, { layers: ["planner-pois", "planner-poi-labels"] })[0];
        if (!feature || feature.geometry.type !== "Point") return null;
        const kind = String(feature.properties.kind);
        const [longitude, latitude] = feature.geometry.coordinates;
        const label = feature.properties["name:en"] ?? feature.properties.name ?? poiKinds[kind]?.label ?? kind;
        return { id: String(feature.id), kind, label: String(label), coordinate: [longitude, latitude] };
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
            overPoi = onMap && !!poiAt(event.point);
            hover = onMap && !overPoi && !dragging && !drawing ? lineHit(event.point) : null;
        }
    }

    function releaseMap(event: maplibregl.MapMouseEvent) {
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

    onMount(() => {
        appliedTheme = theme;
        maplibregl.setWorkerUrl(mapWorkerUrl);
        const protocol = new Protocol();
        maplibregl.addProtocol("pmtiles", protocol.tile);
        dem = new mlcontour.DemSource({ url: TERRAIN_URL, maxzoom: 12, worker: true, cacheSize: 64, encoding: "terrarium" });
        dem.setupMaplibre(maplibregl);
        const contourUrl = dem.contourProtocolUrl({ thresholds: { 10: [200, 1000], 11: [100, 500], 13: [50, 250], 14: [20, 100] }, contourLayer: "contours", elevationKey: "ele", levelKey: "level" });
        insertDot = new maplibregl.Marker({ element: Object.assign(document.createElement("div"), { className: "planner-insert-dot" }) });
        try {
            map = new maplibregl.Map({ container, center, zoom, style: mapStyle(theme, dem.sharedDemProtocolUrl, contourUrl), attributionControl: false, maxPitch: 0, renderWorldCopies: false });
            fitInitialRoute();
            map.addControl(new maplibregl.AttributionControl({ compact: true }), "bottom-right");
            map.addControl(new maplibregl.ScaleControl({ maxWidth: 90, unit: "metric" }), "bottom-left");
            map.dragRotate.disable();
            map.touchZoomRotate.disableRotation();
            map.on("style.load", () => { ready = true; installRoute(); });
            map.once("load", reportView);
            map.on("error", (event) => {
                failure = "Some map data could not load. Check your connection, then retry.";
                errorDetail = event.error.message;
                console.error("Planner map:", event.error);
            });
            map.on("click", (event) => {
                const target = event.originalEvent.target;
                if (consumedPress || drawing || (target instanceof Node && popupContent?.contains(target))) return;
                const poi = poiAt(event.point);
                const hit = poi ? null : lineHit(event.point);
                if (poi) onPoiClick?.(poi);
                else if (hit) onLegClick?.(hit.legEndId, hit.coordinate);
                else onEmptyClick?.([event.lngLat.lng, event.lngLat.lat]);
            });
            map.on("mousedown", pressMap);
            map.on("mousemove", trackPointer);
            map.on("mouseup", releaseMap);
            map.on("mouseout", () => { if (!press) hover = null; overPoi = false; });
            map.on("movestart", (event) => { if (event.originalEvent) wholeRoute = false; });
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
            refit = setTimeout(fitRoute, 150);
        });
        observer.observe(container);
        return () => {
            observer.disconnect();
            clearTimeout(refit);
            markerList.forEach((marker) => marker.remove());
            insertDot.remove();
            calloutPopup?.remove();
            map?.remove();
            maplibregl.removeProtocol(dem.sharedDemProtocolId);
            maplibregl.removeProtocol(dem.contourProtocolId);
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
    $effect(() => { showRoute; if (ready) syncRouteVisibility(); });
    $effect(() => {
        highlightedCoordinates;
        if (map && ready) (map.getSource("trip-highlight") as GeoJSONSource | undefined)?.setData(highlightData());
    });
    $effect(() => {
        const ids = [...highlightedPlaceIds];
        if (map && ready && map.getLayer("planner-pois-matched")) map.setFilter("planner-pois-matched", ["in", ["to-string", ["id"]], ["literal", ids]]);
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
        if (map) map.getCanvas().style.cursor = dragging ? "grabbing" : drawing || pickMode ? "crosshair" : hover || overPoi ? "pointer" : "grab";
    });

    function markerIcon(appearance: "hotel" | "camp" | "waypoint" | "detour") {
        const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
        svg.setAttribute("viewBox", "0 0 24 24");
        svg.setAttribute("aria-hidden", "true");
        svg.setAttribute("fill", "none");
        svg.setAttribute("stroke", "currentColor");
        svg.setAttribute("stroke-width", "1.7");
        svg.setAttribute("stroke-linecap", "round");
        svg.setAttribute("stroke-linejoin", "round");
        const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
        const paths = {
            camp: "M3 20 12 4l9 16H3ZM8 20l4-8 4 8M10 2l4 4M14 2l-4 4",
            hotel: "M3 19V8m18 11V8M3 15h18M5 15V6h14v9M7 12V9h4v3m2 0V9h4v3",
            waypoint: "M6 21V3m0 1c4-3 8 3 12 0v9c-4 3-8-3-12 0",
            detour: "M8 5 3 10l5 5M3 10h11a5 5 0 0 1 0 10h-2",
        };
        path.setAttribute("d", paths[appearance]);
        svg.append(path);
        return svg;
    }

    function routeProgress(marker: maplibregl.Marker) {
        const at = marker.getLngLat();
        return nearestProgress(coordinates, [at.lng, at.lat]);
    }

    $effect(() => {
        if (!map) return;
        markerList.forEach((marker) => marker.remove());
        let nightNumber = 0;
        markerList = points.filter((point) => showRoute || point.kind === "place" || point.kind === "marker").map((point) => {
            if (point.kind === "night") nightNumber++;
            const dayEnd = point.kind === "dayend";
            const draggable = dayEnd ? !!onDayEndDrag : !!onPointMove && !point.fixed && point.kind !== "place";
            const button = document.createElement("button");
            button.className = `planner-map-pin ${point.kind} ${point.appearance ?? ""}${draggable ? " draggable" : ""}${point.id === selectedId ? " selected" : ""}`;
            if (point.color) button.style.setProperty("--pin-color", point.color);
            button.setAttribute("aria-label", point.label);
            button.setAttribute("aria-pressed", String(point.id === selectedId));
            button.title = point.label + (dayEnd ? " · drag along the route" : draggable ? " · drag to move" : "");
            if (point.kind === "place" && (point.appearance === "hotel" || point.appearance === "camp")) {
                button.append(markerIcon(point.appearance));
            } else if (point.kind === "waypoint" || point.kind === "detour") {
                button.append(markerIcon(point.kind));
            } else {
                button.textContent = point.markerLabel ?? (point.kind === "start" ? "A" : point.kind === "finish" ? "B" : point.kind === "night" ? String(nightNumber) : "");
            }
            button.addEventListener("click", (event) => { event.stopPropagation(); onPointSelect?.(point.id); });
            const marker = new maplibregl.Marker({ element: button, draggable }).setLngLat(point.coordinate).addTo(map!);
            if (dayEnd) {
                marker.on("drag", () => marker.setLngLat(coordinateAt(coordinates, routeProgress(marker))));
                marker.on("dragend", () => onDayEndDrag?.(point.night!, routeProgress(marker)));
            } else {
                marker.on("dragend", () => { const p = marker.getLngLat(); onPointMove?.(point.id, [p.lng, p.lat]); });
            }
            return marker;
        });
    });
    $effect(() => {
        if (!map || !popupContent) return;
        if (!callout || !popup) {
            calloutPopup?.remove();
            calloutPopup = undefined;
            return;
        }
        calloutPopup ??= new maplibregl.Popup({ closeButton: false, closeOnClick: false, offset: 20, maxWidth: "300px" }).setDOMContent(popupContent);
        calloutPopup.setLngLat(callout).addTo(map);
    });
</script>

<div class="map-frame" data-map-theme={theme}>
    <div class="map-canvas" bind:this={container} aria-label="Route map"></div>
    <div class="popup-storage"><div bind:this={popupContent}>{#if popup}{@render popup()}{/if}</div></div>
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
    .map-status { position: absolute; bottom: 32px; left: 12px; right: 12px; padding: 10px 12px; background: var(--panel, white); color: var(--ink, #1c1b14); border: 1px solid var(--line-strong, #bcb9aa); font-size: 13px; }
    .map-status button { color: inherit; background: transparent; border: 0; text-decoration: underline; cursor: pointer; font: inherit; }
    :global(.planner-map-pin) { width: 28px; height: 28px; display: grid; place-items: center; padding: 0; border: 2px solid var(--panel, #fff); border-radius: 50%; background: #a4501e; color: #fff; font: 700 12px var(--sans, sans-serif); cursor: pointer; box-shadow: 0 2px 5px #0003; }
    :global(.planner-map-pin.draggable) { cursor: grab; }
    :global(.planner-map-pin.via) { width: 16px; height: 16px; background: var(--route, #cc2a93); }
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
    :global(.planner-map-pin.dayend.draggable) { cursor: ew-resize; }
    :global(.planner-map-pin.selected) { outline: 3px solid var(--amber, #f4a81d); outline-offset: 3px; }
    :global(.planner-map-pin:hover) { filter: brightness(1.08); }
    :global(.planner-map-pin:focus-visible) { outline: 3px solid var(--amber, #f4a81d); outline-offset: 3px; }
    :global(.planner-insert-dot) { width: 14px; height: 14px; border: 2.5px solid var(--route, #cc2a93); border-radius: 50%; background: var(--panel, #fff); pointer-events: none; }
    .map-frame :global(.maplibregl-popup-content) { padding: 0; border-radius: 6px; color: var(--ink, #1c1b14); background: var(--panel, white); font-family: var(--sans, sans-serif); box-shadow: 0 5px 18px #0003; }
    .map-frame :global(.maplibregl-popup-anchor-bottom .maplibregl-popup-tip) { border-top-color: var(--panel, white); }
    .map-frame :global(.maplibregl-popup-anchor-top .maplibregl-popup-tip) { border-bottom-color: var(--panel, white); }
</style>
