<!--
  One preview for routes and rides: the track on a real basemap, the stats, the elevation profile.

  A plain overlay, not a `<dialog>` — same WebKitGTK reasoning as `ConfirmDialog.svelte`. The map
  is a second, short-lived Leaflet instance: created when the modal mounts, `invalidateSize`d on
  the next frame (the container has no size until layout has run), removed on teardown. Leaflet's
  CSS is already global (`main.ts`), and this component only ever lives inside the lazily-loaded
  device/rides chunks, so the static `import L` adds nothing to the entry bundle it wasn't
  already carrying via the region picker.
-->
<script lang="ts">
    import L from "leaflet";
    import { elevationProfile, type ProfilePoint } from "../../lib/device/elevation";

    const PROFILE = { width: 600, height: 64 };

    let {
        title,
        points,
        stats,
        onclose,
        actions,
    }: {
        title: string;
        points: readonly ProfilePoint[];
        /** Label/value pairs for the stats column, already formatted. */
        stats: ReadonlyArray<{ label: string; value: string }>;
        onclose: () => void;
        /** The footer's left side — Delete, Pull, whatever the object supports. */
        actions?: import("svelte").Snippet;
    } = $props();

    let mapEl = $state<HTMLDivElement>();
    let closeButton = $state<HTMLButtonElement>();

    const profile = $derived(elevationProfile(points, PROFILE.width, PROFILE.height));

    $effect(() => {
        closeButton?.focus();
    });

    $effect(() => {
        if (!mapEl) return;
        const map = L.map(mapEl, { zoomControl: false });
        L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", {
            attribution: "&copy; OpenStreetMap contributors",
        }).addTo(map);
        const latlngs = points.map((p) => [p.lat, p.lon] as [number, number]);
        if (latlngs.length > 1) {
            const line = L.polyline(latlngs, { color: "#cf6a2a", weight: 3 });
            line.addTo(map);
            L.circleMarker(latlngs[0], { radius: 5, color: "#3c6b39", fillColor: "#3c6b39", fillOpacity: 1 }).addTo(map);
            L.circleMarker(latlngs[latlngs.length - 1], { radius: 5, color: "#3c6b39", fillOpacity: 0 }).addTo(map);
            map.fitBounds(line.getBounds(), { padding: [18, 18] });
        } else {
            map.setView(latlngs[0] ?? [0, 0], latlngs.length ? 13 : 2);
        }
        // The container was laid out after `L.map` measured it; recheck on the next frame.
        const raf = requestAnimationFrame(() => map.invalidateSize());
        return () => {
            cancelAnimationFrame(raf);
            map.remove();
        };
    });

    function onKeydown(event: KeyboardEvent) {
        if (event.key === "Escape") {
            event.preventDefault();
            onclose();
        }
    }
</script>

<svelte:window on:keydown={onKeydown} />

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onclose()}>
    <div class="sheet card" role="dialog" aria-modal="true" aria-labelledby="preview-title">
        <div class="head">
            <h3 id="preview-title">{title}</h3>
            <button type="button" class="btn ghost" bind:this={closeButton} onclick={onclose} aria-label="Close">
                ✕
            </button>
        </div>

        <div class="body">
            <div class="map" bind:this={mapEl}></div>
            <div class="stats">
                {#each stats as stat (stat.label)}
                    <p class="stat">
                        <span class="small faint label">{stat.label}</span>
                        <b>{stat.value}</b>
                    </p>
                {/each}
            </div>
        </div>

        {#if profile}
            <div class="profile">
                <svg
                    viewBox="0 0 {PROFILE.width} {PROFILE.height}"
                    preserveAspectRatio="none"
                    aria-label="Elevation profile"
                    role="img"
                >
                    <path class="area" d={profile.areaPath} />
                    <path class="line" d={profile.linePath} />
                </svg>
                <p class="small faint">
                    Elevation · {Math.round(profile.minEle).toLocaleString()} –
                    {Math.round(profile.maxEle).toLocaleString()} m
                </p>
            </div>
        {/if}

        <div class="foot">
            {@render actions?.()}
            <button type="button" class="btn right" onclick={onclose}>Close</button>
        </div>
    </div>
</div>

<style>
    .backdrop {
        position: fixed;
        inset: 0;
        z-index: 1500; /* under ConfirmDialog's 2000, so "delete?" asks on top of the preview */
        background: rgba(32, 48, 29, 0.38);
        display: flex;
        align-items: center;
        justify-content: center;
        padding: 20px;
    }

    .sheet {
        width: min(680px, 100%);
        max-height: min(90vh, 640px);
        overflow-y: auto;
        display: flex;
        flex-direction: column;
        gap: 12px;
        box-shadow: 0 18px 44px rgba(32, 48, 29, 0.28);
    }

    .head {
        display: flex;
        align-items: center;
        gap: 10px;
    }

    h3 {
        font-family: var(--serif);
        font-size: 17px;
        margin: 0;
        flex: 1;
        min-width: 0;
    }

    .body {
        display: grid;
        grid-template-columns: 1.4fr 1fr;
        gap: 12px;
    }

    @media (max-width: 560px) {
        .body {
            grid-template-columns: 1fr;
        }
    }

    .map {
        height: 220px;
        border: 1px solid var(--line);
        border-radius: 9px;
        overflow: hidden;
        /* Leaflet panes sit at z-index up to 700 — keep them inside this box's
           stacking context so tiles never paint over the modal chrome. */
        position: relative;
        isolation: isolate;
    }

    .stats {
        display: flex;
        flex-direction: column;
        gap: 8px;
        align-self: center;
    }

    .stat {
        margin: 0;
    }

    .stat .label {
        display: block;
        text-transform: uppercase;
        letter-spacing: 0.05em;
        font-size: 10.5px;
    }

    .stat b {
        font-size: 15px;
        font-variant-numeric: tabular-nums;
    }

    .profile svg {
        display: block;
        width: 100%;
        height: 64px;
    }

    .profile .area {
        fill: var(--wood);
        fill-opacity: 0.25;
    }

    .profile .line {
        fill: none;
        stroke: var(--forest);
        stroke-width: 1.6;
    }

    .profile p {
        margin: 3px 0 0;
    }

    .foot {
        display: flex;
        align-items: center;
        gap: 8px;
        border-top: 1px solid var(--line);
        padding-top: 10px;
    }

    .foot .right {
        margin-left: auto;
    }
</style>
