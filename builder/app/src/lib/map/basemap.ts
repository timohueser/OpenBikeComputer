import L from "leaflet";
import { MAP_BOUNDS } from "../planner/map-data";

/**
 * Puts the planner's own basemap under everything else on `map`. MapLibre loads on
 * demand, so the builder's first paint does not carry it; the map is usable before
 * the tiles arrive.
 *
 * OpenStreetMap's raster tiles load only when ours cannot serve the view: the tile
 * source fails, MapLibre cannot start, or the view leaves the covered bounds. They sit
 * above our layer, clipped to the ground outside those bounds, because the vector
 * land fill spills past its last tile. After a failure the clip goes and they cover
 * everything.
 */
export async function addBasemap(map: L.Map): Promise<void> {
    const pane = map.createPane("basemap-fallback");
    pane.style.zIndex = "250";
    const fallback = L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", {
        pane: "basemap-fallback",
        maxZoom: 19,
        attribution: "&copy; OpenStreetMap contributors",
    });
    const covered = MAP_BOUNDS && L.latLngBounds([MAP_BOUNDS[1], MAP_BOUNDS[0]], [MAP_BOUNDS[3], MAP_BOUNDS[2]]);
    let outage = false;
    const update = () => {
        if (!covered || outage) {
            pane.style.clipPath = "";
        } else {
            const from = map.latLngToLayerPoint(covered.getNorthWest());
            const to = map.latLngToLayerPoint(covered.getSouthEast());
            pane.style.clipPath = `path(evenodd, "M-1e7 -1e7H1e7V1e7H-1e7ZM${from.x} ${from.y}H${to.x}V${to.y}H${from.x}Z")`;
            // A world-scale view overhangs the mercator world; only ground that exists counts.
            const view = map.getBounds();
            const ground = L.latLngBounds(
                [Math.max(view.getSouth(), -85), Math.max(view.getWest(), -180)],
                [Math.min(view.getNorth(), 85), Math.min(view.getEast(), 180)],
            );
            if (!covered.contains(ground)) fallback.addTo(map);
        }
        if (outage) fallback.addTo(map);
    };
    const fail = (error: unknown) => {
        console.error("basemap failed to load", error);
        outage = true;
        update();
    };
    map.on("moveend zoomend viewreset", update);
    update();

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
        const theme = () => (document.documentElement.dataset.theme === "dark" ? "dark" : "light");
        const layer = maplibreGL({ style: basemapStyle(theme()), renderWorldCopies: false, interactive: false }).addTo(map);
        const gl = layer.getMaplibreMap();
        gl.on("error", (event) => {
            if ((event as { sourceId?: string }).sourceId === "basemap") fail(event.error);
        });
        // The page theme is the app's `data-theme`; the fallback's raster tiles are inverted in CSS.
        const themed = new MutationObserver(() => gl.setStyle(basemapStyle(theme())));
        themed.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
        map.on("unload", () => themed.disconnect());
    } catch (error) {
        fail(error);
    }
}
