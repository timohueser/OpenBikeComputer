import L from "leaflet";
import { MAP_BOUNDS } from "../planner/map-data";

/**
 * Puts the planner's own basemap under everything else on `map`. MapLibre loads on
 * demand, so the builder's first paint does not carry it; the map is usable before
 * the tiles arrive.
 *
 * OpenStreetMap's raster tiles sit one pane lower and load only when ours cannot
 * serve the view: the tile source fails, MapLibre cannot start, or the view leaves
 * the covered bounds. The style has no background layer, so they show through
 * wherever ours paint nothing.
 */
export async function addBasemap(map: L.Map): Promise<void> {
    map.createPane("basemap-fallback").style.zIndex = "150";
    const fallback = L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", {
        pane: "basemap-fallback",
        maxZoom: 19,
        attribution: "&copy; OpenStreetMap contributors",
    });
    const useFallback = () => {
        if (!map.hasLayer(fallback)) fallback.addTo(map);
    };
    if (MAP_BOUNDS) {
        const [west, south, east, north] = MAP_BOUNDS;
        const covered = L.latLngBounds([south, west], [north, east]);
        const check = () => {
            // A world-scale view overhangs the mercator world; only ground that exists counts.
            const view = map.getBounds();
            const ground = L.latLngBounds(
                [Math.max(view.getSouth(), -85), Math.max(view.getWest(), -180)],
                [Math.min(view.getNorth(), 85), Math.min(view.getEast(), 180)],
            );
            if (!covered.contains(ground)) useFallback();
        };
        map.on("moveend", check);
        check();
    }

    try {
        const [maplibregl, { Protocol }, { default: workerUrl }, { maplibreGL }, { basemapStyle }] = await Promise.all([
            import("maplibre-gl"),
            import("pmtiles"),
            import("maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url"),
            import("@maplibre/maplibre-gl-leaflet"),
            import("../planner/map-style"),
            import("maplibre-gl/dist/maplibre-gl.css"),
        ]);
        // `remove()` clears the container's id; the import can finish after the view is gone.
        if (!(map.getContainer() as HTMLElement & { _leaflet_id?: number })._leaflet_id) return;
        // MapLibre 6 needs WebGL2.
        if (!document.createElement("canvas").getContext("webgl2")) throw new Error("WebGL2 is unavailable");
        maplibregl.setWorkerUrl(workerUrl);
        maplibregl.addProtocol("pmtiles", new Protocol().tile);
        const layer = maplibreGL({ style: basemapStyle(), renderWorldCopies: false, interactive: false }).addTo(map);
        layer.getMaplibreMap().on("error", (event) => {
            if ((event as { sourceId?: string }).sourceId === "basemap") useFallback();
        });
    } catch (error) {
        console.error("basemap failed to load", error);
        useFallback();
    }
}
