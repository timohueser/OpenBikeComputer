<script lang="ts">
    import L from "leaflet";
    import "leaflet/dist/leaflet.css";
    import { onMount } from "svelte";
    import { corners, extent, union, type Bbox } from "../../lib/data/areas";
    import type { DegPoint } from "../../lib/map/coverageMap";

    // The OpenStreetMap tiles of the whole world, because the operator picks areas that no release
    // covers yet. It draws coloured squares, the boxes that a target serves, and on the Bake screen
    // the area of the bake: a drag draws it while `drawing`, and its handles resize and move it.
    // The area, the outline and the preview take their colours from CSS rules on their class.
    type Bounds = [DegPoint, DegPoint];
    let {
        cells = [],
        outline = [],
        area = null,
        preview = null,
        frame,
        drawing = $bindable(false),
        onarea,
    }: {
        cells?: { bounds: Bounds; color: string; label: string }[];
        /** What the target serves now. */
        outline?: Bounds[];
        /** The area of the bake. With `onarea`, it has handles. */
        area?: Bbox | null;
        /** An area that the list points at. */
        preview?: Bbox | null;
        /** The map fits this box each time it changes; by default the squares and the outline. */
        frame?: Bbox | null;
        drawing?: boolean;
        onarea?: (box: Bbox) => void;
    } = $props();

    let element: HTMLElement;
    let map = $state<L.Map>();
    const layers = { cells: L.layerGroup(), outline: L.layerGroup(), preview: L.layerGroup(), area: L.layerGroup() };
    const tip = L.tooltip({ className: "map-tip", direction: "top", offset: [0, -10], permanent: true });

    const boxOf = (bounds: L.LatLngBounds): Bbox => {
        const edge = (value: number) => +value.toFixed(4);
        return { west: edge(bounds.getWest()), south: edge(bounds.getSouth()), east: edge(bounds.getEast()), north: edge(bounds.getNorth()) };
    };
    const latLngBounds = (box: Bbox) => L.latLngBounds(corners(box));
    const rect = (bounds: L.LatLngBoundsExpression, className: string) => L.rectangle(bounds, { className, interactive: false });
    const showTip = (bounds: L.LatLngBounds) => tip.setLatLng(bounds.getNorthEast()).setContent(extent(boxOf(bounds))).addTo(map!);

    onMount(() => {
        map = L.map(element, { worldCopyJump: true, boxZoom: false, maxZoom: 18 }).setView([49, 9], 4);
        L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", { maxZoom: 19, attribution: "© OpenStreetMap contributors" }).addTo(map);
        for (const layer of Object.values(layers)) layer.addTo(map);
        // The wheel scrolls the page until a click on the map, so a scroll past it does not zoom it.
        map.scrollWheelZoom.disable();
        map.on("click", () => map?.scrollWheelZoom.enable());
        map.on("mouseout", () => map?.scrollWheelZoom.disable());
        const resize = new ResizeObserver(() => map?.invalidateSize());
        resize.observe(element);
        return () => {
            resize.disconnect();
            map?.remove();
        };
    });

    $effect(() => {
        layers.cells.clearLayers();
        if (!map) return;
        for (const { bounds, color, label } of cells) {
            L.rectangle(bounds, { color, weight: 1.5, fillColor: color, fillOpacity: 0.3 }).bindTooltip(label, { className: "map-tip", sticky: true }).addTo(layers.cells);
        }
    });
    $effect(() => {
        layers.outline.clearLayers();
        if (!map) return;
        for (const bounds of outline) rect(bounds, "served").addTo(layers.outline);
    });
    $effect(() => {
        layers.preview.clearLayers();
        if (preview) rect(latLngBounds(preview), "preview").addTo(layers.preview);
    });

    // The area, and a handle on each corner and in the middle. A drag of a corner keeps the opposite
    // corner; a drag of the middle moves the whole box.
    $effect(() => {
        layers.area.clearLayers();
        if (!map || !area) return;
        let bounds = latLngBounds(area);
        const box = rect(bounds, "area").addTo(layers.area);
        if (!onarea) return;
        const spots = (b: L.LatLngBounds) => [b.getSouthWest(), b.getSouthEast(), b.getNorthEast(), b.getNorthWest(), b.getCenter()];
        const handle = (at: L.LatLng, i: number) =>
            L.marker(at, {
                draggable: true,
                keyboard: false,
                title: i === 4 ? "Drag to move the area" : "Drag to resize the area",
                icon: L.divIcon({ className: i === 4 ? "handle move" : "handle", iconSize: i === 4 ? [22, 22] : [14, 14] }),
            }).addTo(layers.area);
        const handles = spots(bounds).map(handle);
        handles.forEach((marker, i) => {
            let from = bounds;
            marker.on("dragstart", () => (from = bounds));
            marker.on("drag", () => {
                const at = marker.getLatLng();
                if (i === 4) {
                    const [dLat, dLng] = [at.lat - from.getCenter().lat, at.lng - from.getCenter().lng];
                    bounds = L.latLngBounds([from.getSouth() + dLat, from.getWest() + dLng], [from.getNorth() + dLat, from.getEast() + dLng]);
                } else bounds = L.latLngBounds(spots(from)[(i + 2) % 4], at);
                box.setBounds(bounds);
                spots(bounds).forEach((spot, j) => j !== i && handles[j].setLatLng(spot));
                showTip(bounds);
            });
            marker.on("dragend", () => {
                tip.remove();
                onarea(boxOf(bounds));
            });
        });
    });

    // While `drawing`, a drag draws a new area and a click does nothing. The pointer events stop the
    // mouse events that Leaflet listens to, so the map does not pan under the box.
    let start: L.LatLng | null = null;
    let rubber: L.Rectangle | null = null;
    $effect(() => {
        if (!map) return;
        if (drawing) map.dragging.disable();
        else map.dragging.enable();
    });
    function down(event: PointerEvent) {
        if (!drawing || !map || event.button !== 0 || (event.target as Element).closest(".leaflet-control")) return;
        event.preventDefault();
        element.setPointerCapture(event.pointerId);
        start = map.mouseEventToLatLng(event);
        rubber = rect(L.latLngBounds(start, start), "area drawn").addTo(map);
    }
    function move(event: PointerEvent) {
        if (!start || !rubber || !map) return;
        const bounds = L.latLngBounds(start, map.mouseEventToLatLng(event));
        rubber.setBounds(bounds);
        showTip(bounds);
    }
    function up() {
        if (!rubber || !map) return;
        const bounds = rubber.getBounds();
        rubber.remove();
        tip.remove();
        [start, rubber] = [null, null];
        const [a, b] = [map.latLngToContainerPoint(bounds.getNorthWest()), map.latLngToContainerPoint(bounds.getSouthEast())];
        if (Math.abs(a.x - b.x) < 8 || Math.abs(a.y - b.y) < 8) return;
        drawing = false;
        onarea?.(boxOf(bounds));
    }
    function key(event: KeyboardEvent) {
        if (event.key !== "Escape" || !drawing) return;
        rubber?.remove();
        tip.remove();
        [start, rubber, drawing] = [null, null, false];
    }

    // The map fits again only when the box to fit changes, not on each poll that draws the same cells.
    const fit = $derived.by(() => {
        const box = frame === undefined ? union([...cells.map((cell) => cell.bounds), ...outline].map(([[south, west], [north, east]]) => ({ west, south, east, north }))) : frame;
        return box ? [box.south, box.west, box.north, box.east].join() : "";
    });
    $effect(() => {
        if (!map || !fit) return;
        const [south, west, north, east] = fit.split(",").map(Number);
        map.fitBounds([[south, west], [north, east]], { maxZoom: 10, padding: [32, 32] });
    });
