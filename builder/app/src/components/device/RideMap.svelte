<!--
  The logbook's sticky all-rides map (#894 ride-library redesign, the "Logbook" option with
  Option 1's cluster behavior): every stored preview track drawn in semi-transparent ink, the
  hovered ride popping coral on top, and — below the cluster zoom — forest circles with ride
  counts instead of tracks. The circles and the tracks never show at once: every zoom change
  redraws exactly one of the two layers (`lib/device/rideMap` owns the math; this component only
  draws its answers).

  Leaflet lifecycle follows PreviewModal's conventions: one map per mount, `invalidateSize` on the
  next frame, everything removed on teardown. Hover is two-way but this component only *reports*
  (`onhover`) — the parent owns the hovered key, so a hover that started on a list row and one
  that started on a track render identically.
-->
<script lang="ts">
    import L from "leaflet";
    import { untrack } from "svelte";
    import { clusterRides, clustersAt, type RideTrack } from "../../lib/device/rideMap";

    interface MapRide extends RideTrack {
        readonly name: string;
    }

    let {
        rides,
        hovered = null,
        onhover,
        onopen,
    }: {
        /** Every previewable ride: key, display name, stored preview track (2+ points). */
        rides: readonly MapRide[];
        /** The highlighted ride, whoever started the hover. */
        hovered?: string | null;
        /** A hover started or ended on a track. Never fires in cluster mode. */
        onhover: (key: string | null) => void;
        /** A track was clicked — open its preview. */
        onopen: (key: string) => void;
    } = $props();

    const INK = "#24331c";
    const CORAL = "#cf6a2a";
    /** An empty library still shows a map — the south-west, where the first ride will land. */
    const EMPTY_CENTER: [number, number] = [48.5, 9.0];
    const EMPTY_ZOOM = 5;

    let mapEl = $state<HTMLDivElement>();
    let map: L.Map | null = null;
    let layer: L.LayerGroup | null = null;
    const lines = new Map<string, L.Polyline>();
    let fitted = false;

    function lineStyle(key: string, hot: string | null): L.PathOptions {
        return key === hot ? { color: CORAL, weight: 3.5, opacity: 1 } : { color: INK, weight: 2, opacity: 0.3 };
    }

    function allBounds(all: readonly MapRide[]): L.LatLngBounds | null {
        const points = all.flatMap((r) => r.track);
        if (points.length < 2) return null;
        return L.latLngBounds(points.map((p) => [p[0], p[1]] as [number, number]));
    }

    /** Redraw the one layer the current zoom calls for. Reads props via `current`, not tracking. */
    function redraw(current: readonly MapRide[], hot: string | null) {
        const m = map;
        const l = layer;
        if (!m || !l) return;
        l.clearLayers();
        lines.clear();
        if (clustersAt(m.getZoom())) {
            for (const cluster of clusterRides(current, m.getZoom())) {
                const size = Math.min(46, 26 + cluster.count * 3);
                const marker = L.marker([cluster.center[0], cluster.center[1]], {
                    icon: L.divIcon({
                        className: "ridecluster-anchor",
                        html: `<div class="ridecluster" style="width:${size}px;height:${size}px">${cluster.count}</div>`,
                        iconSize: [size, size],
                    }),
                    title: `${cluster.count} ride${cluster.count === 1 ? "" : "s"}`,
                });
                marker.on("click", () => {
                    // Zooming into the badge's bounds is what dissolves it into tracks; the cap
                    // keeps a single short ride from diving to house-number zoom.
                    m.fitBounds(
                        [
                            [cluster.bounds[0][0], cluster.bounds[0][1]],
                            [cluster.bounds[1][0], cluster.bounds[1][1]],
                        ],
                        { padding: [40, 40], maxZoom: 13 },
                    );
                });
                marker.addTo(l);
            }
        } else {
            for (const ride of current) {
                if (ride.track.length < 2) continue;
                const line = L.polyline(
                    ride.track.map((p) => [p[0], p[1]] as [number, number]),
                    lineStyle(ride.key, hot),
                );
                // `textContent` semantics: bindTooltip escapes by default only for plain strings
                // it does not — so build the node ourselves; the name is rider data, not markup.
                line.bindTooltip(() => {
                    const el = document.createElement("div");
                    el.textContent = ride.name;
                    return el;
                }, { sticky: true });
                line.on("mouseover", () => onhover(ride.key));
                line.on("mouseout", () => onhover(null));
                line.on("click", () => onopen(ride.key));
                line.addTo(l);
                lines.set(ride.key, line);
            }
            const top = hot && lines.get(hot);
            if (top) top.bringToFront();
        }
    }

    $effect(() => {
        if (!mapEl) return;
        const m = L.map(mapEl);
        L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", {
            attribution: "&copy; OpenStreetMap contributors",
        }).addTo(m);
        layer = L.layerGroup().addTo(m);
        map = m;
        const initial = untrack(() => rides);
        const bounds = allBounds(initial);
        if (bounds) {
            m.fitBounds(bounds, { padding: [32, 32] });
            fitted = true;
        } else {
            m.setView(EMPTY_CENTER, EMPTY_ZOOM);
        }
        m.on("zoomend", () => redraw(untrack(() => rides), untrack(() => hovered)));
        redraw(initial, untrack(() => hovered));
        const raf = requestAnimationFrame(() => m.invalidateSize());
        return () => {
            cancelAnimationFrame(raf);
            lines.clear();
            layer = null;
            map = null;
            m.remove();
        };
    });

    // New or changed rides (a pull landed): redraw, and fit once the library stops being empty.
    $effect(() => {
        const current = rides;
        untrack(() => {
            redraw(current, hovered);
            if (!fitted) {
                const bounds = allBounds(current);
                if (bounds && map) {
                    map.fitBounds(bounds, { padding: [32, 32] });
                    fitted = true;
                }
            }
        });
    });

    // Hover moved (either direction): restyle in place, no relayout, no redraw.
    $effect(() => {
        const hot = hovered;
        for (const [key, line] of lines) {
            line.setStyle(lineStyle(key, hot));
        }
        const top = hot && lines.get(hot);
        if (top) top.bringToFront();
    });
</script>

<div class="ridemap" bind:this={mapEl}></div>

<style>
    .ridemap {
        width: 100%;
        height: 100%;
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
        /* Leaflet panes reach z-index 700; keep them inside this box's stacking context. */
        position: relative;
        isolation: isolate;
        overflow: hidden;
    }

    /* The cluster badge DOM is injected by Leaflet, so its styles escape the component scope. */
    :global(.ridecluster-anchor) {
        background: transparent;
        border: 0;
    }

    :global(.ridecluster) {
        display: flex;
        align-items: center;
        justify-content: center;
        border-radius: 50%;
        background: var(--forest);
        color: var(--parchment);
        font-family: var(--sans);
        font-size: 13px;
        font-weight: 650;
        border: 2px solid var(--panel);
        box-shadow: 0 2px 8px rgba(36, 51, 28, 0.28);
        cursor: pointer;
        transform: translate(-0px, -0px);
    }

    :global(.ridecluster:hover) {
        background: var(--forest-deep);
    }
</style>
