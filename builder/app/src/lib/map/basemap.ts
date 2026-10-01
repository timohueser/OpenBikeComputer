import L from "leaflet";

/** Load the shared vector basemap below the Leaflet overlays. */
export async function addBasemap(map: L.Map): Promise<void> {
    let removed = false;
    map.once("unload", () => { removed = true; });
    let warning: L.Control | undefined;
    const fail = (error: unknown) => {
        if (removed) return;
        console.error("basemap failed to load", error);
        if (warning) return;
        warning = new L.Control({ position: "bottomleft" });
        warning.onAdd = () => {
            const message = L.DomUtil.create("div", "leaflet-bar basemap-warning");
            message.textContent = "Map background unavailable.";
            message.setAttribute("role", "status");
            return message;
        };
        warning.addTo(map);
    };

    try {
        const [maplibregl, { Protocol }, { default: workerUrl }, { maplibreGL }, { basemapStyle }, { basemapConfig }] = await Promise.all([
            import("maplibre-gl"),
            import("pmtiles"),
            import("maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url"),
            import("@maplibre/maplibre-gl-leaflet"),
            import("../planner/map-style"),
            import("./basemap-config"),
            import("maplibre-gl/dist/maplibre-gl.css"),
        ]);
        if (removed) return;
        const config = await basemapConfig();
        if (removed) return;
        const context = document.createElement("canvas").getContext("webgl2");
        if (!context) throw new Error("WebGL2 is unavailable");
        context.getExtension("WEBGL_lose_context")?.loseContext();
        maplibregl.setWorkerUrl(workerUrl);
        maplibregl.addProtocol("pmtiles", new Protocol().tile);
        const theme = () => (document.documentElement.dataset.theme === "dark" ? "dark" : "light");
        const layer = maplibreGL({ style: basemapStyle(theme(), config), renderWorldCopies: false, interactive: false }).addTo(map);
        const gl = layer.getMaplibreMap();
        gl.on("error", (event) => fail(event.error));
        const themed = new MutationObserver(() => gl.setStyle(basemapStyle(theme(), config)));
        themed.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
        map.once("unload", () => themed.disconnect());
    } catch (error) {
        fail(error);
    }
}