</script>

<svelte:window onkeydown={key} />

<div class="map" class:drawing role="application" aria-label="Map" bind:this={element} onpointerdown={down} onpointermove={move} onpointerup={up} onpointercancel={up}></div>

<style>
    .map {
        height: 360px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        background: var(--parchment-2);
        font: inherit;
    }
    .drawing {
        cursor: crosshair;
        touch-action: none;
        user-select: none;
    }
    .drawing :global(.leaflet-interactive),
    .drawing :global(.leaflet-grab) {
        cursor: crosshair;
    }
    :global([data-theme="dark"]) .map :global(.leaflet-tile-pane) {
        filter: brightness(0.78) saturate(0.85);
    }
    /* The selection of the planner is amber; what the target serves now is a quiet dashed water line. */
    .map :global(path.area) {
        stroke: var(--amber);
        stroke-width: 3px;
        fill: var(--amber);
        fill-opacity: 0.16;
    }
    .map :global(path.area.drawn) {
        stroke-dasharray: 6 4;
    }
    .map :global(path.served) {
        stroke: var(--water);
        stroke-width: 2px;
        stroke-dasharray: 2 5;
        stroke-linecap: round;
        fill: var(--water);
        fill-opacity: 0.06;
    }
    .map :global(path.preview) {
        stroke: var(--ink);
        stroke-width: 1.5px;
        stroke-dasharray: 5 4;
        fill: var(--ink);
        fill-opacity: 0.04;
    }
    .map :global(.handle) {
        background: var(--panel);
        border: 2px solid var(--ink);
        border-radius: 3px;
        box-shadow: 0 1px 4px rgb(0 0 0 / 0.3);
        cursor: nesw-resize;
    }
    /* The handles go south-west, south-east, north-east, north-west, then the middle. */
    .map :global(.handle:nth-of-type(even)) {
        cursor: nwse-resize;
    }
    .map :global(.handle.move) {
        border-radius: 999px;
        background: var(--amber);
        cursor: move;
    }
    .map :global(.map-tip) {
        padding: 3px 8px;
        background: var(--ink);
        color: var(--panel);
        border: 0;
        border-radius: 6px;
        box-shadow: var(--shadow);
        font: 600 12px/16px var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .map :global(.map-tip::before) {
        display: none;
    }
    .map :global(.leaflet-bar) {
        border: 0;
        border-radius: 8px;
        box-shadow: var(--shadow);
        overflow: hidden;
    }
    .map :global(.leaflet-bar a) {
        width: 32px;
        height: 32px;
        line-height: 30px;
        background: var(--panel);
        color: var(--ink);
        border-bottom-color: var(--line);
    }
    .map :global(.leaflet-bar a:hover) {
        background: var(--parchment-2);
    }
    .map :global(.leaflet-control-attribution) {
        background: color-mix(in srgb, var(--panel) 80%, transparent);
        color: var(--ink-soft);
        font-size: 11px;
    }
    .map :global(.leaflet-control-attribution a) {
        color: inherit;
    }
</style>
