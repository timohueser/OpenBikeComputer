<script lang="ts">
    import "leaflet/dist/leaflet.css";
    import { onMount } from "svelte";
    import { CoverageMapView, type DegPoint } from "../../lib/map/coverageMap";

    // Squares and an outline on the basemap. With `picking`, a click picks the smallest of `regions`
    // that holds the point; the regions are not drawn. With `drawing`, a drag draws a box.
    type Bounds = [DegPoint, DegPoint];
    let {
        cells = [],
        outline = [],
        regions = [],
        picking = false,
        drawing = false,
        onpick,
        onbox,
    }: {
        cells?: { bounds: Bounds; color: string; label: string }[];
        outline?: Bounds[];
        regions?: { id: string; name: string; rings: DegPoint[][] }[];
        picking?: boolean;
        drawing?: boolean;
        onpick?: (id: string) => void;
        onbox?: (south: number, west: number, north: number, east: number) => void;
    } = $props();

    let element: HTMLElement;
    let view = $state<CoverageMapView>();
    const ring = ([[south, west], [north, east]]: Bounds): DegPoint[] => [[south, west], [south, east], [north, east], [north, west], [south, west]];
    onMount(() => {
        const box = (...edges: number[]) => edges.map((edge) => edge.toFixed(3)).join(", ");
        view = new CoverageMapView(element, {
            onRegionPick: (id) => onpick?.(id),
            onRegionLadder: (ids) => onpick?.(ids[0]),
            onBoxDrawn: (south, west, north, east) => onbox?.(south, west, north, east),
            boxDragLabel: (south, west, north, east) => box(west, south, east, north),
            onLassoDrawn: () => {},
            lassoDragLabel: () => "",
            onDrawEnd: () => drawing && view?.armBoxDraw(),
            onWarningClick: () => {},
        });
        return () => view?.destroy();
    });
    $effect(() => view?.setCells(cells));
    $effect(() => view?.setParts([{ id: "area", kind: "box", rings: outline.map(ring), highlighted: false }]));
    $effect(() => view?.setRegions(regions, true));
    $effect(() => view?.setRegionToolArmed(picking));
    $effect(() => void (drawing ? view?.armBoxDraw() : view?.cancelBoxDraw()));
    // The map frames the squares and the outline once they are known, and again when they move.
    const frame = $derived.by(() => {
        const all = [...cells.map((cell) => cell.bounds), ...outline];
        if (!all.length) return "";
        return JSON.stringify([Math.min(...all.map((b) => b[0][0])), Math.min(...all.map((b) => b[0][1])), Math.max(...all.map((b) => b[1][0])), Math.max(...all.map((b) => b[1][1]))]);
    });
    $effect(() => {
        if (view && frame) view.flyTo(...(JSON.parse(frame) as [number, number, number, number]));
    });
</script>

<div class="map" bind:this={element}></div>

<style>
    .map {
        height: 360px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
    }
</style>
