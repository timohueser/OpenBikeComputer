import type L from "leaflet";

/**
 * Puts the planner's own basemap under everything else on `map`. MapLibre loads on
 * demand, so the builder's first paint does not carry it; the map is usable before
 * the tiles arrive.
 */
export async function addBasemap(map: L.Map): Promise<void> {
    const [maplibregl, { Protocol }, { default: workerUrl }, { maplibreGL }, { basemapStyle }] = await Promise.all([
        import("maplibre-gl"),
        import("pmtiles"),
        import("maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url"),
        import("@maplibre/maplibre-gl-leaflet"),
        import("../planner/map-style"),
        import("maplibre-gl/dist/maplibre-gl.css"),
    ]);
    // `remove()` clears the container's id; the import can finish after the view is gone.
    // MapLibre 6 needs WebGL2: without it the picker stays usable, just without a basemap.
    if (!(map.getContainer() as HTMLElement & { _leaflet_id?: number })._leaflet_id) return;
    if (!document.createElement("canvas").getContext("webgl2")) return;
    maplibregl.setWorkerUrl(workerUrl);
    maplibregl.addProtocol("pmtiles", new Protocol().tile);
    maplibreGL({ style: basemapStyle(), renderWorldCopies: false, interactive: false }).addTo(map);
}
