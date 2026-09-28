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
    import type { Coordinate, MapPoint } from "../../lib/planner/map-types";

    let {
        segments = [], coordinates = [], highlightedCoordinates = [], points = [], selectedId = null, theme = "light", hillshade = true, contours = true, pickMode = false, showRoute = true,
        center = [8.0, 46.7], zoom = 11, onMapClick, onPointSelect, onPointMove, onVisibleRange, popup,
    }: {
        segments?: {coordinates:Coordinate[];color:string}[]; coordinates?: Coordinate[]; highlightedCoordinates?: Coordinate[]; points?: MapPoint[]; selectedId?: string | null; pickMode?: boolean; showRoute?: boolean;
        theme?: "light" | "dark"; hillshade?: boolean; contours?: boolean; center?: Coordinate; zoom?: number;
        onMapClick?: (coordinate: Coordinate) => void; onPointSelect?: (id: string) => void;
        onPointMove?: (id: string, coordinate: Coordinate) => void;
        onVisibleRange?: (range: [number, number]) => void; popup?: Snippet;
    } = $props();

    let container: HTMLDivElement;
    let popupContent: HTMLDivElement;
    let map = $state.raw<maplibregl.Map>();
    let ready = $state(false);
    let failure = $state("");
    let errorDetail = $state("");
    let markerList: maplibregl.Marker[] = [];
    let callout: maplibregl.Popup | undefined;
    let dem: InstanceType<typeof mlcontour.DemSource>;
    let appliedTheme: "light" | "dark";
    let fittedInitialRoute = false;

    export function fitRoute() {
        if (!map || !coordinates.length) return;
        const bounds = new maplibregl.LngLatBounds();
        coordinates.forEach((coordinate) => bounds.extend(coordinate));
        map.fitBounds(bounds, { padding: 65, maxZoom: 14, duration: 0 });
    }

    export function showPlace(coordinate: Coordinate, detail: number) {
        map?.jumpTo({ center: coordinate, zoom: detail });
    }

    export function fitCoordinates(region: Coordinate[]) {
        if (!map || !region.length) return;
        const bounds = new maplibregl.LngLatBounds();
        region.forEach((coordinate) => bounds.extend(coordinate));
        map.fitBounds(bounds, { padding: 65, maxZoom: 13, duration: motionDuration() });
    }

    export function zoomBy(delta: number) {
        if (map) map.zoomTo(map.getZoom() + delta, { duration: motionDuration() });
    }

    function motionDuration() {
        return window.matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 400;
    }

    function routeData(input: Coordinate[] = coordinates): Feature<LineString> {
        // Svelte may proxy nested point arrays; worker messages require plain coordinate values.
        const plain = input.map(([longitude, latitude]) => [longitude, latitude]);
        return { type: "Feature", properties: {}, geometry: { type: "LineString", coordinates: plain } };
    }

    function tripData(): FeatureCollection<LineString> {
        return { type: 'FeatureCollection', features: segments.length ? segments.map(segment => ({ ...routeData(segment.coordinates), properties: { color: segment.color } })) : [{ ...routeData(), properties: { color: theme === 'dark' ? '#f175c5' : '#cc2a93' } }] };
    }

    function highlightData(): FeatureCollection<LineString> {
        return { type: "FeatureCollection", features: highlightedCoordinates.length < 2 ? [] : [routeData(highlightedCoordinates)] };
    }

    function fitInitialRoute() {
        if (fittedInitialRoute || !map || coordinates.length < 2) return;
        fittedInitialRoute = true;
        fitRoute();
    }

    function installRoute() {
        if (!map || map.getSource("trip")) return;
        map.addSource("trip-highlight", { type: "geojson", data: highlightData() });
        map.addLayer({ id: "trip-highlight", type: "line", source: "trip-highlight", layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": theme === "dark" ? "#f2a93a" : "#f4a81d", "line-opacity": 0.38, "line-width": 40, "line-blur": 0 } });
        map.addSource("trip", { type: "geojson", data: tripData() });
        map.addLayer({ id: "trip-casing", type: "line", source: "trip", layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": theme === "dark" ? "#24211b" : "#fff4da", "line-width": 8 } });
        map.addLayer({ id: "trip-line", type: "line", source: "trip", layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": ["get", "color"], "line-width": 4 } });
        syncRouteVisibility();
        syncTerrain();
    }

    function syncRouteVisibility() {
        if (!map?.getLayer("trip-line")) return;
        for (const id of ["trip-line", "trip-casing", "trip-highlight"]) map.setLayoutProperty(id, "visibility", showRoute ? "visible" : "none");
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

    onMount(() => {
        appliedTheme = theme;
        maplibregl.setWorkerUrl(mapWorkerUrl);
        const protocol = new Protocol();
        maplibregl.addProtocol("pmtiles", protocol.tile);
        dem = new mlcontour.DemSource({ url: TERRAIN_URL, maxzoom: 12, worker: true, cacheSize: 64, encoding: "terrarium" });
        dem.setupMaplibre(maplibregl);
        const contourUrl = dem.contourProtocolUrl({ thresholds: { 10: [200, 1000], 11: [100, 500], 13: [50, 250], 14: [20, 100] }, contourLayer: "contours", elevationKey: "ele", levelKey: "level" });
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
                if (target instanceof Node && popupContent?.contains(target)) return;
                onMapClick?.([event.lngLat.lng, event.lngLat.lat]);
            });
            map.on("moveend", reportView);
        } catch (error) {
            failure = "The map could not start. This view needs a browser with WebGL enabled.";
            errorDetail = error instanceof Error ? error.message : String(error);
        }
        const observer = new ResizeObserver(() => map?.resize());
        observer.observe(container);
        return () => {
            observer.disconnect();
            markerList.forEach((marker) => marker.remove());
            callout?.remove();
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
    $effect(() => { if (map) map.getCanvas().style.cursor = pickMode ? "crosshair" : ""; });

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

    $effect(() => {
        if (!map) return;
        markerList.forEach((marker) => marker.remove());
        let nightNumber = 0;
        markerList = points.filter((point) => showRoute || point.kind === "place" || point.kind === "marker").map((point) => {
            if (point.kind === "night") nightNumber++;
            const draggable = !!onPointMove && (point.draggable ?? (point.kind !== "place" && point.appearance !== "suggested"));
            const button = document.createElement("button");
            button.className = `planner-map-pin ${point.kind} ${point.appearance ?? ""}${draggable ? " draggable" : ""}${point.id === selectedId ? " selected" : ""}`;
            if (point.color) { button.style.backgroundColor = point.color; button.style.color = theme === "dark" ? "#201f17" : "white"; }
            button.setAttribute("aria-label", point.label);
            button.setAttribute("aria-pressed", String(point.id === selectedId));
            button.title = point.label + (draggable ? " · drag to move" : "");
            if (point.kind === "place" && (point.appearance === "hotel" || point.appearance === "camp")) {
                button.append(markerIcon(point.appearance));
            } else if (point.kind === "waypoint" || point.kind === "detour") {
                button.append(markerIcon(point.kind));
            } else {
                button.textContent = point.markerLabel ?? (point.kind === "start" ? "A" : point.kind === "finish" ? "B" : point.kind === "night" ? String(nightNumber) : "");
            }
            button.addEventListener("click", (event) => { event.stopPropagation(); onPointSelect?.(point.id); });
            const marker = new maplibregl.Marker({ element: button, draggable }).setLngLat(point.coordinate).addTo(map!);
            marker.on("dragend", () => { const p = marker.getLngLat(); onPointMove?.(point.id, [p.lng, p.lat]); });
            return marker;
        });
    });
    $effect(() => {
        if (!map || !popupContent) return;
        const point = points.find((entry) => entry.id === selectedId);
        if (!point || !popup || (!showRoute && point.kind !== "place" && point.kind !== "marker")) { callout?.remove(); callout = undefined; return; }
        callout ??= new maplibregl.Popup({ closeButton: false, closeOnClick: false, offset: 20, maxWidth: "280px" }).setDOMContent(popupContent);
        callout.setLngLat(point.coordinate).addTo(map);
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
    :global(.planner-map-pin) { width: 28px; height: 28px; display: grid; place-items: center; padding: 0; border: 2px solid #fff7e7; border-radius: 50%; background: #a4501e; color: white; font: 700 12px var(--sans, sans-serif); cursor: pointer; box-shadow: 0 2px 5px #0003; }
    :global(.planner-map-pin.draggable) { cursor: grab; }
    :global(.planner-map-pin.via) { width: 16px; height: 16px; background: #cc2a93; }
    :global(.planner-map-pin.pass) { width: 22px; height: 22px; background: #fffdf6; border: 3px solid #cc2a93; }
    :global(.planner-map-pin.marker) { width: 24px; height: 24px; background: #e0ece7; border-color: #50736b; color: #2e5148; }
    :global(.planner-map-pin.marker:empty)::after { content: ""; width: 6px; height: 6px; border-radius: 50%; background: currentColor; }
    :global(.planner-map-pin.place) { width: 32px; height: 32px; color: #34493c; background: #fffdf6; border-color: #9da894; }
    :global(.planner-map-pin svg) { width: 21px; height: 21px; }
    :global(.planner-map-pin.waypoint) { background: #674667; }
    :global(.planner-map-pin.detour) { background: #fffdf6; color: #486a87; border-color: #7891a4; }
    :global(.planner-map-pin.detour svg) { width: 18px; height: 18px; }
    :global(.planner-map-pin.suggested) { color: #365247; background: #f2f5e9; border: 2px dashed #708c77; box-shadow: none; }
    :global(.planner-map-pin.night:not(.suggested)) { color: #fffdf6; background: #294c41; border-color: #fff7e7; }
    :global(.planner-map-pin.selected) { outline: 3px solid #c59436; outline-offset: 3px; }
    :global(.planner-map-pin:hover) { filter: brightness(1.08); }
    :global(.planner-map-pin:focus-visible) { outline: 3px solid #f4a81d; outline-offset: 3px; }
    .map-frame :global(.maplibregl-popup-content) { padding: 0; border-radius: 6px; color: var(--ink, #1c1b14); background: var(--panel, white); font-family: var(--sans, sans-serif); box-shadow: 0 5px 18px #0003; }
    .map-frame :global(.maplibregl-popup-anchor-bottom .maplibregl-popup-tip) { border-top-color: var(--panel, white); }
    .map-frame :global(.maplibregl-popup-anchor-top .maplibregl-popup-tip) { border-bottom-color: var(--panel, white); }
</style>
